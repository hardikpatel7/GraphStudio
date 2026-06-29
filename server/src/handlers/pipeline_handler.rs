use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use crate::AppState;
use super::err;

/// Format a tokio_postgres::Error into a developer-actionable message.
/// Keeps SQLSTATE, message, detail, hint, position, and the schema-qualified table /
/// column / constraint references. Drops PG internals (where/routine/file:line) since
/// they point at PG's own C source and aren't usable from the UI.
pub(crate) fn format_pg_err(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        let mut parts = vec![format!("[{}] {}", db.code().code(), db.message())];
        if let Some(d) = db.detail()    { parts.push(format!("DETAIL:   {}", d)); }
        if let Some(h) = db.hint()      { parts.push(format!("HINT:     {}", h)); }
        if let Some(p) = db.position()  { parts.push(format!("POSITION: {:?}", p)); }
        let object = match (db.schema(), db.table(), db.column()) {
            (Some(s), Some(t), Some(c)) => Some(format!("{}.{}.{}", s, t, c)),
            (_,       Some(t), Some(c)) => Some(format!("{}.{}", t, c)),
            (Some(s), Some(t), None)    => Some(format!("{}.{}", s, t)),
            (_,       Some(t), None)    => Some(t.to_string()),
            _ => None,
        };
        if let Some(o) = object         { parts.push(format!("OBJECT:   {}", o)); }
        if let Some(c) = db.constraint(){ parts.push(format!("CONSTRAINT: {}", c)); }
        parts.join("\n")
    } else {
        e.to_string()
    }
}

/// POST /api/pipeline/test-pg-query
/// Body: { query: string, connection_ref?: string }
/// Runs `SELECT COUNT(*) FROM (<query>) AS _sub` against the resolved PG connection.
/// Useful for the pg_extract step's "Test Query" button in the UI.
pub async fn test_pg_query(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let query = body["query"].as_str().unwrap_or("").trim();
    if query.is_empty() { return Err(err(400, "query required")); }

    let empty = json!({});
    let dsn = resolve_pg_conn(&state, &empty, &empty)?
        .ok_or_else(|| err(400, "No PG connection available (mark a data_source as default for type=pg)"))?;

    let count_sql = format!("SELECT COUNT(*) AS c FROM ({}) AS _sub", query);
    let t = Instant::now();

    let (client, conn) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls).await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;
    tokio::spawn(async move { conn.await.ok(); });

    let row = client.query_one(&count_sql, &[]).await
        .map_err(|e| err(400, &format!("Query failed:\n{}", format_pg_err(&e))))?;
    let count: i64 = row.get(0);
    let elapsed = t.elapsed().as_millis() as i64;

    Ok(Json(json!({ "count": count, "duration_ms": elapsed })))
}

/// POST /api/pipeline/preview-pg-query
/// Body: { query: string, limit?: number (default 50; 0 or negative = no limit) }
/// Runs `SELECT * FROM (<query>) AS _sub [LIMIT <n>]` against the resolved PG connection.
/// Returns column names + sample rows for the pg_extract step's "Preview" button.
pub async fn preview_pg_query(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let query = body["query"].as_str().unwrap_or("").trim();
    if query.is_empty() { return Err(err(400, "query required")); }
    let limit = body["limit"].as_i64().unwrap_or(50);

    let empty = json!({});
    let dsn = resolve_pg_conn(&state, &empty, &empty)?
        .ok_or_else(|| err(400, "No PG connection available (mark a data_source as default for type=pg)"))?;

    let preview_sql = if limit > 0 {
        format!("SELECT * FROM ({}) AS _sub LIMIT {}", query, limit)
    } else {
        format!("SELECT * FROM ({}) AS _sub", query)
    };
    let t = Instant::now();

    let (client, conn) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls).await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;
    tokio::spawn(async move { conn.await.ok(); });

    // Wrap in a read-only tx with a short timeout so user queries can't hang the server.
    client.execute("BEGIN", &[]).await.ok();
    client.execute("SET TRANSACTION READ ONLY", &[]).await.ok();
    client.execute("SET LOCAL statement_timeout = '30s'", &[]).await.ok();

    let data_rows = client.query(&preview_sql, &[]).await
        .map_err(|e| err(400, &format!("Query failed:\n{}", format_pg_err(&e))))?;

    let col_names: Vec<String> = if !data_rows.is_empty() {
        data_rows[0].columns().iter().map(|c| c.name().to_string()).collect()
    } else { vec![] };

    let mut rows = Vec::new();
    for r in &data_rows {
        let mut obj = serde_json::Map::new();
        for (i, name) in col_names.iter().enumerate() {
            obj.insert(name.clone(), crate::query::pg_val(r, i));
        }
        rows.push(Value::Object(obj));
    }

    client.execute("COMMIT", &[]).await.ok();
    let elapsed = t.elapsed().as_millis() as i64;

    Ok(Json(json!({
        "rows": rows,
        "columns": col_names,
        "row_count": rows.len(),
        "limit": if limit > 0 { Value::from(limit) } else { Value::Null },
        "duration_ms": elapsed,
    })))
}

/// Resolve PG connection string.
/// Priority: 1) TOML config + Secret Manager  2) data_sources table (legacy fallback)
fn resolve_pg_conn(state: &AppState, contract: &Value, workflow: &Value) -> Result<Option<String>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();

    let conn_name = contract.get("datasource_id").and_then(|v| v.as_str())
        .or_else(|| workflow.get("source").and_then(|s| s.get("connection_ref")).and_then(|v| v.as_str()))
        .unwrap_or("primary");

    let config_path = state.traces.get_setting(&tenant_id, "config_base_path")
        .ok().flatten()
        .unwrap_or_else(|| format!("{}/../config", state.parquet_home));

    if let Some(creds) = crate::db_config::resolve_from_toml(&config_path, &tenant_id, conn_name) {
        tracing::info!("PG connection resolved from TOML config for tenant={}", tenant_id);
        return Ok(Some(creds.to_conn_str()));
    }

    tracing::info!("Falling back to data_sources table for tenant={}", tenant_id);
    let all_sources = state.db.query("SELECT * FROM connections", &[]).unwrap_or_default();
    let is_pg = |c: &&Value| {
        let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        t == "postgres" || t == "pg"
    };
    let is_default = |c: &&Value| {
        c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1
    };
    let source = all_sources.iter().find(|c| c.get("id").and_then(|v| v.as_str()) == Some(conn_name))
        .or_else(|| all_sources.iter().find(|c| is_pg(c) && is_default(c)))
        .or_else(|| all_sources.iter().find(is_pg));

    Ok(source.and_then(|c| {
        let config = c.get("config")?.clone();
        crate::query::pg_conn_str(&config)
    }))
}
