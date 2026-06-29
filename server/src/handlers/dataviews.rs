use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::{service, AppState};
use std::time::Instant;
use super::{err, stringify};

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    service::dataviews::list(&state)
        .await
        .map(|rows| Json(Value::Array(rows)))
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    service::dataviews::describe(&state, &id)
        .await
        .map(Json)
        .map_err(|_| err(404, "DataView not found"))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let display_name = body["display_name"].as_str().unwrap_or("");
    let contract = body.get("contract").map(stringify).unwrap_or_else(|| "{}".into());
    let dimensions = body.get("dimensions").map(stringify).unwrap_or_else(|| "[]".into());
    let columns = body.get("columns").map(stringify).unwrap_or_else(|| "[]".into());
    let sort = body.get("sort").map(stringify).unwrap_or_else(|| "[]".into());
    let backend_workflow = body.get("backend_workflow").map(stringify).unwrap_or_else(|| "[]".into());
    let cascading_filters = body.get("cascading_filters").map(stringify).unwrap_or_else(|| "[]".into());

    // Source binding. Two paths:
    //   1. Caller passed `source: {"type":"source","config":{"source_id":...}}`
    //      → use that binding directly (caller already created the Source).
    //   2. No (valid) source in body → auto-create a placeholder
    //      `pg_query` Source and bind the DV to it. The user picks the
    //      real kind/config on the Schema tab afterwards.
    //
    // Legacy inline shapes in the request body (`{type:"pg_query",config:...}`)
    // are not honored — the caller must promote to a Source row first.
    let provided_source_id = body
        .get("source")
        .and_then(|s| s.get("type").and_then(|t| t.as_str()).filter(|t| *t == "source").map(|_| s))
        .and_then(|s| s.get("config"))
        .and_then(|c| c.get("source_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let source = if let Some(sid) = provided_source_id {
        // Verify the caller's source exists; surface a clean 400 if not.
        let exists = state.db.query_one(
            "SELECT id FROM sources WHERE id = ?1",
            &[&sid as &dyn rusqlite::types::ToSql],
        ).is_ok();
        if !exists {
            return Err(err(400, &format!("source_id '{sid}' does not exist")));
        }
        format!(r#"{{"type":"source","config":{{"source_id":"{sid}","output":null}}}}"#)
    } else {
        let synthetic_source_id = format!("src_dv_{id}");
        state.db.execute(
            "INSERT OR IGNORE INTO sources \
                (id, display_name, kind, config, status) \
             VALUES (?1, ?2, 'pg_query', '{}', 'not_yet_populated')",
            &[
                &synthetic_source_id as &dyn rusqlite::types::ToSql,
                &format!("{display_name} (DV-derived)") as _,
            ],
        ).map_err(|e| err(500, &format!("auto-create Source: {e}")))?;
        format!(r#"{{"type":"source","config":{{"source_id":"{synthetic_source_id}","output":null}}}}"#)
    };

    state.db.execute(
        "INSERT INTO dataviews (id, display_name, contract, dimensions, columns, sort, backend_workflow, cascading_filters, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        &[&id as &dyn rusqlite::types::ToSql, &display_name as _, &contract as _, &dimensions as _, &columns as _, &sort as _, &backend_workflow as _, &cascading_filters as _, &source as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM dataviews WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "dataview", "create", "success", &format!("Created DataView '{}'", id), None, Some(elapsed));
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
    if body.get("contract").is_some() { sets.push("contract = ?"); vals.push(Box::new(stringify(&body["contract"]))); }
    if body.get("dimensions").is_some() { sets.push("dimensions = ?"); vals.push(Box::new(stringify(&body["dimensions"]))); }
    if body.get("columns").is_some() { sets.push("columns = ?"); vals.push(Box::new(stringify(&body["columns"]))); }
    if body.get("sort").is_some() { sets.push("sort = ?"); vals.push(Box::new(stringify(&body["sort"]))); }
    if body.get("backend_workflow").is_some() { sets.push("backend_workflow = ?"); vals.push(Box::new(stringify(&body["backend_workflow"]))); }
    if body.get("cascading_filters").is_some() { sets.push("cascading_filters = ?"); vals.push(Box::new(stringify(&body["cascading_filters"]))); }
    if body.get("source").is_some() { sets.push("source = ?"); vals.push(Box::new(stringify(&body["source"]))); }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE dataviews SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "DataView not found")); }

    let row = state.db.query_one("SELECT * FROM dataviews WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "dataview", "update", "success", &format!("Updated DataView '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute(
        "DELETE FROM dataviews WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "DataView not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "dataview", "delete", "success", &format!("Deleted DataView '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}
