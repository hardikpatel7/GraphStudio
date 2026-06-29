//! Per-tool-call wrapper. Tools route their dispatch through `ToolCtx::meter`
//! which is the single place where (a) SSE start/finish events are emitted,
//! (b) the cache is consulted, (c) the per-call timeout and response-size
//! cap are enforced, and (d) the measurement is sent to the meter writer.
//!
//! Tools that are NOT safe to cache (anything filter- or time-window-shaped:
//! `dataview_read`, `*_query`, `*_traverse`) call `meter_uncached` instead.

use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use serde_json::Value;
use tokio::sync::mpsc;

use super::super::cache::{ToolCache, DEFAULT_TTL};
use super::writer::{ApiCallRow, MeterEvent, MeterTx};

/// 1 MB response-size cap. Tool results larger than this are truncated with
/// a marker before being handed back to the model — prevents one wide query
/// from blowing the model's context (and the user's bill).
pub const MAX_BYTES: usize = 1024 * 1024;
/// Per-call timeout. The model sees `{"error":"timeout"}` so it can adapt
/// instead of the SSE stream hanging.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// SSE event variants emitted by the metering wrapper. The agent routes own
/// the serialization; this module only describes the shape.
#[derive(Clone, Debug)]
pub enum SseEvent {
    ToolCallStarted {
        call_id: String,
        tool: String,
        args_preview: String,
    },
    ToolCallFinished {
        call_id: String,
        duration_ms: i64,
        status: String,
        bytes_out: i64,
        source: &'static str, // "live" | "cache"
    },
}

#[derive(Clone)]
pub struct ToolCtx {
    pub prompt_id: String,
    pub sse: mpsc::Sender<SseEvent>,
    pub meter_tx: MeterTx,
    pub cache: Arc<ToolCache>,
}

impl ToolCtx {
    pub async fn meter_cached<F, Fut>(
        &self,
        tool: &'static str,
        args: &Value,
        f: F,
    ) -> Result<Value>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Value>>,
    {
        let call_id = uuid::Uuid::new_v4().to_string();
        let args_str = serde_json::to_string(args).unwrap_or_default();
        let args_bytes = args_str.len() as i64;
        let args_hash = stable_hash(&args_str);
        let key = ToolCache::key(tool, args_hash);

        let _ = self.sse.send(SseEvent::ToolCallStarted {
            call_id: call_id.clone(),
            tool: tool.to_string(),
            args_preview: preview(&args_str),
        }).await;

        let started_at_ms = Utc::now().timestamp_millis();
        let t0 = Instant::now();

        if let Some(hit) = self.cache.get(&key, DEFAULT_TTL) {
            let bytes_out = approx_bytes(&hit) as i64;
            self.record(tool, &call_id, started_at_ms, &t0, args_bytes, bytes_out, "cache_hit", None, "cache").await;
            return Ok(hit);
        }

        let (value, status_str) = match tokio::time::timeout(TIMEOUT, f()).await {
            Ok(Ok(v)) => (truncate(v), "ok"),
            Ok(Err(e)) => (serde_json::json!({ "error": e.to_string() }), "error"),
            Err(_)     => (serde_json::json!({ "error": "timeout" }), "timeout"),
        };
        let bytes_out = approx_bytes(&value) as i64;
        if status_str == "ok" {
            self.cache.put(key, value.clone());
        }
        self.record(tool, &call_id, started_at_ms, &t0, args_bytes, bytes_out, status_str, None, "live").await;
        Ok(value)
    }

    /// Same as `meter_cached` but skips the cache. For tools whose result
    /// depends on filters, time, or other inputs that change per call.
    pub async fn meter_uncached<F, Fut>(
        &self,
        tool: &'static str,
        args: &Value,
        f: F,
    ) -> Result<Value>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Value>>,
    {
        let call_id = uuid::Uuid::new_v4().to_string();
        let args_str = serde_json::to_string(args).unwrap_or_default();
        let args_bytes = args_str.len() as i64;

        let _ = self.sse.send(SseEvent::ToolCallStarted {
            call_id: call_id.clone(),
            tool: tool.to_string(),
            args_preview: preview(&args_str),
        }).await;

        let started_at_ms = Utc::now().timestamp_millis();
        let t0 = Instant::now();

        let (value, status_str) = match tokio::time::timeout(TIMEOUT, f()).await {
            Ok(Ok(v)) => (truncate(v), "ok"),
            Ok(Err(e)) => (serde_json::json!({ "error": e.to_string() }), "error"),
            Err(_)     => (serde_json::json!({ "error": "timeout" }), "timeout"),
        };
        let bytes_out = approx_bytes(&value) as i64;
        self.record(tool, &call_id, started_at_ms, &t0, args_bytes, bytes_out, status_str, None, "live").await;
        Ok(value)
    }

    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        tool: &str,
        call_id: &str,
        started_at_ms: i64,
        t0: &Instant,
        bytes_in: i64,
        bytes_out: i64,
        status: &str,
        error: Option<String>,
        source: &'static str,
    ) {
        let duration_ms = t0.elapsed().as_millis() as i64;
        self.meter_tx.record(MeterEvent::ApiCall(ApiCallRow {
            prompt_id: self.prompt_id.clone(),
            tool_name: tool.to_string(),
            started_at_ms,
            duration_ms,
            bytes_in,
            bytes_out,
            status: status.to_string(),
            error,
            // wrap.rs is the legacy ToolCtx path (unused by the Rig
            // flow); previews live on hook.rs only.
            args_preview: None,
            response_preview: None,
        }));
        let _ = self.sse.send(SseEvent::ToolCallFinished {
            call_id: call_id.to_string(),
            duration_ms,
            status: status.to_string(),
            bytes_out,
            source,
        }).await;
    }
}

fn preview(s: &str) -> String {
    let truncated: String = s.chars().take(200).collect();
    if truncated.len() < s.len() { format!("{truncated}…") } else { truncated }
}

fn approx_bytes(v: &Value) -> usize {
    serde_json::to_string(v).map(|s| s.len()).unwrap_or(0)
}

fn truncate(v: Value) -> Value {
    let body = match serde_json::to_string(&v) {
        Ok(s) => s,
        Err(_) => return v,
    };
    if body.len() <= MAX_BYTES { return v; }
    // Truncate at MAX_BYTES boundary; return a structured marker so the
    // model sees that data was elided rather than a stray fragment.
    let cut = body.chars().take(MAX_BYTES).collect::<String>();
    serde_json::json!({
        "truncated": true,
        "original_bytes": body.len(),
        "head": cut,
        "note": "Result exceeded 1MB cap; head retained, tail elided."
    })
}

fn stable_hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
