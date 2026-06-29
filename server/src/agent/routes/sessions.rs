//! Session routes. A session is a conversation thread inside a workspace —
//! its `model`, `provider_state` (provider-opaque continuation), and title
//! live here. Pre-warm on create is a fire-and-forget log line for v1; the
//! Rig path doesn't need an explicit warm step.

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

pub async fn list_for_workspace(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    // Derived `kind`: a session is `"dashboard"` when a dashboard
    // row points at it; otherwise `"chat"`. This lets the UI render
    // user chat sessions and dashboard-backed synthetic sessions as
    // two separate lists without a schema migration. Previously
    // both were mixed in one list and the synthetic ones (titled
    // `<dashboard> · widgets`) felt like noise.
    //
    // The preview-session for components (`title = '_component_preview · widgets'`)
    // is also marked as `dashboard` so the chat list stays clean.
    let rows = state.agent.db.query(
        "SELECT s.*, \
                (SELECT COUNT(*) FROM prompt p WHERE p.session_id = s.id) AS prompt_count, \
                CASE \
                  WHEN EXISTS (SELECT 1 FROM dashboard d WHERE d.session_id = s.id) THEN 'dashboard' \
                  WHEN s.title LIKE '\\_component\\_preview%' ESCAPE '\\' THEN 'dashboard' \
                  ELSE 'chat' \
                END AS kind \
         FROM session s WHERE s.workspace_id = ? \
         ORDER BY s.last_active_at DESC",
        &[&workspace_id],
    ).map_err(HttpError::internal)?;
    Ok(Json(Value::Array(rows)))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpError> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HttpError::bad_request("`model` is required"))?;
    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Untitled session".to_string());

    // Look up the model in the allowlist to learn provider + backend.
    let m = state
        .agent
        .db
        .query_one(
            "SELECT provider, model, backend FROM model_allowlist \
             WHERE model = ? AND enabled = 1",
            &[&model],
        )
        .map_err(|_| HttpError::bad_request(format!("model `{model}` not in allowlist")))?;
    let provider = m
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("openai");

    // Workspace must exist. We also pull its `kind` so we can kick off the
    // schema pre-warm below.
    let ws = state
        .agent
        .db
        .query_one(
            "SELECT id, kind FROM workspace WHERE id = ?",
            &[&workspace_id],
        )
        .map_err(|_| HttpError::not_found("workspace not found"))?;

    let id = format!("sess_{}", Uuid::new_v4().simple());
    let now = Utc::now().timestamp_millis();
    state.agent.db.execute(
        "INSERT INTO session (id, workspace_id, provider, model, title, provider_state, created_at, last_active_at) \
         VALUES (?, ?, ?, ?, ?, NULL, ?, ?)",
        &[&id, &workspace_id, &provider, &model, &title, &now, &now],
    ).map_err(HttpError::internal)?;

    // Pre-warm: kick off schema discovery in the background. Writes the
    // result into `session.schema_hint`. The prompts/submit route reads
    // this; if the row's still NULL when the first prompt arrives, that
    // route falls back to running discovery inline.
    let kind_str = ws
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if let Some(kind) = crate::agent::tools::WorkspaceKind::from_str(&kind_str) {
        let state_disc = state.clone();
        let sid = id.clone();
        tokio::spawn(async move {
            let hint = crate::agent::schema::discover(state_disc.clone(), kind).await;
            if hint.is_empty() { return; }
            if let Err(e) = state_disc.agent.db.execute(
                "UPDATE session SET schema_hint = ? WHERE id = ?",
                &[&hint, &sid],
            ) {
                tracing::warn!(error = %e, session = %sid, "[agent] pre-warm schema_hint write failed");
            } else {
                tracing::info!(
                    session = %sid,
                    bytes = hint.len(),
                    "[agent] schema_hint pre-warmed"
                );
            }
        });
    }

    Ok(Json(json!({
        "id": id,
        "workspace_id": workspace_id,
        "provider": provider,
        "model": model,
        "title": title,
        "created_at": now,
    })))
}

/// `PATCH /api/agent/sessions/{id}` — partial update. v1 supports only
/// `title` since that's the only user-editable field. Sends back the updated
/// row so the caller can re-render without a follow-up GET.
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpError> {
    use rusqlite::types::ToSql;

    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if title.is_none() && model.is_none() {
        return Err(HttpError::bad_request(
            "at least one of `title` or `model` must be a non-empty string",
        ));
    }

    // Validate model against allowlist before writing — keep the
    // contract consistent with the dashboard PATCH route.
    if let Some(m) = model.as_ref() {
        let allowed = state.agent.db.query_one(
            "SELECT 1 AS ok FROM model_allowlist WHERE model = ? AND enabled = 1",
            &[&m],
        );
        if allowed.is_err() {
            return Err(HttpError::bad_request(format!(
                "model `{m}` is not enabled in the allowlist"
            )));
        }
    }

    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(t) = title {
        sets.push("title = ?");
        vals.push(Box::new(t));
    }
    if let Some(m) = model {
        sets.push("model = ?");
        vals.push(Box::new(m));
    }
    // Touch last_active_at so the sidebar order reflects the edit.
    let now = chrono::Utc::now().timestamp_millis();
    sets.push("last_active_at = ?");
    vals.push(Box::new(now));

    let sql = format!("UPDATE session SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));
    let refs: Vec<&dyn ToSql> = vals.iter().map(|b| b.as_ref()).collect();
    let n = state
        .agent
        .db
        .execute(&sql, &refs)
        .map_err(HttpError::internal)?;
    if n == 0 {
        return Err(HttpError::not_found("session not found"));
    }

    let row = state
        .agent
        .db
        .query_one(
            "SELECT * FROM session WHERE id = ?",
            &[&id],
        )
        .map_err(HttpError::internal)?;
    Ok(Json(row))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let row = state
        .agent
        .db
        .query_one(
            "SELECT * FROM session WHERE id = ?",
            &[&id],
        )
        .map_err(|_| HttpError::not_found("session not found"))?;
    Ok(Json(row))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    // SQLite foreign keys are ON, so we have to clear dependent rows
    // before the session row itself. Order matters: response_chunk /
    // api_call / llm_usage all reference prompt; prompt references
    // session. Cheap per-table DELETEs scoped through the session's
    // prompt ids.
    let db = &state.agent.db;
    let prompt_id_subselect = "(SELECT id FROM prompt WHERE session_id = ?)";

    db.execute(
        &format!("DELETE FROM response_chunk WHERE prompt_id IN {prompt_id_subselect}"),
        &[&id],
    ).map_err(HttpError::internal)?;
    db.execute(
        &format!("DELETE FROM api_call WHERE prompt_id IN {prompt_id_subselect}"),
        &[&id],
    ).map_err(HttpError::internal)?;
    db.execute(
        &format!("DELETE FROM llm_usage WHERE prompt_id IN {prompt_id_subselect}"),
        &[&id],
    ).map_err(HttpError::internal)?;
    db.execute(
        "DELETE FROM prompt WHERE session_id = ?",
        &[&id],
    ).map_err(HttpError::internal)?;

    let n = db
        .execute("DELETE FROM session WHERE id = ?", &[&id])
        .map_err(HttpError::internal)?;
    if n == 0 {
        return Err(HttpError::not_found("session not found"));
    }
    Ok(Json(json!({ "deleted": true })))
}
