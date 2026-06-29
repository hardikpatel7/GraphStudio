use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::{err, stringify};
use std::time::Instant;

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query("SELECT * FROM templates ORDER BY display_name", &[])
        .map(|rows| Json(Value::Array(rows)))
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let display_name = body["display_name"].as_str().unwrap_or("");
    let description = body["description"].as_str().unwrap_or("");
    let app_snapshot = body.get("app_snapshot").map(stringify).unwrap_or_else(|| "{}".into());

    state.db.execute(
        "INSERT INTO templates (id, display_name, description, app_snapshot) VALUES (?1, ?2, ?3, ?4)",
        &[&id as &dyn rusqlite::types::ToSql, &display_name as _, &description as _, &app_snapshot as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM templates WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "template", "create", "success", &format!("Created template '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn clone(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let new_id = body["id"].as_str().unwrap_or("");
    let new_name = body.get("display_name").and_then(|v| v.as_str());

    // Fetch original
    let original = state.db.query_one(
        "SELECT * FROM templates WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "Template not found"))?;

    let display_name = new_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{} (copy)", original["display_name"].as_str().unwrap_or("")));
    let description = original["description"].as_str().unwrap_or("").to_string();
    let app_snapshot = stringify(&original["app_snapshot"]);

    state.db.execute(
        "INSERT INTO templates (id, display_name, description, app_snapshot) VALUES (?1, ?2, ?3, ?4)",
        &[&new_id as &dyn rusqlite::types::ToSql, &display_name as _, &description as _, &app_snapshot as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM templates WHERE id = ?1", &[&new_id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "template", "clone", "success", &format!("Cloned template '{}' → '{}'", id, new_id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}
