use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::err;
use std::time::Instant;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query("SELECT * FROM saved_queries ORDER BY display_name", &[])
        .map(|r| Json(Value::Array(r)))
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn create(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let name = body["display_name"].as_str().unwrap_or("");
    let sql_text = body["sql_text"].as_str().unwrap_or("");
    let engine = body["engine"].as_str().unwrap_or("duckdb");
    let description = body["description"].as_str().unwrap_or("");

    if id.is_empty() || sql_text.is_empty() { return Err(err(400, "id and sql_text are required")); }

    state.db.execute(
        "INSERT OR REPLACE INTO saved_queries (id, display_name, sql_text, engine, description) VALUES (?1, ?2, ?3, ?4, ?5)",
        &[&id as &dyn rusqlite::types::ToSql, &name as _, &sql_text as _, &engine as _, &description as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM saved_queries WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql]).map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "saved_query", "create", "success", &format!("Created saved query '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn update(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();
    for col in &["display_name", "sql_text", "engine", "description"] {
        if let Some(v) = body.get(*col).and_then(|v| v.as_str()) { sets.push(format!("{} = ?", col)); vals.push(Box::new(v.to_string())); }
    }
    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')".to_string());
    let sql = format!("UPDATE saved_queries SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));
    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "not found")); }
    let row = state.db.query_one("SELECT * FROM saved_queries WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql]).map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "saved_query", "update", "success", &format!("Updated saved query '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute("DELETE FROM saved_queries WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql]).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "saved_query", "delete", "success", &format!("Deleted saved query '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}
