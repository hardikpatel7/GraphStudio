use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::{err, stringify};
use std::time::Instant;

pub async fn list(
    State(state): State<Arc<AppState>>,
    Path(sub_id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query(
        "SELECT * FROM components WHERE submodule_id = ?1 ORDER BY sort_order",
        &[&sub_id as &dyn rusqlite::types::ToSql],
    )
    .map(|rows| Json(Value::Array(rows)))
    .map_err(|e| err(500, &e.to_string()))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Path(sub_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let display_name = body["display_name"].as_str().unwrap_or("");
    let tab_label = body["tab_label"].as_str().unwrap_or("");
    let dataview_refs = body.get("dataview_refs").map(stringify).unwrap_or_else(|| "[]".into());
    let primary_dataview = body["primary_dataview"].as_str().unwrap_or("");
    let config = body.get("config").map(stringify).unwrap_or_else(|| "{}".into());
    let sort_order = body["sort_order"].as_i64().unwrap_or(0).to_string();

    state.db.execute(
        "INSERT INTO components (id, submodule_id, display_name, tab_label, dataview_refs, primary_dataview, config, sort_order) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        &[&id as &dyn rusqlite::types::ToSql, &sub_id as _, &display_name as _, &tab_label as _, &dataview_refs as _, &primary_dataview as _, &config as _, &sort_order as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM components WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "component", "create", "success", &format!("Created component '{}'", id), None, Some(elapsed));
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
    if let Some(v) = body.get("tab_label").and_then(|v| v.as_str()) { sets.push("tab_label = ?"); vals.push(Box::new(v.to_string())); }
    if body.get("dataview_refs").is_some() { sets.push("dataview_refs = ?"); vals.push(Box::new(stringify(&body["dataview_refs"]))); }
    if let Some(v) = body.get("primary_dataview").and_then(|v| v.as_str()) { sets.push("primary_dataview = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("submodule_id").and_then(|v| v.as_str()) { sets.push("submodule_id = ?"); vals.push(Box::new(v.to_string())); }
    if body.get("config").is_some() { sets.push("config = ?"); vals.push(Box::new(stringify(&body["config"]))); }
    if let Some(v) = body.get("sort_order").and_then(|v| v.as_i64()) { sets.push("sort_order = ?"); vals.push(Box::new(v.to_string())); }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE components SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Component not found")); }

    let row = state.db.query_one("SELECT * FROM components WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "component", "update", "success", &format!("Updated component '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute(
        "DELETE FROM components WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Component not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "component", "delete", "success", &format!("Deleted component '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}
