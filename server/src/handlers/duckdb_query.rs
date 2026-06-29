use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use crate::AppState;
use super::err;

/// Generic DuckDB query endpoint — runs SQL against the tenant's tenant_data.duckdb.
/// POST /api/query  { "sql": "SELECT ...", "limit": 100, "offset": 0 }
pub async fn execute(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let args = crate::service::query::DuckdbQueryArgs {
        sql: body["sql"].as_str().unwrap_or("").to_string(),
        limit: body["limit"].as_i64(),
        offset: body["offset"].as_i64(),
    };
    crate::service::query::duckdb(&state, args)
        .await
        .map(Json)
        .map_err(crate::service::error::into_http)
}

/// List all tables in DuckDB.
/// GET /api/query/tables
pub async fn tables(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let duckdb_path = state.duckdb_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        let db = duckdb::Connection::open(&duckdb_path).map_err(|e| format!("DuckDB open: {}", e))?;
        let mut stmt = db.prepare(
            "SELECT table_name, estimated_size, column_count \
             FROM duckdb_tables() \
             ORDER BY table_name"
        ).map_err(|e| e.to_string())?;
        let frames = stmt.query_arrow(duckdb::params![]).map_err(|e| e.to_string())?;

        let mut rows = Vec::new();
        for batch in frames {
            let col_names: Vec<String> = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
            for row_idx in 0..batch.num_rows() {
                let mut obj = serde_json::Map::new();
                for (col_idx, name) in col_names.iter().enumerate() {
                    obj.insert(name.clone(), crate::query::arrow_to_json(batch.column(col_idx), row_idx));
                }
                rows.push(Value::Object(obj));
            }
        }
        Ok(rows)
    })
    .await
    .map_err(|e| err(500, &format!("Task: {}", e)))?
    .map_err(|e| err(500, &e))?;

    Ok(Json(json!({ "tables": result })))
}

/// `GET /api/duckdb/relations` — every queryable object (base tables +
/// user views) in the tenant DuckDB, sorted by name. Used by the
/// GraphDesigner SourcesInspector's table combobox so users can
/// autocomplete against what's actually present.
///
/// Response: `{ "relations": [{ "name": "asv2_ph_master", "kind": "table" }, …] }`
pub async fn relations(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let duckdb_path = state.duckdb_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        let db = duckdb::Connection::open(&duckdb_path).map_err(|e| e.to_string())?;
        // Two unions: base tables + user views. `internal = false`
        // on `duckdb_views()` skips system schemas (information_schema
        // etc.); `duckdb_tables()` doesn't expose system tables.
        let mut stmt = db
            .prepare(
                "SELECT table_name AS name, 'table' AS kind FROM duckdb_tables() \
                 UNION ALL \
                 SELECT view_name AS name, 'view' AS kind FROM duckdb_views() WHERE internal = false \
                 ORDER BY 1",
            )
            .map_err(|e| e.to_string())?;
        let frames = stmt.query_arrow(duckdb::params![]).map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        for batch in frames {
            let col_names: Vec<String> = batch
                .schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect();
            for row_idx in 0..batch.num_rows() {
                let mut obj = serde_json::Map::new();
                for (col_idx, name) in col_names.iter().enumerate() {
                    obj.insert(
                        name.clone(),
                        crate::query::arrow_to_json(batch.column(col_idx), row_idx),
                    );
                }
                rows.push(Value::Object(obj));
            }
        }
        Ok(rows)
    })
    .await
    .map_err(|e| err(500, &format!("Task: {}", e)))?
    .map_err(|e| err(500, &e))?;
    Ok(Json(json!({ "relations": result })))
}

/// Drop a DuckDB table from `tenant_data.duckdb`.
/// DELETE /api/query/tables/{name}
///
/// Blocked when one or more Sources reference the table via `target_table` —
/// drop those Sources (or rename their target) first to keep lineage honest.
pub async fn drop_table(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    if name.is_empty() || name.contains('"') {
        return Err(err(400, "invalid table name"));
    }

    // Block if any Source binds to this table — otherwise we'd silently
    // orphan the Source row and the next materialize/CDC start would fail
    // confusingly. The user should drop the Source first (which is itself
    // blocked while DataViews bind to it).
    let bindings = state.db.query(
        "SELECT id, display_name, kind FROM sources WHERE target_table = ?1",
        &[&name as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    if !bindings.is_empty() {
        let names: Vec<String> = bindings.iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from))
            .collect();
        return Err(err(409, &format!(
            "Table '{}' is the target of Source(s): {}. Delete (or re-target) those Sources first.",
            name, names.join(", ")
        )));
    }

    let duckdb_path = state.duckdb_path.clone();
    let table = name.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let db = duckdb::Connection::open(&duckdb_path).map_err(|e| format!("DuckDB open: {}", e))?;
        db.execute_batch(&format!(r#"DROP TABLE IF EXISTS "{}""#, table))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| err(500, &format!("Task: {}", e)))?
    .map_err(|e| err(500, &e))?;

    Ok(Json(json!({ "dropped": name })))
}

/// Truncate a SQL fragment for inclusion in error messages so the client
/// can see *which* statement of a multi-statement script failed.
fn short(s: &str) -> String {
    let s = s.trim();
    if s.len() <= 80 { s.to_string() } else { format!("{}…", &s[..80]) }
}
