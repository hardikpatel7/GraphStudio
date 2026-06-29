use axum::{extract::State, Json, response::sse::{Event, Sse}};
use serde_json::{json, Value};
use std::sync::Arc;
use std::convert::Infallible;
use futures::stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use crate::AppState;
use super::err;

/// Get activity log for the running tenant. Supports category, time range, follow-up filters.
pub async fn get_activity(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let limit = body["limit"].as_i64().unwrap_or(50);
    let offset = body["offset"].as_i64().unwrap_or(0);
    let category = body["category"].as_str();
    let hours_ago = body["hours_ago"].as_i64();
    let follow_up_only = body["follow_up_only"].as_bool().unwrap_or(false);

    state.traces.get_activity(&state.tenant_id, limit, offset, category, hours_ago, follow_up_only)
        .map(Json)
        .map_err(|e| err(500, &e.to_string()))
}

/// Toggle follow-up flag on an activity item.
pub async fn toggle_follow_up(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let row_id = body["row_id"].as_i64().unwrap_or(0);
    if row_id == 0 { return Err(err(400, "row_id is required")); }

    state.traces.toggle_follow_up(&state.tenant_id, row_id)
        .map_err(|e| err(500, &e.to_string()))?;
    Ok(Json(json!({"success": true})))
}

pub async fn get_errors(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.traces.get_errors(&state.tenant_id, 100)
        .map(Json)
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn get_pipeline_runs(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.traces.get_pipeline_runs(&state.tenant_id, 100)
        .map(Json)
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn get_settings(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.traces.get_all_settings(&state.tenant_id)
        .map(Json)
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn set_setting(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let key = body["key"].as_str().unwrap_or("");
    let value = body["value"].as_str().unwrap_or("");
    if key.is_empty() { return Err(err(400, "key is required")); }

    state.traces.set_setting(&state.tenant_id, key, value)
        .map_err(|e| err(500, &e.to_string()))?;
    Ok(Json(json!({"success": true})))
}

/// SSE stream: real-time activity events for the running tenant.
pub async fn stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.traces.subscribe(&state.tenant_id);

    let stream = BroadcastStream::new(rx).map(|msg| -> Result<Event, Infallible> {
        match msg {
            Ok(val) => Ok(Event::default().data(val.to_string())),
            Err(_) => Ok(Event::default().comment("missed")),
        }
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping")
    )
}
