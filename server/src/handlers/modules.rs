use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::err;
use std::time::Instant;

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query("SELECT * FROM modules ORDER BY sort_order", &[])
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
    let route = body["route"].as_str().unwrap_or("");
    let icon = body["icon"].as_str().unwrap_or("");
    let permission_key = body["permission_key"].as_str().unwrap_or("");
    let sort_order = body["sort_order"].as_i64().unwrap_or(0).to_string();

    state.db.execute(
        "INSERT INTO modules (id, display_name, route, icon, permission_key, sort_order) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        &[&id as &dyn rusqlite::types::ToSql, &display_name as _, &route as _, &icon as _, &permission_key as _, &sort_order as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM modules WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "module", "create", "success", &format!("Created module '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

    if let Some(v) = body.get("display_name").and_then(|v| v.as_str()) { sets.push("display_name = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("route").and_then(|v| v.as_str()) { sets.push("route = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("icon").and_then(|v| v.as_str()) { sets.push("icon = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("permission_key").and_then(|v| v.as_str()) { sets.push("permission_key = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("sort_order").and_then(|v| v.as_i64()) { sets.push("sort_order = ?"); vals.push(Box::new(v.to_string())); }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE modules SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Module not found")); }

    let row = state.db.query_one("SELECT * FROM modules WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "module", "update", "success", &format!("Updated module '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute(
        "DELETE FROM modules WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Module not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "module", "delete", "success", &format!("Deleted module '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}
