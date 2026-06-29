//! Ad-hoc query services. v1: DuckDB `SELECT` (and other read-shaped
//! statements like `SHOW`/`DESCRIBE`/`PRAGMA`) against the tenant
//! `tenant_data.duckdb`. The handler in `handlers::duckdb_query` becomes a
//! thin shim; the agent's `duckdb_query` tool calls the same service fn.

use std::time::Instant;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

use super::error::ServiceError;
use super::ServiceResult;

#[derive(Debug, Deserialize, Default)]
pub struct DuckdbQueryArgs {
    pub sql: String,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

pub async fn duckdb(state: &AppState, args: DuckdbQueryArgs) -> ServiceResult<Value> {
    let t = Instant::now();
    let sql = args.sql.trim().to_string();
    if sql.is_empty() {
        return Err(ServiceError::bad_request("sql is required"));
    }
    let limit = args.limit.unwrap_or(500);
    let offset = args.offset.unwrap_or(0);

    let duckdb_path = state.duckdb_path.clone();
    let parquet_home = state.parquet_home.clone();

    let join_result = tokio::task::spawn_blocking(
        move || -> Result<(Vec<String>, Vec<Value>, i64), String> {
            let db = duckdb::Connection::open(&duckdb_path)
                .map_err(|e| format!("DuckDB open: {e}"))?;

            let resolved_sql = sql
                .replace("{PARQUET_HOME}", &parquet_home)
                .replace("${PARQUET_HOME}", &parquet_home);

            let stmts = crate::query::split_statements(&resolved_sql);
            if stmts.is_empty() {
                return Err("sql is empty after stripping comments".to_string());
            }
            for prelude in &stmts[..stmts.len() - 1] {
                db.execute_batch(prelude)
                    .map_err(|e| format!("{}: {e}", short(prelude)))?;
            }
            let last = stmts.last().unwrap();

            let trimmed = last.trim_start().to_uppercase();
            let is_select = trimmed.starts_with("SELECT")
                || trimmed.starts_with("WITH")
                || trimmed.starts_with("SHOW")
                || trimmed.starts_with("DESCRIBE")
                || trimmed.starts_with("PRAGMA")
                || trimmed.starts_with("FROM");

            let exec_sql = if is_select {
                format!("SELECT * FROM ({last}) AS _q LIMIT {limit} OFFSET {offset}")
            } else {
                last.clone()
            };

            let mut stmt = db.prepare(&exec_sql).map_err(|e| e.to_string())?;
            let frames = stmt
                .query_arrow(duckdb::params![])
                .map_err(|e| e.to_string())?;

            let mut col_names: Vec<String> = Vec::new();
            let mut all_rows = Vec::new();
            for batch in frames {
                if col_names.is_empty() {
                    col_names = batch
                        .schema()
                        .fields()
                        .iter()
                        .map(|f| f.name().clone())
                        .collect();
                }
                for row_idx in 0..batch.num_rows() {
                    let mut obj = serde_json::Map::new();
                    for (col_idx, name) in col_names.iter().enumerate() {
                        let col = batch.column(col_idx);
                        let json_val = crate::query::arrow_to_json(col, row_idx);
                        obj.insert(name.clone(), json_val);
                    }
                    all_rows.push(Value::Object(obj));
                }
            }

            let total = if is_select {
                let count_sql = format!("SELECT COUNT(*) FROM ({last}) AS _c");
                db.query_row(&count_sql, [], |row| row.get::<_, i64>(0))
                    .unwrap_or(all_rows.len() as i64)
            } else {
                all_rows.len() as i64
            };

            Ok((col_names, all_rows, total))
        },
    )
    .await
    .map_err(|e| ServiceError::internal(anyhow::anyhow!("task: {e}")))?;
    let (col_names, rows, total) = join_result.map_err(ServiceError::bad_request)?;

    let row_count = rows.len();
    Ok(json!({
        "columns": col_names,
        "rows": rows,
        "total": total,
        "row_count": row_count,
        "duration_ms": t.elapsed().as_millis() as i64,
    }))
}

fn short(s: &str) -> String {
    let mut out: String = s.chars().take(80).collect();
    if out.len() < s.len() {
        out.push('…');
    }
    out
}
