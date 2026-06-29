//! Workspace routes. v1 ships with the 5 kinds auto-seeded by
//! `agent::config::seed_workspaces`; create is exposed for future
//! multi-workspace-per-kind scenarios.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::routes::HttpError;
use crate::AppState;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, HttpError> {
    // `tool_count` is the size of the kind's entry in `workspace_kind_tools`.
    // The UI uses it to decide which workspaces are "wired" (any tools)
    // vs. "pending" (zero tools) — replaces the previous hardcoded
    // `kind == 'inventory'` check.
    let rows = state.agent.db.query(
        "SELECT w.id, w.kind, w.name, w.config_json, w.created_at, \
                (SELECT COUNT(*) FROM session s WHERE s.workspace_id = w.id)               AS session_count, \
                (SELECT COUNT(*) FROM workspace_kind_tools t WHERE t.kind = w.kind)        AS tool_count \
         FROM workspace w \
         ORDER BY CASE w.kind \
            WHEN 'inventory' THEN 0 \
            WHEN 'item'      THEN 1 \
            WHEN 'pricing'   THEN 2 \
            WHEN 'assort'    THEN 3 \
            WHEN 'plan'      THEN 4 \
            ELSE 5 END",
        &[],
    ).map_err(HttpError::internal)?;
    Ok(Json(Value::Array(rows)))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let row = state.agent.db.query_one(
        "SELECT * FROM workspace WHERE id = ?",
        &[&id],
    ).map_err(|_| HttpError::not_found("workspace not found"))?;
    Ok(Json(row))
}

/// `GET /api/agent/workspaces/{id}/stats` — rollup metrics for the workspace
/// dashboard. Computes prompt status breakdown, total cost, tool-call counts,
/// token totals, average latency, and a top-tools chart. Cost is derived
/// via the pricing engine (joins api_call/llm_usage against the latest
/// pricing_config row).
pub async fn stats(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let db = &state.agent.db;

    // Workspace must exist (cheap 404 instead of returning empty).
    db.query_one("SELECT id FROM workspace WHERE id = ?", &[&workspace_id])
        .map_err(|_| HttpError::not_found("workspace not found"))?;

    // Sessions total.
    let sessions_total: i64 = db
        .query_one(
            "SELECT COUNT(*) AS n FROM session WHERE workspace_id = ?",
            &[&workspace_id],
        )
        .ok()
        .and_then(|v| v.get("n").and_then(|v| v.as_i64()))
        .unwrap_or(0);

    // Prompt status breakdown.
    let status_rows = db
        .query(
            "SELECT p.status AS status, COUNT(*) AS n \
             FROM prompt p JOIN session s ON s.id = p.session_id \
             WHERE s.workspace_id = ? \
             GROUP BY p.status",
            &[&workspace_id],
        )
        .unwrap_or_default();
    let mut p_total = 0i64;
    let mut p_done  = 0i64;
    let mut p_err   = 0i64;
    let mut p_run   = 0i64;
    for r in &status_rows {
        let n = r.get("n").and_then(|v| v.as_i64()).unwrap_or(0);
        let s = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
        p_total += n;
        match s {
            "done"      => p_done = n,
            "error"     => p_err  = n,
            "streaming" => p_run  = n,
            _ => {}
        }
    }

    // Token + latency totals from llm_usage.
    let usage_row = db
        .query_one(
            "SELECT COALESCE(SUM(u.tokens_in), 0)  AS tokens_in_total, \
                    COALESCE(SUM(u.tokens_out), 0) AS tokens_out_total, \
                    COALESCE(CAST(AVG(u.latency_ms) AS INTEGER), 0) AS avg_latency_ms \
             FROM llm_usage u \
             JOIN prompt p  ON p.id = u.prompt_id \
             JOIN session s ON s.id = p.session_id \
             WHERE s.workspace_id = ?",
            &[&workspace_id],
        )
        .unwrap_or_else(|_| json!({ "tokens_in_total": 0, "tokens_out_total": 0, "avg_latency_ms": 0 }));

    // API call totals, split by outcome.
    let calls_row = db
        .query_one(
            "SELECT COUNT(*) AS total, \
                    COALESCE(SUM(CASE WHEN status = 'error' OR status = 'timeout' THEN 1 ELSE 0 END), 0) AS errors, \
                    COALESCE(SUM(CASE WHEN status = 'cache_hit' THEN 1 ELSE 0 END), 0) AS cache_hits \
             FROM api_call a \
             JOIN prompt p  ON p.id = a.prompt_id \
             JOIN session s ON s.id = p.session_id \
             WHERE s.workspace_id = ?",
            &[&workspace_id],
        )
        .unwrap_or_else(|_| json!({ "total": 0, "errors": 0, "cache_hits": 0 }));

    // Top tools by call count.
    let top_tools = db
        .query(
            "SELECT a.tool_name AS tool, COUNT(*) AS count \
             FROM api_call a \
             JOIN prompt p  ON p.id = a.prompt_id \
             JOIN session s ON s.id = p.session_id \
             WHERE s.workspace_id = ? \
             GROUP BY a.tool_name \
             ORDER BY count DESC \
             LIMIT 6",
            &[&workspace_id],
        )
        .unwrap_or_default();

    // Total cost: sum the per-prompt cost over every prompt in this
    // workspace. Each row's cost is derived live against the current
    // `pricing_config`. For a large tenant this could become hot; for now
    // it's bounded by prompt count.
    let prompt_ids = db
        .query(
            "SELECT p.id FROM prompt p JOIN session s ON s.id = p.session_id \
             WHERE s.workspace_id = ?",
            &[&workspace_id],
        )
        .unwrap_or_default();
    let mut cost_total = 0.0f64;
    for row in &prompt_ids {
        if let Some(pid) = row.get("id").and_then(|v| v.as_str()) {
            if let Ok(Some(c)) = crate::agent::meter::pricing::prompt_cost_usd(db, pid) {
                cost_total += c;
            }
        }
    }

    let avg_cost = if p_total > 0 { cost_total / p_total as f64 } else { 0.0 };

    Ok(Json(json!({
        "sessions_total": sessions_total,
        "prompts": {
            "total":     p_total,
            "done":      p_done,
            "errored":   p_err,
            "streaming": p_run,
        },
        "api_calls":       calls_row,
        "tokens":          usage_row,
        "cost_usd_total":  cost_total,
        "cost_usd_avg":    avg_cost,
        "top_tools":       top_tools,
    })))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpError> {
    let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    if !["inventory", "item", "pricing", "assort", "plan"].contains(&kind) {
        return Err(HttpError::bad_request("kind must be one of inventory|item|pricing|assort|plan"));
    }
    if name.is_empty() {
        return Err(HttpError::bad_request("name is required"));
    }
    let id = format!("ws_{}", Uuid::new_v4().simple());
    let now = Utc::now().timestamp_millis();
    state.agent.db.execute(
        "INSERT INTO workspace (id, kind, name, config_json, created_at) \
         VALUES (?, ?, ?, '{}', ?)",
        &[&id, &kind, &name, &now],
    ).map_err(HttpError::internal)?;
    Ok(Json(json!({ "id": id, "kind": kind, "name": name, "created_at": now })))
}
