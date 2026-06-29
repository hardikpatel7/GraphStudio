//! `GET /api/agent/models` — list enabled models from the allowlist. UI uses
//! this to populate the model picker on session creation.

use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::Value;

use crate::agent::routes::HttpError;
use crate::AppState;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, HttpError> {
    let rows = state
        .agent
        .db
        .query(
            "SELECT provider, model, display_name, backend FROM model_allowlist \
             WHERE enabled = 1 \
             ORDER BY provider, model",
            &[],
        )
        .map_err(HttpError::internal)?;
    Ok(Json(Value::Array(rows)))
}
