use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::{err, stringify};
use std::time::Instant;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query("SELECT * FROM derived_tables ORDER BY display_name", &[])
        .map(|r| Json(Value::Array(r)))
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn get_one(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query_one("SELECT * FROM derived_tables WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map(Json)
        .map_err(|_| err(404, "Derived table not found"))
}

pub async fn create(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let name = body["display_name"].as_str().unwrap_or("");
    let source_query = body["source_query"].as_str().unwrap_or("");
    let output_table_name = body["output_table_name"].as_str().unwrap_or("");
    let output_format = body["output_format"].as_str().unwrap_or("parquet");
    let schedule = body["schedule"].as_str().unwrap_or("");
    let config = body.get("config").map(stringify).unwrap_or_else(|| "{}".into());

    state.db.execute(
        "INSERT INTO derived_tables (id, display_name, source_query, source_type, output_table_name, output_format, schedule, config) VALUES (?1, ?2, ?3, 'duckdb', ?4, ?5, ?6, ?7)",
        &[&id as &dyn rusqlite::types::ToSql, &name as _, &source_query as _, &output_table_name as _, &output_format as _, &schedule as _, &config as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM derived_tables WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql]).map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "derived_table", "create", "success", &format!("Created derived table '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn update(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

    for col in &["display_name", "source_query", "output_table_name", "output_format", "schedule"] {
        if let Some(v) = body.get(*col).and_then(|v| v.as_str()) {
            sets.push(format!("{} = ?", col));
            vals.push(Box::new(v.to_string()));
        }
    }
    if body.get("config").is_some() {
        sets.push("config = ?".to_string());
        vals.push(Box::new(stringify(&body["config"])));
    }
    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')".to_string());

    let sql = format!("UPDATE derived_tables SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Derived table not found")); }

    let row = state.db.query_one("SELECT * FROM derived_tables WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "derived_table", "update", "success", &format!("Updated derived table '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute("DELETE FROM derived_tables WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql]).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Derived table not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "derived_table", "delete", "success", &format!("Deleted derived table '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}

/// Materialize a derived table: run the source query in DuckDB and write result to parquet.
/// DuckDB can read existing parquet files (from pipeline) and create new derived parquet files.
pub async fn materialize(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let result = materialize_inner(&state, &id).await;
    let elapsed = t.elapsed().as_millis() as i64;

    if let Err((_, Json(ref err_val))) = result {
        let err_msg = err_val["error"].as_str().unwrap_or("unknown error");
        super::log_activity(&state, &state.tenant_id, "derived_table", "materialize", "failed",
            &format!("Materialize failed for '{}': {}", id, err_msg),
            None, Some(elapsed));
    }
    result
}

async fn materialize_inner(state: &Arc<AppState>, id: &str) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let dt = state.db.query_one("SELECT * FROM derived_tables WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|_| err(404, "Derived table not found"))?;

    let source_query = dt.get("source_query").and_then(|v| v.as_str()).unwrap_or("");
    if source_query.is_empty() {
        return Err(err(400, "No source query configured"));
    }

    let output_table = dt.get("output_table_name").and_then(|v| v.as_str()).unwrap_or("");
    let parquet_home = state.parquet_home.clone();
    let output_path = if output_table.is_empty() {
        format!("{}/derived/{}", parquet_home, id)
    } else {
        output_table.replace("{PARQUET_HOME}", &parquet_home).replace("${PARQUET_HOME}", &parquet_home)
    };

    let sql = source_query.replace("{PARQUET_HOME}", &parquet_home).replace("${PARQUET_HOME}", &parquet_home);
    let output = output_path.clone();
    let id_owned = id.to_string();

    // Run in DuckDB: execute query on parquet sources → write result to new parquet
    let result = tokio::task::spawn_blocking(move || -> Result<(i64, String), String> {
        let db = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;

        // First: count rows
        let count_sql = format!("SELECT COUNT(*) FROM ({}) AS _c", sql);
        let row_count: i64 = db.query_row(&count_sql, [], |row| row.get(0)).map_err(|e| e.to_string())?;

        // Create output directory
        std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;

        // Write to parquet
        let out_file = format!("{}/data.parquet", output);
        let copy_sql = format!("COPY ({}) TO '{}' (FORMAT PARQUET, COMPRESSION SNAPPY)", sql, out_file);
        db.execute_batch(&copy_sql).map_err(|e| e.to_string())?;

        Ok((row_count, out_file))
    })
    .await
    .map_err(|e| err(500, &format!("Task: {}", e)))?
    .map_err(|e| err(500, &e))?;

    let (row_count, out_file) = result;

    // Update metadata
    state.db.execute(
        "UPDATE derived_tables SET materialized = 1, last_run_at = datetime('now'), last_run_status = 'success', last_run_message = ?1, row_count = ?2, output_table_name = ?3, updated_at = datetime('now') WHERE id = ?4",
        &[
            &format!("Wrote {} rows to {}", row_count, out_file) as &dyn rusqlite::types::ToSql,
            &row_count as _,
            &output_path as _,
            &id_owned as _,
        ],
    ).ok();

    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(state, &state.tenant_id, "derived_table", "materialize", "success",
        &format!("Materialized derived table '{}': {} rows", id, row_count),
        Some(&format!("output: {}", out_file)), Some(elapsed));

    state.db.query_one("SELECT * FROM derived_tables WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map(Json)
        .map_err(|e| err(500, &e.to_string()))
}
