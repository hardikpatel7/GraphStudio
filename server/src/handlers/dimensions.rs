use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::{err, stringify};
use std::time::Instant;

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query(
        "SELECT * FROM dimensions ORDER BY display_name",
        &[],
    )
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
    let master_table = body["master_table"].as_str().unwrap_or("");
    let datasource_ref = body["datasource_ref"].as_str().unwrap_or("");
    let levels = body.get("levels").map(stringify).unwrap_or_else(|| "[]".into());
    let additional_filter_cols = body.get("additional_filter_cols").map(stringify).unwrap_or_else(|| "[]".into());

    state.db.execute(
        "INSERT INTO dimensions (id, display_name, master_table, datasource_ref, levels, additional_filter_cols) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        &[&id as &dyn rusqlite::types::ToSql, &display_name as _, &master_table as _, &datasource_ref as _, &levels as _, &additional_filter_cols as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM dimensions WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "dimension", "create", "success", &format!("Created dimension '{}'", id), None, Some(elapsed));
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
    if let Some(v) = body.get("master_table").and_then(|v| v.as_str()) { sets.push("master_table = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("datasource_ref").and_then(|v| v.as_str()) { sets.push("datasource_ref = ?"); vals.push(Box::new(v.to_string())); }
    if body.get("levels").is_some() { sets.push("levels = ?"); vals.push(Box::new(stringify(&body["levels"]))); }
    if body.get("additional_filter_cols").is_some() { sets.push("additional_filter_cols = ?"); vals.push(Box::new(stringify(&body["additional_filter_cols"]))); }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE dimensions SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Dimension not found")); }

    let row = state.db.query_one("SELECT * FROM dimensions WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "dimension", "update", "success", &format!("Updated dimension '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute(
        "DELETE FROM dimensions WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Dimension not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "dimension", "delete", "success", &format!("Deleted dimension '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}
