use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::err;

/// Browse parquet files on disk. Returns file listing with partition info.
pub async fn browse(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let path = body["path"].as_str().unwrap_or("");
    if path.is_empty() { return Err(err(400, "path is required")); }

    // Paths are relative — prepend parquet_home. Also handle legacy {PARQUET_HOME} prefix.
    let resolved = if path.starts_with('/') || path.starts_with("gs://") || path.contains("{PARQUET_HOME}") {
        path.replace("{PARQUET_HOME}", &state.parquet_home).replace("${PARQUET_HOME}", &state.parquet_home)
    } else {
        format!("{}/{}", state.parquet_home.trim_end_matches('/'), path)
    };

    let parquet_home = state.parquet_home.clone();
    let resolved_clone = resolved.clone();

    // List files and get metadata via DuckDB
    let result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let base = std::path::Path::new(&resolved_clone);

        // Check if directory exists
        if !base.exists() {
            return Ok(json!({
                "exists": false,
                "path": resolved_clone,
                "files": [],
                "total_files": 0,
                "message": "Directory does not exist. Run the pipeline to materialize parquet files."
            }));
        }

        // Walk directory to find .parquet files
        let mut files: Vec<Value> = Vec::new();
        let mut partitions: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        walk_parquet_files(base, base, &mut files, &mut partitions);

        // Try to get row count + schema via DuckDB
        let mut total_rows: i64 = 0;
        let mut schema_cols: Vec<Value> = Vec::new();

        if !files.is_empty() {
            if let Ok(db) = duckdb::Connection::open_in_memory() {
                let glob = format!("{}/**/*.parquet", resolved_clone);
                let has_hive = !partitions.is_empty();
                let sql = if has_hive {
                    format!("SELECT COUNT(*) FROM read_parquet('{}', hive_partitioning=true)", glob)
                } else {
                    format!("SELECT COUNT(*) FROM read_parquet('{}')", glob)
                };
                if let Ok(count) = db.query_row(&sql, [], |row| row.get::<_, i64>(0)) {
                    total_rows = count;
                }

                // Get schema
                let schema_sql = if has_hive {
                    format!("SELECT * FROM read_parquet('{}', hive_partitioning=true) LIMIT 0", glob)
                } else {
                    format!("SELECT * FROM read_parquet('{}') LIMIT 0", glob)
                };
                if let Ok(mut stmt) = db.prepare(&schema_sql) {
                    if let Ok(_) = stmt.query([]) {
                        let names = stmt.column_names();
                        schema_cols = names.into_iter().map(|n| json!({"name": n})).collect();
                    }
                }
            }
        }

        let partition_list: Vec<String> = partitions.into_iter().collect();

        Ok(json!({
            "exists": true,
            "path": resolved_clone,
            "files": files,
            "total_files": files.len(),
            "total_rows": total_rows,
            "partitions": partition_list,
            "schema": schema_cols,
        }))
    })
    .await
    .map_err(|e| err(500, &e.to_string()))?
    .map_err(|e| err(500, &e))?;

    Ok(Json(result))
}

/// Materialize a DataView's parquet by running its pipeline.
/// This is a convenience wrapper that takes a DataView ID and triggers the pipeline.
pub async fn materialize(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let dv_id = body["dataview_id"].as_str().unwrap_or("");
    if dv_id.is_empty() { return Err(err(400, "dataview_id is required")); }

    // Delegate to pipeline execute
    let row = state.db.query_one(
        "SELECT * FROM dataviews WHERE id = ?1",
        &[&dv_id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "DataView not found"))?;

    let backend_workflow = row.get("backend_workflow").cloned().unwrap_or(json!({}));
    let _contract = row.get("contract").cloned().unwrap_or(json!({}));

    // Resolve PG connection from data_sources: prefer is_default=1, fall back to first pg row.
    let pg_conn_str: Option<String> = (|| -> Option<String> {
        let sources = state.db.query("SELECT * FROM connections", &[]).ok()?;
        let is_pg = |c: &&Value| {
            let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
            t == "pg" || t == "postgres"
        };
        let is_default = |c: &&Value| c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
        let conn = sources.iter().find(|c| is_pg(c) && is_default(c))
            .or_else(|| sources.iter().find(is_pg))?;
        crate::query::pg_conn_str(conn.get("config")?)
    })();

    let tenant_id = state.tenant_id.clone();
    state.traces.log_activity(&tenant_id, "pipeline", dv_id, "info", &format!("Pipeline started for {}", dv_id), None, None).ok();

    let result = crate::pipeline::execute(
        dv_id,
        &backend_workflow,
        pg_conn_str.as_deref(),
        &state.parquet_home,
    ).await;

    match &result {
        Ok(r) => {
            let status = r.get("status").and_then(|v| v.as_str()).unwrap_or("unknown");
            let duration = r.get("total_time_ms").and_then(|v| v.as_i64()).unwrap_or(0);
            let tasks_json = r.get("tasks").map(|v| v.to_string()).unwrap_or_default();
            state.traces.log_pipeline_run(&tenant_id, dv_id, status, &tasks_json, duration).ok();

            if status == "success" {
                let row_count = r.get("tasks").and_then(|t| t.as_array())
                    .and_then(|arr| arr.iter().find(|t| t.get("row_count").is_some()))
                    .and_then(|t| t.get("row_count")).and_then(|v| v.as_i64()).unwrap_or(0);
                state.traces.log_activity(&tenant_id, "pipeline", dv_id, "success",
                    &format!("Pipeline complete for {} — {} rows written in {}ms", dv_id, row_count, duration), None, Some(duration)).ok();
            } else {
                let msg = r.get("tasks").and_then(|t| t.as_array())
                    .and_then(|arr| arr.iter().find(|t| t.get("status").and_then(|s| s.as_str()) == Some("failed")))
                    .and_then(|t| t.get("message")).and_then(|v| v.as_str()).unwrap_or("Materialize failed");
                state.traces.log_error(&tenant_id, &format!("materialize:{}", dv_id), msg, &tasks_json).ok();
            }
        }
        Err(e) => {
            state.traces.log_error(&tenant_id, &format!("materialize:{}", dv_id), &e.to_string(), "").ok();
        }
    }

    let result = result.map_err(|e: anyhow::Error| err(500, &e.to_string()))?;
    Ok(Json(result))
}

fn walk_parquet_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    files: &mut Vec<Value>,
    partitions: &mut std::collections::BTreeSet<String>,
) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Check if directory name looks like a partition (key=value)
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.contains('=') {
                        if let Some(key) = name.split('=').next() {
                            partitions.insert(key.to_string());
                        }
                    }
                }
                walk_parquet_files(root, &path, files, partitions);
            } else if path.extension().and_then(|e| e.to_str()) == Some("parquet") {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                files.push(json!({
                    "path": relative.to_string_lossy(),
                    "size": size,
                    "size_mb": format!("{:.1}", size as f64 / 1_048_576.0),
                }));
            }
        }
    }
}
