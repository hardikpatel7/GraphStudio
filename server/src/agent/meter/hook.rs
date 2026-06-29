//! Bridge between Rig's `PromptHook` and our SSE + meter pipeline.
//!
//! Rig fires three relevant events on a `PromptHook<M>`:
//!
//! - `on_tool_call(name, _, internal_id, args)` — *before* a tool is dispatched.
//!   We record `(internal_id → started_at)` so `on_tool_result` can compute
//!   duration, and emit a `ToolCallStarted` SSE so the UI shows a spinner.
//! - `on_tool_result(name, _, internal_id, args, result)` — *after* the tool
//!   returns (success or error). We close the timing entry, emit
//!   `ToolCallFinished`, and push an `ApiCall` row to the meter writer.
//! - `on_text_delta(delta, _)` — text token streamed from the model. Forwarded
//!   as a `TextDelta` SSE chunk so the UI renders incrementally.
//!
//! The hook is `Clone` per Rig's trait bound. State that must persist across
//! `on_tool_call` + `on_tool_result` (start timestamps) lives behind a
//! shared `Arc<Mutex<HashMap<…>>>`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::Utc;
use rig::agent::{HookAction, PromptHook, ToolCallHookAction};
use rig::completion::CompletionModel;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::writer::{ApiCallRow, MeterEvent, MeterTx};

/// What the hook forwards to the SSE route. Keeps the route's JSON shape
/// owned by `agent/routes/prompts.rs`; this module is provider-neutral.
#[derive(Debug, Clone)]
pub enum SseEvent {
    TextDelta { text: String },
    ToolCallStarted { call_id: String, tool: String, args_preview: String },
    ToolCallFinished { call_id: String, duration_ms: i64, status: String, bytes_out: i64 },
}

/// How much of the tool args / response we keep in the SQLite row for the
/// prompt-detail drawer. Keep these small — the rows can pile up and a
/// long DuckDB result is duplicated in `bytes_out` already. The UI shows
/// a "truncated" marker when these limits hit.
const MAX_ARGS_PREVIEW_BYTES:     usize = 600;
const MAX_RESPONSE_PREVIEW_BYTES: usize = 2_000;

#[derive(Clone)]
struct StartInfo {
    /// Public id we hand to the UI. Distinct from Rig's `internal_call_id`
    /// because Rig's id format is provider-specific; ours is a stable UUID.
    call_id: String,
    started_at_ms: i64,
    t0: Instant,
    args_bytes: i64,
    /// Truncated JSON of what the model passed as args. Captured on
    /// `on_tool_call` and forwarded through `on_tool_result` into the
    /// `api_call` row.
    args_preview: Option<String>,
}

#[derive(Clone)]
pub struct MeteringHook {
    pub prompt_id: String,
    pub sse_tx: mpsc::Sender<SseEvent>,
    pub meter_tx: MeterTx,
    pending: Arc<Mutex<HashMap<String, StartInfo>>>,
}

impl MeteringHook {
    pub fn new(prompt_id: String, sse_tx: mpsc::Sender<SseEvent>, meter_tx: MeterTx) -> Self {
        Self {
            prompt_id,
            sse_tx,
            meter_tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn send(&self, ev: SseEvent) {
        let _ = self.sse_tx.send(ev).await;
    }
}

fn args_preview(args: &str) -> String {
    let head: String = args.chars().take(200).collect();
    if head.len() < args.len() { format!("{head}…") } else { head }
}

/// Head-truncate a JSON string to fit in the SQLite row. Appends an
/// ellipsis marker so the UI can render it as such.
fn truncate_preview(s: &str, cap: usize) -> String {
    if s.len() <= cap { return s.to_string(); }
    let mut out: String = s.chars().take(cap).collect();
    out.push_str(&format!("…[truncated, original {} bytes]", s.len()));
    out
}

impl<M> PromptHook<M> for MeteringHook
where
    M: CompletionModel,
{
    fn on_tool_call(
        &self,
        tool_name: &str,
        _tool_call_id: Option<String>,
        internal_call_id: &str,
        args: &str,
    ) -> impl std::future::Future<Output = ToolCallHookAction> + Send {
        let info = StartInfo {
            call_id: Uuid::new_v4().to_string(),
            started_at_ms: Utc::now().timestamp_millis(),
            t0: Instant::now(),
            args_bytes: args.len() as i64,
            args_preview: Some(truncate_preview(args, MAX_ARGS_PREVIEW_BYTES)),
        };
        let preview = args_preview(args);
        if let Ok(mut g) = self.pending.lock() {
            g.insert(internal_call_id.to_string(), info.clone());
        }
        let me = self.clone();
        let tool = tool_name.to_string();
        async move {
            me.send(SseEvent::ToolCallStarted {
                call_id: info.call_id,
                tool,
                args_preview: preview,
            }).await;
            ToolCallHookAction::cont()
        }
    }

    fn on_tool_result(
        &self,
        tool_name: &str,
        _tool_call_id: Option<String>,
        internal_call_id: &str,
        _args: &str,
        result: &str,
    ) -> impl std::future::Future<Output = HookAction> + Send {
        let bytes_out = result.len() as i64;
        // Tool results reach us in two shapes:
        //   1. Tool returned Err(_)   → Rig serializes via Display, producing
        //                                a plain string that starts with
        //                                "ToolCallError:" or "JsonError:".
        //   2. Tool returned Ok(json) → Rig serializes the JSON. Service
        //                                fns surface a soft error as a JSON
        //                                object with an "error" key (e.g.
        //                                ClickHouse 4xx wrapped to
        //                                {"error": "..."}).
        // Both should record `status = "error"`. Anything else is "ok".
        // Rig wraps a tool's `Err` as `"Toolset error: <ToolError display>"`
        // when bubbling up; our `ToolError` Display in turn prefixes
        // with `"ToolCallError: "`. So the literal result string can
        // begin with EITHER prefix depending on which layer surfaced
        // the failure — accept both, plus the JsonError variant for
        // arg-parse failures.
        let status = if result.starts_with("ToolCallError")
            || result.starts_with("JsonError")
            || result.starts_with("Toolset error")
            || result.contains("ToolCallError:")
        {
            "error"
        } else {
            match serde_json::from_str::<serde_json::Value>(result) {
                Ok(v) if v.get("error").is_some() => "error",
                _ => "ok",
            }
        };

        // When the call failed, keep a truncated copy of the message in the
        // `error` column so the prompt-detail UI can surface it without
        // having to scrape the model's reply.
        let error_msg: Option<String> = if status == "error" {
            let s: String = result.chars().take(500).collect();
            Some(s)
        } else {
            None
        };

        let response_preview = Some(truncate_preview(result, MAX_RESPONSE_PREVIEW_BYTES));
        let popped = self.pending.lock().ok().and_then(|mut g| g.remove(internal_call_id));
        let me = self.clone();
        let tool = tool_name.to_string();
        async move {
            if let Some(info) = popped {
                let duration_ms = info.t0.elapsed().as_millis() as i64;
                me.meter_tx.record(MeterEvent::ApiCall(ApiCallRow {
                    prompt_id: me.prompt_id.clone(),
                    tool_name: tool,
                    started_at_ms: info.started_at_ms,
                    duration_ms,
                    bytes_in: info.args_bytes,
                    bytes_out,
                    status: status.to_string(),
                    error: error_msg,
                    args_preview: info.args_preview,
                    response_preview,
                }));
                me.send(SseEvent::ToolCallFinished {
                    call_id: info.call_id,
                    duration_ms,
                    status: status.to_string(),
                    bytes_out,
                }).await;
            }
            HookAction::cont()
        }
    }

    fn on_text_delta(
        &self,
        text_delta: &str,
        _aggregated_text: &str,
    ) -> impl std::future::Future<Output = HookAction> + Send {
        let me = self.clone();
        let text = text_delta.to_string();
        async move {
            me.send(SseEvent::TextDelta { text }).await;
            HookAction::cont()
        }
    }
}
