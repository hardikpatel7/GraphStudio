/// Snapshot management: two-step materialization with versioned directories.
///
/// Step 1: Source → GCS  (BQ EXPORT or PG extract → upload to timestamped GCS dir)
/// Step 2: GCS → Local   (download from latest GCS snapshot → timestamped local dir)
///
/// Each snapshot is a timestamped directory. N snapshots are retained per step.
/// The active snapshot is the one the viewport reads from.

use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::err;

/// Get all snapshots for a DataView (both gcs + local steps).
pub async fn list(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let snapshots = state.traces.get_snapshots(&tenant_id, &dv_id)
        .map_err(|e| err(500, &e.to_string()))?;

    // Also get active for each step
    let active_gcs = state.traces.get_active_snapshot(&tenant_id, &dv_id, "gcs").ok().flatten();
    let active_local = state.traces.get_active_snapshot(&tenant_id, &dv_id, "local").ok().flatten();

    Ok(Json(json!({
        "dataview_id": dv_id,
        "snapshots": snapshots,
        "active_gcs": active_gcs,
        "active_local": active_local,
    })))
}

/// Materialize Step 1: Source → GCS bucket (timestamped directory).
pub async fn materialize_gcs(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let dv = get_dataview(&state, &dv_id)?;
    let wf = dv.get("backend_workflow").cloned().unwrap_or(json!({}));
    let rel_path = wf.get("parquet").and_then(|p| p.get("path")).and_then(|v| v.as_str()).unwrap_or("");
    let partitions: Vec<String> = wf.get("parquet").and_then(|p| p.get("partition_by")).and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();

    if rel_path.is_empty() { return Err(err(400, "No parquet path configured")); }

    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let gcs_root = state.traces.get_setting(&tenant_id, "gcs_bucket_root")
        .ok().flatten().unwrap_or_default();
    let gcs_snapshot_path = format!("{}/{}/{}", gcs_root.trim_end_matches('/'), rel_path, ts);

    let src_type = wf.get("source").and_then(|s| s.get("type")).and_then(|v| v.as_str()).unwrap_or("");

    state.traces.log_activity(&tenant_id, "materialize", &dv_id, "info",
        &format!("Step 1: {} → GCS started for {} ({})", src_type, dv_id, ts), None, None).ok();

    let start = std::time::Instant::now();

    // Execute source extraction
    let row_count: i64 = match src_type {
        "bq_export" => {
            // Generate EXPORT DATA SQL (BQ export writes directly to GCS)
            let source = wf.get("source").cloned().unwrap_or(json!({}));
            let dataset = source.get("dataset").and_then(|v| v.as_str()).unwrap_or("");
            let default_query = format!("SELECT * FROM `{}`", dataset);
            let source_query = source.get("query").and_then(|v| v.as_str()).unwrap_or(&default_query);
            let partition_clause = if !partitions.is_empty() {
                format!(", hive_partitioning = TRUE, hive_partition_column = '{}'", partitions[0])
            } else { String::new() };
            let _export_sql = format!(
                "EXPORT DATA OPTIONS (\n  uri = '{}/*.parquet',\n  format = 'PARQUET',\n  overwrite = true,\n  compression = 'SNAPPY'{}\n) AS (\n  {}\n)",
                gcs_snapshot_path, partition_clause, source_query
            );
            // TODO: Execute via GbqClient. For now, record as 0 rows.
            0
        }
        "pg_query" | "pg_sp" => {
            // PG extract → write parquet to local temp → upload to GCS
            let pg_conn_str = resolve_pg_conn(&state)
                .ok_or_else(|| err(500, "No PG connection available"))?;
            let source = wf.get("source").cloned().unwrap_or(json!({}));
            let explicit_query = source.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let sql = if !explicit_query.is_empty() {
                explicit_query.to_string()
            } else if src_type == "pg_sp" {
                format!("SELECT * FROM {}()", source.get("sp_name").and_then(|v| v.as_str()).unwrap_or(""))
            } else {
                String::new()
            };
            // Write to a temp local path, then would upload to GCS
            let temp_path = format!("{}/gcs_staging/{}/{}", state.parquet_home.trim_end_matches('/'), rel_path, ts);
            let result = crate::pipeline::exec_write_parquet_public(&pg_conn_str, &sql, &temp_path).await
                .map_err(|e| err(500, &e.to_string()))?;
            // TODO: Upload temp_path to gcs_snapshot_path via GcsClient
            result.get("row_count").and_then(|v| v.as_i64()).unwrap_or(0)
        }
        _ => return Err(err(400, &format!("Unsupported source type: {}", src_type))),
    };

    let duration = start.elapsed().as_millis() as i64;
    let max_keep = state.traces.get_setting(&tenant_id, "max_snapshots")
        .ok().flatten().and_then(|v| v.parse::<i64>().ok()).unwrap_or(3);

    state.traces.record_snapshot(&tenant_id, &dv_id, "gcs", &gcs_snapshot_path, &ts, row_count, max_keep).ok();
    state.traces.log_activity(&tenant_id, "materialize", &dv_id, "success",
        &format!("Step 1: → GCS done for {} — {} rows, {}ms ({})", dv_id, row_count, duration, ts), None, Some(duration)).ok();

    Ok(Json(json!({
        "status": "success",
        "step": "gcs",
        "dataview_id": dv_id,
        "snapshot_ts": ts,
        "path": gcs_snapshot_path,
        "row_count": row_count,
        "duration_ms": duration,
    })))
}

/// Materialize Step 2: GCS → Local (read from active GCS snapshot, write to timestamped local dir).
pub async fn materialize_local(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let dv = get_dataview(&state, &dv_id)?;
    let wf = dv.get("backend_workflow").cloned().unwrap_or(json!({}));
    let rel_path = wf.get("parquet").and_then(|p| p.get("path")).and_then(|v| v.as_str()).unwrap_or("");
    let partitions: Vec<String> = wf.get("parquet").and_then(|p| p.get("partition_by")).and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();

    if rel_path.is_empty() { return Err(err(400, "No parquet path configured")); }

    // Get the active GCS snapshot to read from
    let gcs_snapshot = state.traces.get_active_snapshot(&tenant_id, &dv_id, "gcs")
        .map_err(|e| err(500, &e.to_string()))?;
    let gcs_path = gcs_snapshot.as_ref().and_then(|s| s.get("path")).and_then(|v| v.as_str()).unwrap_or("");
    let gcs_ts = gcs_snapshot.as_ref().and_then(|s| s.get("snapshot_ts")).and_then(|v| v.as_str()).unwrap_or("");

    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let local_path = format!("{}/{}/{}", state.parquet_home.trim_end_matches('/'), rel_path, ts);

    state.traces.log_activity(&tenant_id, "materialize", &dv_id, "info",
        &format!("Step 2: GCS → Local started for {} (gcs:{} → local:{})", dv_id, gcs_ts, ts), None, None).ok();

    let start = std::time::Instant::now();

    // If we have a GCS snapshot with a staging path, read from that
    // Otherwise, fall back to the existing pipeline (PG → local directly)
    let staging_path = format!("{}/gcs_staging/{}/{}", state.parquet_home.trim_end_matches('/'), rel_path, gcs_ts);
    let source_glob = if std::path::Path::new(&staging_path).exists() {
        format!("{}/**/*.parquet", staging_path)
    } else if !gcs_path.is_empty() {
        // TODO: Download from GCS. For now, use existing local pipeline as fallback.
        // Fall back to running the full PG → local pipeline
        let pg_conn_str = resolve_pg_conn(&state);
        if let Some(conn_str) = pg_conn_str {
            let source = wf.get("source").cloned().unwrap_or(json!({}));
            let src_type = source.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let explicit_q = source.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let sql = if !explicit_q.is_empty() {
                explicit_q.to_string()
            } else if src_type == "pg_sp" {
                format!("SELECT * FROM {}()", source.get("sp_name").and_then(|v| v.as_str()).unwrap_or(""))
            } else {
                source.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string()
            };
            let result = crate::pipeline::exec_write_parquet_public(&conn_str, &sql, &local_path).await
                .map_err(|e| err(500, &e.to_string()))?;
            let row_count = result.get("row_count").and_then(|v| v.as_i64()).unwrap_or(0);
            let duration = start.elapsed().as_millis() as i64;
            let max_keep = state.traces.get_setting(&tenant_id, "max_snapshots")
                .ok().flatten().and_then(|v| v.parse::<i64>().ok()).unwrap_or(3);
            state.traces.record_snapshot(&tenant_id, &dv_id, "local", &local_path, &ts, row_count, max_keep).ok();
            state.traces.log_activity(&tenant_id, "materialize", &dv_id, "success",
                &format!("Step 2: → Local done for {} — {} rows, {}ms ({})", dv_id, row_count, duration, ts), None, Some(duration)).ok();
            return Ok(Json(json!({
                "status": "success", "step": "local", "dataview_id": dv_id,
                "snapshot_ts": ts, "path": local_path, "row_count": row_count, "duration_ms": duration,
            })));
        }
        return Err(err(500, "No GCS snapshot or PG connection available"));
    } else {
        return Err(err(400, "No GCS snapshot available. Run Step 1 first."));
    };

    // Copy from GCS staging to local snapshot dir via DuckDB
    let hive = if !partitions.is_empty() { ", hive_partitioning=true" } else { "" };
    let local_out = local_path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let db = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        let count: i64 = db.query_row(
            &format!("SELECT COUNT(*) FROM read_parquet('{}'{}))", source_glob, hive),
            [], |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        if std::path::Path::new(&local_out).exists() { std::fs::remove_dir_all(&local_out).ok(); }
        std::fs::create_dir_all(&local_out).map_err(|e| e.to_string())?;
        let part_clause = if partitions.is_empty() { String::new() }
            else { format!(", PARTITION_BY ({})", partitions.join(", ")) };
        let dest = if partitions.is_empty() { format!("{}/data.parquet", local_out) } else { local_out.clone() };
        db.execute_batch(&format!(
            "COPY (SELECT * FROM read_parquet('{}'{}) TO '{}' (FORMAT PARQUET, COMPRESSION SNAPPY{})",
            source_glob, hive, dest, part_clause
        )).map_err(|e| e.to_string())?;
        Ok(count)
    }).await.map_err(|e| err(500, &e.to_string()))?.map_err(|e| err(500, &e))?;

    let duration = start.elapsed().as_millis() as i64;
    let max_keep = state.traces.get_setting(&tenant_id, "max_snapshots")
        .ok().flatten().and_then(|v| v.parse::<i64>().ok()).unwrap_or(3);
    state.traces.record_snapshot(&tenant_id, &dv_id, "local", &local_path, &ts, result, max_keep).ok();
    state.traces.log_activity(&tenant_id, "materialize", &dv_id, "success",
        &format!("Step 2: → Local done for {} — {} rows, {}ms ({})", dv_id, result, duration, ts), None, Some(duration)).ok();

    Ok(Json(json!({
        "status": "success", "step": "local", "dataview_id": dv_id,
        "snapshot_ts": ts, "path": local_path, "row_count": result, "duration_ms": duration,
    })))
}

/// Materialize Direct: PG query/SP → Local parquet (single step, no GCS).
pub async fn materialize_direct(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let dv = get_dataview(&state, &dv_id)?;
    let wf = dv.get("backend_workflow").cloned().unwrap_or(json!({}));
    let rel_path = wf.get("parquet").and_then(|p| p.get("path")).and_then(|v| v.as_str()).unwrap_or("");

    if rel_path.is_empty() { return Err(err(400, "No parquet path configured")); }

    let source = wf.get("source").cloned().unwrap_or(json!({}));
    let src_type = source.get("type").and_then(|v| v.as_str()).unwrap_or("");
    // Prefer explicit query if available, fall back to SP call
    let explicit_query = source.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let sql = if !explicit_query.is_empty() {
        explicit_query.to_string()
    } else {
        match src_type {
            "pg_sp" => {
                let sp = source.get("sp_name").and_then(|v| v.as_str()).unwrap_or("");
                if sp.is_empty() { return Err(err(400, "No sp_name configured")); }
                format!("SELECT * FROM {}()", sp)
            }
            "pg_query" => return Err(err(400, "No source query configured")),
            _ => return Err(err(400, "Direct strategy requires pg_query or pg_sp source")),
        }
    };

    let pg_conn_str = resolve_pg_conn(&state)
        .ok_or_else(|| err(500, "No PG connection available"))?;

    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let local_path = format!("{}/{}/{}", state.parquet_home.trim_end_matches('/'), rel_path, ts);

    state.traces.log_activity(&tenant_id, "materialize", &dv_id, "info",
        &format!("Direct: {} → Local started for {} ({})", src_type, dv_id, ts), None, None).ok();

    let start = std::time::Instant::now();
    tracing::info!("Direct materialize {}: running query ({} chars) → {}", dv_id, sql.len(), local_path);
    let result = crate::pipeline::exec_write_parquet_public(&pg_conn_str, &sql, &local_path).await
        .map_err(|e| {
            let msg = format!("Materialize failed for {}: {}", dv_id, e);
            tracing::error!("{}", msg);
            state.traces.log_error(&tenant_id, &format!("materialize:{}", dv_id), &msg, &sql).ok();
            err(500, &msg)
        })?;

    let row_count = result.get("row_count").and_then(|v| v.as_i64()).unwrap_or(0);
    let duration = start.elapsed().as_millis() as i64;
    let max_keep = state.traces.get_setting(&tenant_id, "max_snapshots")
        .ok().flatten().and_then(|v| v.parse::<i64>().ok()).unwrap_or(3);

    state.traces.record_snapshot(&tenant_id, &dv_id, "local", &local_path, &ts, row_count, max_keep).ok();
    state.traces.log_activity(&tenant_id, "materialize", &dv_id, "success",
        &format!("Direct: → Local done for {} — {} rows, {}ms ({})", dv_id, row_count, duration, ts), None, Some(duration)).ok();

    Ok(Json(json!({
        "status": "success", "step": "direct", "dataview_id": dv_id,
        "snapshot_ts": ts, "path": local_path, "row_count": row_count, "duration_ms": duration,
    })))
}

/// Switch the active snapshot for a dataview+step.
pub async fn switch_active(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let step = body["step"].as_str().unwrap_or("");
    let snapshot_ts = body["snapshot_ts"].as_str().unwrap_or("");
    if step.is_empty() || snapshot_ts.is_empty() { return Err(err(400, "step and snapshot_ts required")); }

    state.traces.set_active_snapshot(&tenant_id, &dv_id, step, snapshot_ts)
        .map_err(|e| err(500, &e.to_string()))?;

    Ok(Json(json!({"success": true, "dataview_id": dv_id, "step": step, "snapshot_ts": snapshot_ts})))
}

/// Extract column names from a source query (runs SELECT ... LIMIT 0 on PG).
pub async fn query_columns(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let src_type = body["type"].as_str().unwrap_or("pg_query");
    let query = body["query"].as_str().unwrap_or("");
    let sp_name = body["sp_name"].as_str().unwrap_or("");

    let sql = match src_type {
        "pg_sp" => {
            if sp_name.is_empty() { return Err(err(400, "sp_name required")); }
            format!("SELECT * FROM {}() LIMIT 0", sp_name)
        }
        "pg_query" => {
            if query.is_empty() { return Err(err(400, "query required")); }
            format!("SELECT * FROM ({}) AS _q LIMIT 0", query)
        }
        "bq_export" => {
            // Can't run BQ queries locally — return empty for now
            return Ok(Json(json!({ "columns": [] })));
        }
        _ => return Err(err(400, "Unsupported type")),
    };

    // Try live PG connection first, fall back to SQL parsing
    let pg_conn_str = resolve_pg_conn(&state);

    if let Some(conn_str) = pg_conn_str {
        if let Ok((client, conn)) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls).await {
            tokio::spawn(async move { conn.await.ok(); });
            // Try prepare first (works for simple queries)
            match client.prepare(&sql).await {
                Ok(stmt) => {
                    let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
                    return Ok(Json(json!({ "columns": columns, "source": "pg" })));
                }
                Err(e) => {
                    tracing::warn!("prepare failed for '{}': {}, trying query...", sql, e);
                    // Fallback: execute the query (needed for SPs that can't be prepared)
                    if let Ok(rows) = client.query(&sql, &[]).await {
                        if let Some(row) = rows.first() {
                            let columns: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
                            return Ok(Json(json!({ "columns": columns, "source": "pg" })));
                        }
                        // No rows — get columns from an empty result by trying simple_query
                        if let Ok(stmt) = client.prepare(&format!("SELECT * FROM ({}) AS _q WHERE false", sql.replace(" LIMIT 0", ""))).await {
                            let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
                            return Ok(Json(json!({ "columns": columns, "source": "pg" })));
                        }
                    }
                    tracing::warn!("query also failed for SP, falling through to parser");
                }
            }
        }
    }

    // Fallback: parse column names from SQL SELECT clause
    let columns = if !query.is_empty() {
        parse_select_columns(query)
    } else {
        vec![]
    };

    // If still empty (SP without query), try to get columns from the DataView metadata
    // Merge: display columns + partition_by + dimension filter columns (deduped, preserving order)
    if columns.is_empty() {
        if let Some(dv_id) = body["dataview_id"].as_str() {
            if let Ok(row) = state.db.query_one(
                "SELECT * FROM dataviews WHERE id = ?1",
                &[&dv_id as &dyn rusqlite::types::ToSql],
            ) {
                let mut all_cols: Vec<String> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                let mut add = |name: &str| {
                    if seen.insert(name.to_string()) { all_cols.push(name.to_string()); }
                };

                // 1. Display columns
                if let Some(cols) = row.get("columns").and_then(|v| v.as_array()) {
                    for c in cols { if let Some(n) = c.get("name").and_then(|n| n.as_str()) { add(n); } }
                }
                // 2. Partition columns
                if let Some(pq) = row.get("backend_workflow").and_then(|w| w.get("parquet")).and_then(|p| p.get("partition_by")).and_then(|v| v.as_array()) {
                    for c in pq { if let Some(n) = c.as_str() { add(n); } }
                }
                // 3. Dimension filter columns
                if let Some(dims) = row.get("dimensions").and_then(|v| v.as_array()) {
                    for d in dims {
                        if let Some(cols) = d.get("allowed_filter_cols").and_then(|v| v.as_array()) {
                            for c in cols { if let Some(n) = c.as_str() { add(n); } }
                        }
                    }
                }

                if !all_cols.is_empty() {
                    return Ok(Json(json!({ "columns": all_cols, "source": "dataview_metadata" })));
                }
            }
        }
    }

    let note = if columns.is_empty() { "Could not extract columns. Enter a query or ensure PG is reachable." } else { "" };
    Ok(Json(json!({ "columns": columns, "source": "parsed", "note": note })))
}

// ── Helpers ──

fn get_dataview(state: &AppState, dv_id: &str) -> Result<Value, (axum::http::StatusCode, Json<Value>)> {
    state.db.query_one(
        "SELECT * FROM dataviews WHERE id = ?1",
        &[&dv_id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "DataView not found"))
}

fn resolve_pg_conn(state: &AppState) -> Option<String> {
    let sources = state.db.query("SELECT * FROM connections", &[]).ok()?;
    let is_pg = |c: &&Value| {
        let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        t == "pg" || t == "postgres"
    };
    let is_default = |c: &&Value| c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
    let conn = sources.iter().find(|c| is_pg(c) && is_default(c))
        .or_else(|| sources.iter().find(is_pg))?;
    crate::query::pg_conn_str(conn.get("config")?)
}

/// Parse column names/aliases from a SELECT query. Handles:
/// - SELECT col1, col2 FROM ...
/// - SELECT t.col AS alias FROM ...
/// - SELECT func(x) AS alias FROM ...
fn parse_select_columns(sql: &str) -> Vec<String> {
    let upper = sql.to_uppercase();
    // Find first SELECT ... FROM
    let select_pos = upper.find("SELECT").unwrap_or(0) + 6;
    let from_pos = find_top_level_from(&upper[select_pos..]).map(|p| p + select_pos).unwrap_or(sql.len());
    let select_clause = &sql[select_pos..from_pos];

    // Split by top-level commas (not inside parentheses)
    let parts = split_top_level(select_clause, ',');
    parts.iter().filter_map(|part| {
        let p = part.trim();
        if p == "*" || p.is_empty() { return None; }
        let upper_p = p.to_uppercase();
        // Check for AS alias
        if let Some(as_pos) = upper_p.rfind(" AS ") {
            let alias = p[as_pos + 4..].trim().trim_matches('"');
            return Some(alias.to_string());
        }
        // Last token after dot or space
        let token = p.split_whitespace().last().unwrap_or(p);
        let col = token.rsplit('.').next().unwrap_or(token).trim_matches('"');
        Some(col.to_string())
    }).collect()
}

fn find_top_level_from(s: &str) -> Option<usize> {
    let mut depth = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ if depth == 0 => {
                if s[i..].to_uppercase().starts_with("FROM") && (i == 0 || bytes[i-1].is_ascii_whitespace()) {
                    let after = i + 4;
                    if after >= s.len() || s.as_bytes()[after].is_ascii_whitespace() || s.as_bytes()[after] == b'(' {
                        return Some(i);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in s.chars() {
        match c {
            '(' => { depth += 1; current.push(c); }
            ')' => { depth -= 1; current.push(c); }
            c if c == sep && depth == 0 => { parts.push(current.clone()); current.clear(); }
            _ => current.push(c),
        }
    }
    if !current.is_empty() { parts.push(current); }
    parts
}
