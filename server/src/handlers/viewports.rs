use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::{err, stringify};
use std::time::Instant;

pub async fn list(State(state): State<Arc<AppState>>, Path(dv_id): Path<String>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query(
        "SELECT * FROM viewports WHERE dataview_id = ?1 ORDER BY display_name",
        &[&dv_id as &dyn rusqlite::types::ToSql],
    ).map(|r| Json(Value::Array(r))).map_err(|e| err(500, &e.to_string()))
}

pub async fn create(State(state): State<Arc<AppState>>, Path(dv_id): Path<String>, Json(body): Json<Value>) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let name = body["display_name"].as_str().unwrap_or("");
    let filter_config_ref = body["filter_config_ref"].as_str().unwrap_or("");
    let filters = body.get("filters").map(stringify).unwrap_or_else(|| "{}".into());
    let sort = body.get("sort").map(stringify).unwrap_or_else(|| "{}".into());
    let page_size = body["page_size"].as_i64().unwrap_or(100);
    let role_filter = body.get("role_filter").map(stringify).unwrap_or_else(|| "{}".into());
    let config = body.get("config").map(stringify).unwrap_or_else(|| "{}".into());

    state.db.execute(
        "INSERT INTO viewports (id, dataview_id, display_name, filter_config_ref, filters, sort, page_size, role_filter, config) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        &[&id as &dyn rusqlite::types::ToSql, &dv_id as _, &name as _, &filter_config_ref as _, &filters as _, &sort as _, &page_size as _, &role_filter as _, &config as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM viewports WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql]).map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "viewport", "create", "success", &format!("Created viewport '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn update(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

    if let Some(v) = body.get("display_name").and_then(|v| v.as_str()) { sets.push("display_name = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("filter_config_ref").and_then(|v| v.as_str()) { sets.push("filter_config_ref = ?"); vals.push(Box::new(v.to_string())); }
    if body.get("filters").is_some() { sets.push("filters = ?"); vals.push(Box::new(stringify(&body["filters"]))); }
    if body.get("sort").is_some() { sets.push("sort = ?"); vals.push(Box::new(stringify(&body["sort"]))); }
    if let Some(v) = body["page_size"].as_i64() { sets.push("page_size = ?"); vals.push(Box::new(v)); }
    if body.get("role_filter").is_some() { sets.push("role_filter = ?"); vals.push(Box::new(stringify(&body["role_filter"]))); }
    if body.get("config").is_some() { sets.push("config = ?"); vals.push(Box::new(stringify(&body["config"]))); }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE viewports SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));
    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "ViewPort not found")); }

    let row = state.db.query_one("SELECT * FROM viewports WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "viewport", "update", "success", &format!("Updated viewport '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute("DELETE FROM viewports WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "ViewPort not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "viewport", "delete", "success", &format!("Deleted viewport '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}
