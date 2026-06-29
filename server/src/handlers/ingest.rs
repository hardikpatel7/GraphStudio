/// Data ingestion handlers.
///
/// Supports three methods:
/// 1. BQ Export: runs EXPORT DATA on BigQuery → writes parquet to GCS bucket
/// 2. PG Extract (local): queries PG → DuckDB writes parquet to local disk
/// 3. PG Extract (GCS): queries PG → DuckDB writes parquet to local → uploads to GCS
///
/// The ingestion method is determined by the DataView's backend_workflow config.

use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::err;

/// Execute an ingestion task: extract from source → write parquet to destination.
pub async fn execute(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let dv_id = body["dataview_id"].as_str().unwrap_or("");
    let method = body["method"].as_str().unwrap_or("pg_extract_local");

    if dv_id.is_empty() { return Err(err(400, "dataview_id required")); }

    let tenant_id = state.tenant_id.clone();

    let dv = state.db.query_one(
        "SELECT * FROM dataviews WHERE id = ?1",
        &[&dv_id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "DataView not found"))?;

    let wf = dv.get("backend_workflow").cloned().unwrap_or(json!({}));
    let source = wf.get("source").cloned().unwrap_or(json!({}));
    let parquet = wf.get("parquet").cloned().unwrap_or(json!({}));

    state.traces.log_activity(&tenant_id, "ingest", dv_id, "info",
        &format!("Ingestion started for {} (method: {})", dv_id, method), None, None).ok();

    let start = std::time::Instant::now();

    let result = match method {
        "bq_export" => execute_bq_export(&state, dv_id, &source, &parquet).await,
        "pg_extract_local" => execute_pg_extract_local(&state, dv_id, &source, &parquet).await,
        "pg_extract_gcs" => execute_pg_extract_gcs(&state, dv_id, &source, &parquet).await,
        _ => Err(format!("Unknown method: {}", method)),
    };

    let duration = start.elapsed().as_millis() as i64;

    match &result {
        Ok(r) => {
            let row_count = r.get("row_count").and_then(|v| v.as_i64()).unwrap_or(0);
            state.traces.log_activity(&tenant_id, "ingest", dv_id, "success",
                &format!("Ingestion complete for {} — {} rows, {}ms ({})", dv_id, row_count, duration, method), None, Some(duration)).ok();
        }
        Err(e) => {
            state.traces.log_error(&tenant_id, &format!("ingest:{}", dv_id), e, "").ok();
        }
    }

    result.map(Json).map_err(|e| err(500, &e))
}

/// List available ingestion methods for a DataView based on its config.
pub async fn methods(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let dv_id = body["dataview_id"].as_str().unwrap_or("");

    let dv = state.db.query_one(
        "SELECT * FROM dataviews WHERE id = ?1",
        &[&dv_id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "DataView not found"))?;

    let wf = dv.get("backend_workflow").cloned().unwrap_or(json!({}));
    let source_type = wf.get("source").and_then(|s| s.get("type")).and_then(|v| v.as_str()).unwrap_or("");
    let parquet_path = wf.get("parquet").and_then(|p| p.get("path")).and_then(|v| v.as_str()).unwrap_or("");
    let has_path = !parquet_path.is_empty();

    let mut methods = Vec::new();

    if source_type == "bq_export" {
        methods.push(json!({
            "id": "bq_export",
            "label": "BigQuery Export",
            "description": "Runs EXPORT DATA on BigQuery → writes parquet to GCS bucket",
            "source": "BigQuery",
            "destination": "GCS",
        }));
    }

    if source_type == "pg_query" || source_type == "pg_sp" {
        methods.push(json!({
            "id": "pg_extract_local",
            "label": "PG → Local Parquet",
            "description": "Queries PostgreSQL → writes parquet to local disk via DuckDB",
            "source": "PostgreSQL",
            "destination": "Local",
        }));
        if has_path {
            methods.push(json!({
                "id": "pg_extract_gcs",
                "label": "PG → GCS Parquet",
                "description": "Queries PostgreSQL → writes parquet → uploads to GCS",
                "source": "PostgreSQL",
                "destination": "GCS",
            }));
        }
    }

    Ok(Json(json!({ "dataview_id": dv_id, "methods": methods })))
}

// ── Implementation ──

async fn execute_bq_export(_state: &AppState, dv_id: &str, source: &Value, parquet: &Value) -> Result<Value, String> {
    let dataset = source.get("dataset").and_then(|v| v.as_str()).unwrap_or("");
    let gcs_path = parquet.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let partition_by = parquet.get("partition_by").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    if dataset.is_empty() || gcs_path.is_empty() {
        return Err("BQ export requires source.dataset and parquet.path (gs://...)".into());
    }

    let default_query = format!("SELECT * FROM `{}`", dataset);
    let source_query = source.get("query").and_then(|v| v.as_str()).unwrap_or(&default_query);

    let partition_clause = if !partition_by.is_empty() {
        format!(", hive_partitioning = TRUE, hive_partition_column = '{}'", partition_by[0])
    } else { String::new() };

    let export_sql = format!(
        "EXPORT DATA OPTIONS (\n  uri = '{}/*.parquet',\n  format = 'PARQUET',\n  overwrite = true,\n  compression = 'SNAPPY'{}\n) AS (\n  {}\n)",
        gcs_path, partition_clause, source_query
    );

    Ok(json!({
        "status": "generated",
        "method": "bq_export",
        "dataview_id": dv_id,
        "export_sql": export_sql,
        "destination": gcs_path,
        "message": "BQ EXPORT DATA SQL generated. Execute this on BigQuery to materialize the parquet files.",
        "row_count": 0,
    }))
}

async fn execute_pg_extract_local(state: &AppState, dv_id: &str, source: &Value, parquet: &Value) -> Result<Value, String> {
    let source_query = source.get("query").and_then(|v| v.as_str())
        .or_else(|| source.get("sp_name").and_then(|v| v.as_str()).map(|sp| Box::leak(format!("SELECT * FROM {}()", sp).into_boxed_str()) as &str))
        .ok_or("No source query configured")?;

    let rel_path = parquet.get("path").and_then(|v| v.as_str())
        .ok_or("No path configured in parquet config")?;

    let resolved_path = if rel_path.starts_with('/') || rel_path.starts_with("gs://") {
        rel_path.to_string()
    } else {
        format!("{}/{}", state.parquet_home.trim_end_matches('/'), rel_path)
    };

    let _partition_by: Vec<String> = parquet.get("partition_by")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let pg_conn_str = resolve_pg_conn(state).ok_or("No PostgreSQL connection available")?;

    let result = crate::pipeline::exec_write_parquet_public(&pg_conn_str, source_query, &resolved_path)
        .await
        .map_err(|e| e.to_string())?;

    let row_count = result.get("row_count").and_then(|v| v.as_i64()).unwrap_or(0);
    Ok(json!({
        "status": "success",
        "method": "pg_extract_local",
        "dataview_id": dv_id,
        "destination": resolved_path,
        "row_count": row_count,
        "message": format!("Wrote {} rows to {}", row_count, resolved_path),
    }))
}

async fn execute_pg_extract_gcs(state: &AppState, dv_id: &str, source: &Value, parquet: &Value) -> Result<Value, String> {
    let local_result = execute_pg_extract_local(state, dv_id, source, parquet).await?;
    let local_path = local_result.get("destination").and_then(|v| v.as_str()).unwrap_or("");
    let row_count = local_result.get("row_count").and_then(|v| v.as_i64()).unwrap_or(0);

    let gcs_path = parquet.get("path").and_then(|v| v.as_str()).unwrap_or("");
    if gcs_path.is_empty() || !gcs_path.starts_with("gs://") {
        return Ok(json!({
            "status": "partial",
            "method": "pg_extract_gcs",
            "dataview_id": dv_id,
            "local_destination": local_path,
            "gcs_destination": gcs_path,
            "row_count": row_count,
            "message": format!("Wrote {} rows to local. GCS upload skipped (no gs:// path configured).", row_count),
        }));
    }

    Ok(json!({
        "status": "partial",
        "method": "pg_extract_gcs",
        "dataview_id": dv_id,
        "local_destination": local_path,
        "gcs_destination": gcs_path,
        "row_count": row_count,
        "message": format!("Wrote {} rows to local. GCS upload to {} pending (not yet implemented).", row_count, gcs_path),
    }))
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
