use anyhow::{Result, anyhow};
use serde_json::{json, Value};
use std::collections::HashMap;

pub mod sql_split;
pub use sql_split::split_statements;

/// Execute a query against DuckDB (in-memory, reading parquet files).
pub async fn duckdb_execute(
    sql: &str,
    parquet_home: &str,
    sort_col: Option<&str>,
    sort_dir: Option<&str>,
    filters: &HashMap<String, Vec<String>>,
    limit: i64,
    offset: i64,
) -> Result<Value> {
    let limit = limit.min(10000).max(1);
    let offset = offset.max(0);

    let resolved = sql
        .replace("{PARQUET_HOME}", parquet_home)
        .replace("${PARQUET_HOME}", parquet_home);

    // Build filter clause
    let filter_clause = build_filter_clause(filters);

    // Build sort
    let sort_clause = match sort_col {
        Some(col) if is_valid_id(col) => {
            let dir = if sort_dir == Some("ASC") { "ASC" } else { "DESC" };
            format!(r#"ORDER BY "{}" {} NULLS LAST"#, col, dir)
        }
        _ => String::new(),
    };

    let wrapped = format!("SELECT * FROM ({}) AS _q {} {} LIMIT {} OFFSET {}",
        resolved, filter_clause, sort_clause, limit, offset);
    let count_sql = format!("SELECT COUNT(*) AS total FROM ({}) AS _q {}", resolved, filter_clause);

    let wrapped_clone = wrapped.clone();
    let count_clone = count_sql.clone();

    tokio::task::spawn_blocking(move || {
        let db = duckdb::Connection::open_in_memory()?;

        // Use Arrow API to avoid the column_name panic issue
        let mut stmt = db.prepare(&wrapped_clone)?;
        let frames = stmt.query_arrow(duckdb::params![])?;

        let mut col_names: Vec<String> = Vec::new();
        let mut rows: Vec<Value> = Vec::new();

        for batch in frames {
            if col_names.is_empty() {
                col_names = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
            }
            let num_rows = batch.num_rows();
            for row_idx in 0..num_rows {
                let mut obj = serde_json::Map::new();
                for (col_idx, name) in col_names.iter().enumerate() {
                    let col = batch.column(col_idx);
                    let val = arrow_val(col, row_idx);
                    obj.insert(name.clone(), val);
                }
                rows.push(Value::Object(obj));
            }
        }

        // Count
        let total: i64 = db.query_row(&count_clone, [], |row| row.get(0)).unwrap_or(rows.len() as i64);

        let columns: Vec<Value> = col_names.iter().map(|n| json!({"name": n})).collect();
        Ok(json!({
            "rows": rows,
            "total": total,
            "columns": columns,
            "sql": wrapped_clone,
        }))
    }).await?
}

/// Execute a query against PostgreSQL.
pub async fn pg_execute(
    conn_str: &str,
    sql: &str,
    sort_col: Option<&str>,
    sort_dir: Option<&str>,
    filters: &HashMap<String, Vec<String>>,
    limit: i64,
    offset: i64,
) -> Result<Value> {
    let limit = limit.min(10000).max(1);
    let offset = offset.max(0);

    let filter_clause = build_filter_clause(filters);
    let sort_clause = match sort_col {
        Some(col) if is_valid_id(col) => {
            let dir = if sort_dir == Some("ASC") { "ASC" } else { "DESC" };
            format!(r#"ORDER BY "{}" {} NULLS LAST"#, col, dir)
        }
        _ => String::new(),
    };

    let wrapped = format!("SELECT * FROM ({}) AS _q {} {} LIMIT {} OFFSET {}",
        sql, filter_clause, sort_clause, limit, offset);
    let count_sql = format!("SELECT COUNT(*) AS total FROM ({}) AS _q {}", sql, filter_clause);

    let (client, connection) = tokio_postgres::connect(conn_str, tokio_postgres::NoTls).await
        .map_err(|e| anyhow!("PG connect: {}", e))?;
    tokio::spawn(async move { connection.await.ok(); });

    // Read-only + timeout
    client.execute("BEGIN", &[]).await?;
    client.execute("SET TRANSACTION READ ONLY", &[]).await?;
    client.execute("SET LOCAL statement_timeout = '30s'", &[]).await?;

    let data_rows = client.query(&wrapped, &[]).await
        .map_err(|e| anyhow!("query: {}", e))?;

    let col_names: Vec<String> = if !data_rows.is_empty() {
        data_rows[0].columns().iter().map(|c| c.name().to_string()).collect()
    } else { vec![] };

    let mut rows = Vec::new();
    for row in &data_rows {
        let mut obj = serde_json::Map::new();
        for (i, name) in col_names.iter().enumerate() {
            obj.insert(name.clone(), pg_val(row, i));
        }
        rows.push(Value::Object(obj));
    }

    // Count
    let total: i64 = match client.query_one(&count_sql, &[]).await {
        Ok(row) => row.get(0),
        Err(_) => rows.len() as i64,
    };

    client.execute("COMMIT", &[]).await.ok();

    let columns: Vec<Value> = col_names.iter().map(|n| json!({"name": n})).collect();
    Ok(json!({
        "rows": rows,
        "total": total,
        "columns": columns,
        "sql": wrapped,
    }))
}

/// Build PG connection string from data source config JSON.
///
/// All fields (host, port, user, password, database) are required — missing
/// or wrong-type returns `None`. No silent defaults: a malformed config
/// shouldn't connect to localhost:5432 by accident. `port` accepts JSON
/// string (`"5433"`) or number (`5433`).
pub fn pg_conn_str(config: &Value) -> Option<String> {
    let host = config.get("host")?.as_str()?;
    let port = config.get("port").and_then(|v| {
        v.as_str().map(String::from)
            .or_else(|| v.as_u64().map(|n| n.to_string()))
    })?;
    let user = config.get("user")?.as_str()?;
    let password = config.get("password")?.as_str()?;
    let database = config.get("database")?.as_str()?;
    Some(format!("host={} port={} user={} password={} dbname={} sslmode=disable connect_timeout=30",
        host, port, user, password, database))
}

fn build_filter_clause(filters: &HashMap<String, Vec<String>>) -> String {
    if filters.is_empty() { return String::new(); }
    let parts: Vec<String> = filters.iter()
        .filter(|(col, vals)| is_valid_id(col) && !vals.is_empty())
        .map(|(col, vals)| {
            let quoted: Vec<String> = vals.iter().map(|v| format!("'{}'", v.replace('\'', "''"))).collect();
            format!(r#""{}" IN ({})"#, col, quoted.join(", "))
        })
        .collect();
    if parts.is_empty() { String::new() } else { format!("WHERE {}", parts.join(" AND ")) }
}

fn is_valid_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

pub fn arrow_to_json(col: &dyn duckdb::arrow::array::Array, row: usize) -> Value {
    arrow_val(col, row)
}

fn arrow_val(col: &dyn duckdb::arrow::array::Array, row: usize) -> Value {
    use duckdb::arrow::array::*;
    use duckdb::arrow::datatypes::DataType;

    if col.is_null(row) { return Value::Null; }

    match col.data_type() {
        DataType::Utf8 => {
            let arr = col.as_any().downcast_ref::<StringArray>().unwrap();
            Value::String(arr.value(row).to_string())
        }
        DataType::LargeUtf8 => {
            let arr = col.as_any().downcast_ref::<LargeStringArray>().unwrap();
            Value::String(arr.value(row).to_string())
        }
        DataType::Int8 => json!(col.as_any().downcast_ref::<Int8Array>().unwrap().value(row)),
        DataType::Int16 => json!(col.as_any().downcast_ref::<Int16Array>().unwrap().value(row)),
        DataType::Int32 => json!(col.as_any().downcast_ref::<Int32Array>().unwrap().value(row)),
        DataType::Int64 => json!(col.as_any().downcast_ref::<Int64Array>().unwrap().value(row)),
        DataType::UInt8 => json!(col.as_any().downcast_ref::<UInt8Array>().unwrap().value(row)),
        DataType::UInt16 => json!(col.as_any().downcast_ref::<UInt16Array>().unwrap().value(row)),
        DataType::UInt32 => json!(col.as_any().downcast_ref::<UInt32Array>().unwrap().value(row)),
        DataType::UInt64 => json!(col.as_any().downcast_ref::<UInt64Array>().unwrap().value(row)),
        DataType::Float32 => json!(col.as_any().downcast_ref::<Float32Array>().unwrap().value(row)),
        DataType::Float64 => json!(col.as_any().downcast_ref::<Float64Array>().unwrap().value(row)),
        DataType::Boolean => Value::Bool(col.as_any().downcast_ref::<BooleanArray>().unwrap().value(row)),
        _ => {
            // Fallback: try to format as string
            let formatted = duckdb::arrow::util::display::array_value_to_string(col, row);
            match formatted {
                Ok(s) => Value::String(s),
                Err(_) => Value::Null,
            }
        }
    }
}

// Keep for PG queries
#[allow(dead_code)]
fn duck_val(row: &duckdb::Row, idx: usize) -> Value {
    if let Ok(Some(s)) = row.get::<_, Option<String>>(idx) { return Value::String(s); }
    if let Ok(Some(n)) = row.get::<_, Option<i64>>(idx) { return json!(n); }
    if let Ok(Some(f)) = row.get::<_, Option<f64>>(idx) { return json!(f); }
    if let Ok(Some(b)) = row.get::<_, Option<bool>>(idx) { return Value::Bool(b); }
    Value::Null
}

pub fn pg_val(row: &tokio_postgres::Row, idx: usize) -> Value {
    if let Ok(Some(s)) = row.try_get::<_, Option<String>>(idx) { return Value::String(s); }
    if let Ok(Some(n)) = row.try_get::<_, Option<i64>>(idx) { return json!(n); }
    if let Ok(Some(f)) = row.try_get::<_, Option<f64>>(idx) { return json!(f); }
    if let Ok(Some(b)) = row.try_get::<_, Option<bool>>(idx) { return Value::Bool(b); }
    if let Ok(Some(n)) = row.try_get::<_, Option<i32>>(idx) { return json!(n); }
    // TIMESTAMP / TIMESTAMPTZ / DATE — without these, every non-null
    // timestamp column was silently coerced to JSON null because none of
    // the primitive try_get calls above match a PG timestamp OID.
    // ISO-8601 is what JSON consumers (mcp-server, FE) expect anyway.
    if let Ok(Some(t)) = row.try_get::<_, Option<chrono::NaiveDateTime>>(idx) {
        return Value::String(t.format("%Y-%m-%dT%H:%M:%S%.f").to_string());
    }
    if let Ok(Some(t)) = row.try_get::<_, Option<chrono::DateTime<chrono::Utc>>>(idx) {
        return Value::String(t.to_rfc3339());
    }
    if let Ok(Some(d)) = row.try_get::<_, Option<chrono::NaiveDate>>(idx) {
        return Value::String(d.format("%Y-%m-%d").to_string());
    }
    Value::Null
}
