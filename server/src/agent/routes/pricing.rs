//! `GET /api/agent/pricing` — current weights row.
//! `PATCH /api/agent/pricing` — insert a new versioned row.

use std::sync::Arc;

use axum::{extract::State, Json};
use chrono::Utc;
use serde_json::Value;

use crate::agent::routes::HttpError;
use crate::AppState;

pub async fn get_current(State(state): State<Arc<AppState>>) -> Result<Json<Value>, HttpError> {
    let row = state
        .agent
        .db
        .query_one(
            "SELECT id, effective_from, weights, notes FROM pricing_config \
             ORDER BY effective_from DESC LIMIT 1",
            &[],
        )
        .map_err(|_| HttpError::not_found("no pricing_config row yet"))?;
    Ok(Json(row))
}

pub async fn patch(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, HttpError> {
    // Required: `weights` (JSON object). Optional: `effective_from` (ms epoch,
    // defaults to now), `notes`. We always *insert* — never update an
    // existing row — so re-pricing is non-destructive and historical prompts
    // reprice retroactively on next read.
    let weights = body
        .get("weights")
        .ok_or_else(|| HttpError::bad_request("`weights` JSON object is required"))?
        .clone();
    if !weights.is_object() {
        return Err(HttpError::bad_request("`weights` must be a JSON object"));
    }
    let effective_from = body
        .get("effective_from")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    let notes = body
        .get("notes")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    state
        .agent
        .db
        .execute(
            "INSERT INTO pricing_config (effective_from, weights, notes) VALUES (?, ?, ?)",
            &[&effective_from, &weights.to_string(), &notes],
        )
        .map_err(HttpError::internal)?;
    // Return the new latest row.
    get_current(State(state)).await
}
