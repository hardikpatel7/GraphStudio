//! Single-writer task that drains `MeterEvent`s into `agent.db`.
//!
//! Why a single writer: SQLite-on-WAL handles concurrent readers fine, but
//! many concurrent writers serialize on the lock anyway. Funneling through
//! one task lets us batch (one transaction per drain) and keeps the SSE hot
//! path off the DB lock — the streaming code only does a non-blocking `send`
//! onto the mpsc channel.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::warn;

use super::super::db::AgentDb;

/// Bounded channel capacity — sized to absorb a couple of seconds of bursty
/// tool calls before back-pressure kicks in.
const CAPACITY: usize = 1024;
const DRAIN_INTERVAL: Duration = Duration::from_millis(100);
const DRAIN_BATCH: usize = 64;

#[derive(Clone, Debug)]
pub struct ApiCallRow {
    pub prompt_id: String,
    pub tool_name: String,
    pub started_at_ms: i64,
    pub duration_ms: i64,
    pub bytes_in: i64,
    pub bytes_out: i64,
    pub status: String,
    pub error: Option<String>,
    /// Truncated JSON the model passed as tool args. ~500 chars so big
    /// payloads stay out of the SQLite row but small ones (most calls)
    /// fit verbatim.
    pub args_preview: Option<String>,
    /// Truncated JSON the tool returned. ~2 KB so the user can read what
    /// came back without bloating the DB.
    pub response_preview: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LlmUsageRow {
    pub prompt_id: String,
    pub model: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub latency_ms: i64,
}

#[derive(Debug)]
pub enum MeterEvent {
    ApiCall(ApiCallRow),
    LlmUsage(LlmUsageRow),
}

#[derive(Clone)]
pub struct MeterTx {
    tx: mpsc::Sender<MeterEvent>,
}

impl MeterTx {
    /// Non-blocking send. Drops + logs on overflow rather than back-pressuring
    /// the SSE stream — metering is observability, not correctness.
    pub fn record(&self, ev: MeterEvent) {
        if let Err(e) = self.tx.try_send(ev) {
            warn!("meter channel overflow: {e}");
        }
    }
}

/// Spawn the writer task. Returns the `MeterTx` for plumbing into `ToolCtx`.
pub fn spawn(db: Arc<AgentDb>) -> MeterTx {
    let (tx, mut rx) = mpsc::channel::<MeterEvent>(CAPACITY);
    tokio::spawn(async move {
        let mut buf = Vec::with_capacity(DRAIN_BATCH);
        loop {
            // Wait for at least one event, then opportunistically drain more.
            let first = match rx.recv().await {
                Some(ev) => ev,
                None => break, // sender dropped — only happens on shutdown
            };
            buf.push(first);
            // Brief settle window so bursts land in the same transaction.
            tokio::time::sleep(DRAIN_INTERVAL).await;
            while let Ok(ev) = rx.try_recv() {
                buf.push(ev);
                if buf.len() >= DRAIN_BATCH { break; }
            }
            if let Err(e) = flush(&db, buf.drain(..)) {
                warn!("meter flush failed: {e:#}");
            }
        }
    });
    MeterTx { tx }
}

fn flush(db: &AgentDb, events: impl Iterator<Item = MeterEvent>) -> anyhow::Result<()> {
    use rusqlite::types::ToSql;

    let mut api_rows: Vec<Vec<Box<dyn ToSql>>> = Vec::new();
    let mut usage_rows: Vec<Vec<Box<dyn ToSql>>> = Vec::new();
    for ev in events {
        match ev {
            MeterEvent::ApiCall(r) => {
                api_rows.push(vec![
                    Box::new(r.prompt_id),
                    Box::new(r.tool_name),
                    Box::new(r.started_at_ms),
                    Box::new(r.duration_ms),
                    Box::new(r.bytes_in),
                    Box::new(r.bytes_out),
                    Box::new(r.status),
                    Box::new(r.error),
                    Box::new(r.args_preview),
                    Box::new(r.response_preview),
                ]);
            }
            MeterEvent::LlmUsage(r) => {
                usage_rows.push(vec![
                    Box::new(r.prompt_id),
                    Box::new(r.model),
                    Box::new(r.tokens_in),
                    Box::new(r.tokens_out),
                    Box::new(r.latency_ms),
                ]);
            }
        }
    }
    if !api_rows.is_empty() {
        db.execute_batch_inserts(
            "INSERT INTO api_call \
             (prompt_id, tool_name, started_at, duration_ms, bytes_in, bytes_out, status, error, args_preview, response_preview) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            api_rows.into_iter(),
        )?;
    }
    if !usage_rows.is_empty() {
        // ON CONFLICT REPLACE — a turn that re-reports usage overwrites.
        db.execute_batch_inserts(
            "INSERT INTO llm_usage (prompt_id, model, tokens_in, tokens_out, latency_ms) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(prompt_id) DO UPDATE SET \
               model = excluded.model, \
               tokens_in = excluded.tokens_in, \
               tokens_out = excluded.tokens_out, \
               latency_ms = excluded.latency_ms",
            usage_rows.into_iter(),
        )?;
    }
    Ok(())
}
