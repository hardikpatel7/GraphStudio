//! Connection (data-source) read services. The `connections` table stores
//! per-kind connection rows (PG, BigQuery, DuckDB, ClickHouse, ...). For the
//! agent's `list_connections` tool we expose a masked-list shape that's
//! safe to feed to the model — passwords are replaced with a fixed bullet
//! string in the config JSON.
//!
//! ClickHouse-specific tools (`clickhouse_query`, `clickhouse_dictionary`)
//! also live here — both look up a ClickHouse connection by id, build a
//! `ChConnection`, and call into `crate::clickhouse`. The HTTP handler
//! `handlers::datasources::dictionary` thins down to delegate here; the
//! `execute_query` handler keeps its mixed-mode shape (PG/CH/DuckDB) since
//! the agent only needs the CH path.

use std::time::Instant;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

use super::error::ServiceError;
use super::ServiceResult;

/// Bullet-character mask. Mirrors the constant in `handlers::datasources`
/// so the HTTP response and the agent's view of a connection match exactly.
const PASSWORD_MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

fn mask_password(mut row: Value) -> Value {
    if let Some(config) = row.get_mut("config") {
        if let Some(obj) = config.as_object_mut() {
            if obj.contains_key("password") {
                obj.insert("password".to_string(), Value::String(PASSWORD_MASK.to_string()));
            }
        }
    }
    row
}

pub async fn list(state: &AppState) -> Result<Vec<Value>> {
    let rows = state
        .db
        .query("SELECT * FROM connections ORDER BY display_name", &[])
        .unwrap_or_default();
    Ok(rows.into_iter().map(mask_password).collect())
}

// ── ClickHouse ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct DictionaryArgs {
    /// Optional database filter (overrides the connection's
    /// `default_database` hint). Empty string is treated as "no filter".
    #[serde(default)]
    pub database: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClickhouseQueryArgs {
    pub sql: String,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

fn load_conn(state: &AppState, id: &str) -> ServiceResult<(String, Value)> {
    let row = state
        .db
        .query_one(
            "SELECT * FROM connections WHERE id = ?1",
            &[&id as &dyn rusqlite::types::ToSql],
        )
        .map_err(|_| ServiceError::not_found("Connection not found"))?;
    let ty = row
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let cfg = row.get("config").cloned().unwrap_or(json!({}));
    Ok((ty, cfg))
}

pub async fn clickhouse_dictionary(
    state: &AppState,
    id: &str,
    args: DictionaryArgs,
) -> ServiceResult<Value> {
    let (ty, config) = load_conn(state, id)?;
    if ty != "clickhouse" {
        return Err(ServiceError::bad_request(
            "connection is not a ClickHouse connection",
        ));
    }
    let conn = crate::clickhouse::ChConnection::from_config(&config)
        .map_err(|e| ServiceError::bad_request(format!("ClickHouse config invalid: {e:#}")))?;
    let only_db = args
        .database
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(conn.default_database.as_deref())
        .map(|s| s.to_string());
    let (dict, elapsed_ms) = crate::clickhouse::dictionary(&conn, only_db.as_deref())
        .await
        .map_err(|e| ServiceError::internal(anyhow::anyhow!("ClickHouse dictionary build: {e:#}")))?;
    let mut merged = match dict {
        Value::Object(m) => m,
        other => return Ok(other),
    };
    merged.insert("duration_ms".into(), json!(elapsed_ms));
    if let Some(name) = only_db {
        merged.insert("database_filter".into(), json!(name));
    }
    Ok(Value::Object(merged))
}

pub async fn clickhouse_query(
    state: &AppState,
    id: &str,
    args: ClickhouseQueryArgs,
) -> ServiceResult<Value> {
    let (ty, config) = load_conn(state, id)?;
    if ty != "clickhouse" {
        return Err(ServiceError::bad_request(
            "connection is not a ClickHouse connection",
        ));
    }
    let conn = crate::clickhouse::ChConnection::from_config(&config)
        .map_err(|e| ServiceError::bad_request(format!("ClickHouse config invalid: {e:#}")))?;
    // Trim AND strip a trailing semicolon before wrapping. Without
    // this the agent's perfectly-valid `SELECT ... FROM t;` becomes
    // `SELECT * FROM (SELECT ... FROM t;) LIMIT 100 OFFSET 0`, which
    // ClickHouse rejects as `Syntax error: failed at … ;)` — the
    // model then loops trying different SQL variants and never
    // recovers (Item workspace was hitting this on every prompt).
    let sql_raw = args.sql;
    let cleaned = sql_raw.trim().trim_end_matches(';').trim_end().to_string();
    let limit = args.limit.unwrap_or(100);
    let offset = args.offset.unwrap_or(0);
    let upper = cleaned.trim_start().to_uppercase();
    let is_query = upper.starts_with("SELECT") || upper.starts_with("WITH");
    let wrapped = if is_query {
        format!("SELECT * FROM ({cleaned}) LIMIT {limit} OFFSET {offset}")
    } else {
        cleaned.clone()
    };

    let t = Instant::now();
    let result = crate::clickhouse::query_exec(&conn, &wrapped)
        .await
        .map_err(|e| ServiceError::bad_request(format!("ClickHouse query failed: {e:#}")))?;
    let columns: Vec<Value> = result
        .rows
        .first()
        .and_then(|r| r.as_object())
        .map(|m| m.keys().map(|k| json!({ "name": k })).collect())
        .unwrap_or_default();
    let row_count = result.rows.len();
    Ok(json!({
        "rows": result.rows,
        "total": row_count,
        "columns": columns,
        "duration_ms": t.elapsed().as_millis() as i64,
        "client_ms": result.client_ms,
        "server_ms": result.server_ms,
        "read_rows": result.read_rows,
        "read_bytes": result.read_bytes,
    }))
}
