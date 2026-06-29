//! Prompt routes — submit + SSE stream, list, detail.
//!
//! `POST /api/agent/sessions/:id/prompts` is the streaming entry point. The
//! handler:
//!
//! 1. Validates the session + workspace + model.
//! 2. Inserts a `prompt` row (status = streaming).
//! 3. Builds a `LlmRunner` from the model's `backend` and a `RunnerInput`
//!    carrying the SSE sender + meter handle.
//! 4. Spawns the run in a tokio task. The task pipes
//!    `ToolCallStarted/Finished` + `TextDelta` events through the SSE
//!    sender, then on completion records `llm_usage`, updates the prompt
//!    row, and pushes `Usage` + `TurnFinished` SSE events.
//! 5. Returns an `Sse<...>` body that drains the mpsc receiver until the
//!    task closes the channel.

use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use chrono::Utc;
use futures::stream::Stream;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::agent::llm::{self, Backend, ModelEntry, RunnerInput};
use crate::agent::meter::hook::SseEvent as InternalSse;
use crate::agent::meter::writer::{LlmUsageRow, MeterEvent};
use crate::agent::routes::HttpError;
use crate::agent::tools::WorkspaceKind;
use crate::AppState;

// ── list / detail ────────────────────────────────────────────────────────

pub async fn list_for_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let mut rows = state.agent.db.query(
        "SELECT p.*, u.tokens_in, u.tokens_out, u.latency_ms \
         FROM prompt p LEFT JOIN llm_usage u ON u.prompt_id = p.id \
         WHERE p.session_id = ? \
         ORDER BY p.started_at",
        &[&session_id],
    ).map_err(HttpError::internal)?;
    // Derive cost per prompt server-side so the UI can show it inline on
    // the thread without N round-trips. `prompt_cost_usd` is cheap — joins
    // api_call/llm_usage to the latest pricing_config row.
    for row in rows.iter_mut() {
        let id = row.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if id.is_empty() { continue; }
        let cost = crate::agent::meter::pricing::prompt_cost_usd(&state.agent.db, &id)
            .ok()
            .flatten();
        if let Some(obj) = row.as_object_mut() {
            obj.insert("cost_usd".into(), serde_json::json!(cost));
        }
    }
    Ok(Json(Value::Array(rows)))
}

pub async fn detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let prompt = state.agent.db.query_one(
        "SELECT * FROM prompt WHERE id = ?",
        &[&id],
    ).map_err(|_| HttpError::not_found("prompt not found"))?;
    let usage = state
        .agent
        .db
        .query("SELECT * FROM llm_usage WHERE prompt_id = ?", &[&id])
        .map_err(HttpError::internal)?
        .into_iter()
        .next();
    let api_calls = state
        .agent
        .db
        .query(
            "SELECT id, tool_name, started_at, duration_ms, bytes_in, bytes_out, status, error, \
                    args_preview, response_preview \
             FROM api_call WHERE prompt_id = ? ORDER BY started_at",
            &[&id],
        )
        .map_err(HttpError::internal)?;
    let cost = crate::agent::meter::pricing::prompt_cost_usd(&state.agent.db, &id)
        .ok()
        .flatten();
    let cost_breakdown = crate::agent::meter::pricing::prompt_cost_breakdown(&state.agent.db, &id)
        .ok()
        .flatten();
    Ok(Json(json!({
        "prompt":   prompt,
        "usage":    usage,
        "api_calls": api_calls,
        "cost_usd": cost,
        "cost_breakdown": cost_breakdown,
    })))
}

// ── submit (SSE) ─────────────────────────────────────────────────────────

pub async fn submit(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, HttpError> {
    let user_text = body
        .get("user_text")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| HttpError::bad_request("`user_text` is required"))?
        .to_string();

    // Load session → workspace → model. Each lookup is its own DB call so we
    // can give a precise 404 message; the volumes here are tiny.
    let session = state
        .agent
        .db
        .query_one("SELECT * FROM session WHERE id = ?", &[&session_id])
        .map_err(|_| HttpError::not_found("session not found"))?;
    let workspace_id = session
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let model = session
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let workspace = state
        .agent
        .db
        .query_one("SELECT * FROM workspace WHERE id = ?", &[&workspace_id])
        .map_err(|_| HttpError::not_found("workspace not found"))?;
    let kind_str = workspace
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("inventory");
    let workspace_kind = WorkspaceKind::from_str(kind_str)
        .ok_or_else(|| HttpError::internal(format!("unknown workspace kind {kind_str}")))?;
    // A workspace kind is "wired" if `workspace_kind_tools` has rows for it.
    // Empty mapping → reject the prompt with a clear message so the UI can
    // surface the "Backend not yet configured" state without guessing.
    let tool_row = state
        .agent
        .db
        .query(
            "SELECT COUNT(*) AS n FROM workspace_kind_tools WHERE kind = ?",
            &[&workspace_kind.as_str()],
        )
        .map_err(HttpError::internal)?;
    let tool_count = tool_row
        .first()
        .and_then(|v| v.get("n"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if tool_count == 0 {
        return Err(HttpError::bad_request(
            "Backend not yet configured for this workspace kind",
        ));
    }
    let allowlist_row = state
        .agent
        .db
        .query_one(
            "SELECT provider, model, display_name, backend FROM model_allowlist \
             WHERE model = ? AND enabled = 1",
            &[&model],
        )
        .map_err(|_| HttpError::bad_request(format!("model `{model}` not enabled")))?;
    let model_entry = ModelEntry {
        provider: allowlist_row.get("provider").and_then(|v| v.as_str()).unwrap_or("openai").to_string(),
        model: allowlist_row.get("model").and_then(|v| v.as_str()).unwrap_or(&model).to_string(),
        display_name: allowlist_row.get("display_name").and_then(|v| v.as_str()).unwrap_or(&model).to_string(),
        backend: Backend::from_str(allowlist_row.get("backend").and_then(|v| v.as_str()).unwrap_or("rig")),
    };

    // Insert the prompt row. Pending status flips to `done` / `error` from
    // the spawned task; if the server dies mid-run the row stays
    // `streaming` and a future scrub can mark it stale.
    let prompt_id = format!("pmt_{}", Uuid::new_v4().simple());
    let now = Utc::now().timestamp_millis();
    state
        .agent
        .db
        .execute(
            "INSERT INTO prompt (id, session_id, parent_prompt_id, user_text, model, status, started_at) \
             VALUES (?, ?, NULL, ?, ?, 'streaming', ?)",
            &[&prompt_id, &session_id, &user_text, &model_entry.model, &now],
        )
        .map_err(HttpError::internal)?;
    state
        .agent
        .db
        .execute(
            "UPDATE session SET last_active_at = ? WHERE id = ?",
            &[&now, &session_id],
        )
        .map_err(HttpError::internal)?;

    // Schema hint. Populated by `sessions::create`'s background pre-warm.
    // If the row's still NULL (first prompt before pre-warm finished), run
    // discovery inline and write it back so subsequent prompts get the
    // cached version.
    let cached_hint: Option<String> = session
        .get("schema_hint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());
    let schema_hint = match cached_hint {
        Some(h) => h,
        None => {
            let h = crate::agent::schema::discover(state.clone(), workspace_kind).await;
            if !h.is_empty() {
                let _ = state.agent.db.execute(
                    "UPDATE session SET schema_hint = ? WHERE id = ?",
                    &[&h, &session_id],
                );
            }
            h
        }
    };

    // Channel that the metering hook + the runner write to; the SSE stream
    // drains the receiver. Bounded buffer so a slow client back-pressures
    // the model rather than blowing memory.
    let (sse_tx, sse_rx) = mpsc::channel::<InternalSse>(128);
    // A second channel for the route-side "turn complete" / "error" events
    // emitted after the runner returns. We collapse both into a single
    // output stream by selecting from both.
    let (post_tx, post_rx) = mpsc::channel::<RouteSse>(8);

    let state_run = state.clone();
    let prompt_id_run = prompt_id.clone();
    let model_run = model_entry.clone();
    let user_text_run = user_text.clone();
    let schema_hint_run = schema_hint.clone();
    tokio::spawn(async move {
        let backend = model_run.backend.clone();
        let runner = llm::build_runner(&backend);
        let input = RunnerInput {
            state: state_run.clone(),
            workspace_kind,
            model: model_run.clone(),
            prompt_id: prompt_id_run.clone(),
            sse_tx: sse_tx.clone(),
            meter_tx: state_run.agent.meter_tx.clone(),
            schema_hint: schema_hint_run,
            // Chat path doesn't need a per-call output contract.
            addendum: String::new(),
        };
        let finished_at = match runner.run_turn(input, &user_text_run).await {
            Ok(summary) => {
                state_run.agent.meter_tx.record(MeterEvent::LlmUsage(LlmUsageRow {
                    prompt_id: prompt_id_run.clone(),
                    model: model_run.model.clone(),
                    tokens_in: summary.tokens_in,
                    tokens_out: summary.tokens_out,
                    latency_ms: summary.latency_ms,
                }));
                let finished = Utc::now().timestamp_millis();
                let _ = state_run.agent.db.execute(
                    "UPDATE prompt SET status = 'done', response_text = ?, finished_at = ? WHERE id = ?",
                    &[&summary.final_text, &finished, &prompt_id_run],
                );
                let _ = post_tx
                    .send(RouteSse::Usage {
                        tokens_in: summary.tokens_in,
                        tokens_out: summary.tokens_out,
                    })
                    .await;
                let _ = post_tx
                    .send(RouteSse::TurnFinished {
                        prompt_id: prompt_id_run.clone(),
                        final_text: summary.final_text,
                        latency_ms: summary.latency_ms,
                    })
                    .await;
                finished
            }
            Err(e) => {
                let finished = Utc::now().timestamp_millis();
                let err_msg = e.to_string();
                let _ = state_run.agent.db.execute(
                    "UPDATE prompt SET status = 'error', error = ?, finished_at = ? WHERE id = ?",
                    &[&err_msg, &finished, &prompt_id_run],
                );
                let _ = post_tx
                    .send(RouteSse::Error { message: err_msg })
                    .await;
                finished
            }
        };
        let _ = finished_at;
        drop(sse_tx);
        drop(post_tx);
    });

    // Drain both channels into a single SSE stream. `turn_started` fires
    // immediately so the UI knows we accepted the prompt.
    let header = futures::stream::once(async move {
        Ok(Event::default().json_data(json!({
            "type": "turn_started",
            "prompt_id": prompt_id,
            "model": model_entry.model,
        })).unwrap())
    });
    let internal = ReceiverStream::new(sse_rx).map(|ev| {
        let v = match ev {
            InternalSse::TextDelta { text } => json!({ "type": "text_delta", "text": text }),
            InternalSse::ToolCallStarted { call_id, tool, args_preview } => json!({
                "type": "tool_call_started",
                "call_id": call_id, "tool": tool, "args_preview": args_preview,
            }),
            InternalSse::ToolCallFinished { call_id, duration_ms, status, bytes_out } => json!({
                "type": "tool_call_finished",
                "call_id": call_id, "duration_ms": duration_ms, "status": status, "bytes_out": bytes_out,
            }),
        };
        Ok::<Event, Infallible>(Event::default().json_data(v).unwrap())
    });
    let post = ReceiverStream::new(post_rx).map(|ev| {
        let v = match ev {
            RouteSse::Usage { tokens_in, tokens_out } => json!({
                "type": "usage", "tokens_in": tokens_in, "tokens_out": tokens_out,
            }),
            RouteSse::TurnFinished { prompt_id, final_text, latency_ms } => json!({
                "type": "turn_finished",
                "latency_ms": latency_ms,
                "prompt_id": prompt_id, "final_text": final_text,
            }),
            RouteSse::Error { message } => json!({
                "type": "error", "message": message, "retriable": false,
            }),
        };
        Ok::<Event, Infallible>(Event::default().json_data(v).unwrap())
    });
    let body = header.chain(internal).chain(post);
    Ok(Sse::new(body).keep_alive(KeepAlive::default()))
}

enum RouteSse {
    Usage { tokens_in: i64, tokens_out: i64 },
    TurnFinished { prompt_id: String, final_text: String, latency_ms: i64 },
    Error { message: String },
}
