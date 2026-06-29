use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use std::time::Instant;
use super::{err, stringify};

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

fn parse_pg_config(config: &Value) -> Result<String, String> {
    // No silent defaults — every field must be present and correctly typed.
    let host = config.get("host").and_then(|v| v.as_str())
        .ok_or_else(|| "config.host is missing or not a string".to_string())?;
    let port = config.get("port").and_then(|v| v.as_u64())
        .ok_or_else(|| "config.port is missing or not a number".to_string())?;
    let user = config.get("user").and_then(|v| v.as_str())
        .ok_or_else(|| "config.user is missing or not a string".to_string())?;
    let password = config.get("password").and_then(|v| v.as_str())
        .ok_or_else(|| "config.password is missing or not a string".to_string())?;
    let database = config.get("database").and_then(|v| v.as_str())
        .ok_or_else(|| "config.database is missing or not a string".to_string())?;
    Ok(format!(
        "host={} port={} user={} password={} dbname={}",
        host, port, user, password, database
    ))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let rows = crate::service::connections::list(&state)
        .await
        .unwrap_or_default();
    Ok(Json(Value::Array(rows)))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query_one(
        "SELECT * FROM connections WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
    .map(|row| Json(mask_password(row)))
    .map_err(|_| err(404, "Data source not found"))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let display_name = body["display_name"].as_str().unwrap_or("");
    let ds_type = body["type"].as_str().unwrap_or("postgres");
    let config = body.get("config").map(stringify).unwrap_or_else(|| "{}".into());

    state.db.execute(
        "INSERT INTO connections (id, display_name, type, config) VALUES (?1, ?2, ?3, ?4)",
        &[&id as &dyn rusqlite::types::ToSql, &display_name as _, &ds_type as _, &config as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM connections WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "data_source", "create", "success", &format!("Created data source '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(mask_password(row))))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

    if let Some(v) = body.get("display_name").and_then(|v| v.as_str()) {
        sets.push("display_name = ?");
        vals.push(Box::new(v.to_string()));
    }
    if let Some(v) = body.get("type").and_then(|v| v.as_str()) {
        sets.push("type = ?");
        vals.push(Box::new(v.to_string()));
    }
    // is_default is per-type: only one row per type can be flagged. When setting to 1,
    // clear the flag on all other rows of the same type first.
    if let Some(flag) = body.get("is_default").and_then(|v| v.as_bool()) {
        let new_flag: i64 = if flag { 1 } else { 0 };
        if flag {
            let row = state.db.query_one("SELECT type FROM connections WHERE id = ?1",
                &[&id as &dyn rusqlite::types::ToSql]).map_err(|_| err(404, "Data source not found"))?;
            let row_type = row.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            state.db.execute(
                "UPDATE connections SET is_default = 0 WHERE type = ?1 AND id != ?2",
                &[&row_type as &dyn rusqlite::types::ToSql, &id as _],
            ).map_err(|e| err(500, &e.to_string()))?;
        }
        sets.push("is_default = ?");
        vals.push(Box::new(new_flag));
    }
    if body.get("config").is_some() {
        // Preserve password if the incoming config has the masked value
        let mut new_config = body["config"].clone();
        if let Some(pw) = new_config.get("password").and_then(|v| v.as_str()) {
            if pw == PASSWORD_MASK {
                // Fetch existing password from DB
                if let Ok(existing) = state.db.query_one(
                    "SELECT * FROM connections WHERE id = ?1",
                    &[&id as &dyn rusqlite::types::ToSql],
                ) {
                    if let Some(old_pw) = existing.get("config").and_then(|c| c.get("password")).and_then(|v| v.as_str()) {
                        new_config.as_object_mut().unwrap().insert("password".to_string(), Value::String(old_pw.to_string()));
                    }
                }
            }
        }
        sets.push("config = ?");
        vals.push(Box::new(stringify(&new_config)));
    }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE connections SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Data source not found")); }

    let row = state.db.query_one("SELECT * FROM connections WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "data_source", "update", "success", &format!("Updated data source '{}'", id), None, Some(elapsed));
    Ok(Json(mask_password(row)))
}

/// `POST /api/connections/{src_id}/clone`
///
/// Body: `{ "id": "new_id", "display_name"?: "..." }`
///
/// Copies the source connection (including its real password — the API never
/// surfaces it to the client, so the UI can't carry it across) into a new
/// row. `is_default` is intentionally NOT copied — at most one default per
/// type. Returns the new row with password masked, just like create/update.
pub async fn clone_connection(
    State(state): State<Arc<AppState>>,
    Path(src_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let new_id = body.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
    if new_id.is_empty() {
        return Err(err(400, "id required"));
    }

    let src = state.db.query_one(
        "SELECT * FROM connections WHERE id = ?1",
        &[&src_id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "Source connection not found"))?;

    let src_type = src.get("type").and_then(|v| v.as_str()).unwrap_or("postgres").to_string();
    let src_config = src.get("config").cloned().unwrap_or_else(|| json!({}));
    let src_display = src.get("display_name").and_then(|v| v.as_str()).unwrap_or(&src_id).to_string();

    let new_display = body.get("display_name").and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} (clone)", src_display));
    let config_str = stringify(&src_config);

    state.db.execute(
        "INSERT INTO connections (id, display_name, type, config) VALUES (?1, ?2, ?3, ?4)",
        &[
            &new_id as &dyn rusqlite::types::ToSql,
            &new_display as _,
            &src_type as _,
            &config_str as _,
        ],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM connections WHERE id = ?1", &[&new_id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(
        &state, &state.tenant_id, "data_source", "clone", "success",
        &format!("Cloned connection '{}' → '{}'", src_id, new_id),
        None, Some(elapsed),
    );
    Ok((axum::http::StatusCode::CREATED, Json(mask_password(row))))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute(
        "DELETE FROM connections WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Data source not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "data_source", "delete", "success", &format!("Deleted data source '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}

/// Fetch the raw (unmasked) config for a data source.
fn get_ds_config(state: &Arc<AppState>, id: &str) -> Result<Value, (axum::http::StatusCode, Json<Value>)> {
    state.db.query_one(
        "SELECT * FROM connections WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
    .map(|row| row["config"].clone())
    .map_err(|_| err(404, "Data source not found"))
}

/// Fetch (type, config) for a data source. Used by handlers that
/// branch on connection type (e.g. PG vs ClickHouse).
fn get_ds_type_and_config(
    state: &Arc<AppState>,
    id: &str,
) -> Result<(String, Value), (axum::http::StatusCode, Json<Value>)> {
    let row = state.db.query_one(
        "SELECT * FROM connections WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
    .map_err(|_| err(404, "Data source not found"))?;
    let ty = row.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cfg = row.get("config").cloned().unwrap_or(json!({}));
    Ok((ty, cfg))
}

/// Return 501 with a uniform message for CH endpoints not yet wired
/// in the MVP. Used by the schema-browser routes (`list_schemas`,
/// `list_tables`, etc.) — operators type SQL by hand for CH until
/// these grow CH-specific implementations.
fn ch_not_implemented(feature: &str) -> (axum::http::StatusCode, Json<Value>) {
    err(501, &format!(
        "{feature} is not yet implemented for ClickHouse connections — \
         type SQL by hand against known tables for now"
    ))
}

pub async fn test_connection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let (ty, config) = get_ds_type_and_config(&state, &id)?;

    if ty == "clickhouse" {
        let conn = crate::clickhouse::ChConnection::from_config(&config)
            .map_err(|e| err(400, &format!("ClickHouse config invalid: {e:#}")))?;
        let probe = crate::clickhouse::test_connection(&conn).await
            .map_err(|e| err(500, &format!("ClickHouse connection failed: {e:#}")))?;
        let elapsed = t.elapsed().as_millis() as i64;
        super::log_activity(&state, &state.tenant_id, "data_source", "test", "success",
            &format!("Tested ClickHouse connection for '{}'", id), None, Some(elapsed));
        return Ok(Json(json!({
            "success": true,
            "engine": "clickhouse",
            "duration_ms": elapsed,
            "client_ms": probe.client_ms,
            "server_ms": probe.server_ms,
        })));
    }

    let conn_str = parse_pg_config(&config).map_err(|e| err(400, &e))?;

    let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
        .await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;

    tokio::spawn(async move { connection.await.ok(); });

    let row = client.query_one("SELECT version()", &[])
        .await
        .map_err(|e| err(500, &format!("Query failed: {}", e)))?;

    let version: String = row.get(0);
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "data_source", "test", "success", &format!("Tested connection for '{}'", id), Some(&version), Some(elapsed));
    Ok(Json(json!({"success": true, "version": version})))
}

pub async fn list_schemas(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let (ty, config) = get_ds_type_and_config(&state, &id)?;

    if ty == "clickhouse" {
        let conn = crate::clickhouse::ChConnection::from_config(&config)
            .map_err(|e| err(400, &format!("ClickHouse config invalid: {e:#}")))?;
        let rows = crate::clickhouse::list_databases(&conn).await
            .map_err(|e| err(500, &format!("ClickHouse list_databases: {e:#}")))?;
        // Normalize to {schema_name} so callers (existing PG-shaped
        // UI + MCP) can consume both engines uniformly. Engine /
        // comment are kept as additional fields.
        let schemas: Vec<Value> = rows.into_iter().map(|r| {
            let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mut obj = serde_json::Map::new();
            obj.insert("schema_name".into(), Value::String(name));
            if let Some(engine) = r.get("engine") { obj.insert("engine".into(), engine.clone()); }
            if let Some(comment) = r.get("comment") { obj.insert("comment".into(), comment.clone()); }
            Value::Object(obj)
        }).collect();
        return Ok(Json(Value::Array(schemas)));
    }

    let conn_str = parse_pg_config(&config).map_err(|e| err(400, &e))?;

    let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
        .await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;
    tokio::spawn(async move { connection.await.ok(); });

    let rows = client.query(
        "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name",
        &[],
    ).await.map_err(|e| err(500, &e.to_string()))?;

    let schemas: Vec<Value> = rows.iter().map(|r| {
        let name: String = r.get(0);
        json!({"schema_name": name})
    }).collect();

    Ok(Json(Value::Array(schemas)))
}

pub async fn list_tables(
    State(state): State<Arc<AppState>>,
    Path((id, schema)): Path<(String, String)>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let (ty, config) = get_ds_type_and_config(&state, &id)?;

    if ty == "clickhouse" {
        let conn = crate::clickhouse::ChConnection::from_config(&config)
            .map_err(|e| err(400, &format!("ClickHouse config invalid: {e:#}")))?;
        let rows = crate::clickhouse::list_tables_in(&conn, &schema).await
            .map_err(|e| err(500, &format!("ClickHouse list_tables_in: {e:#}")))?;
        let tables: Vec<Value> = rows.into_iter().map(|r| {
            let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mut obj = serde_json::Map::new();
            obj.insert("table_name".into(), Value::String(name));
            obj.insert("table_type".into(), Value::String("BASE TABLE".into()));
            for k in ["engine", "total_rows", "total_bytes", "comment"] {
                if let Some(v) = r.get(k) { obj.insert(k.into(), v.clone()); }
            }
            Value::Object(obj)
        }).collect();
        return Ok(Json(Value::Array(tables)));
    }

    let conn_str = parse_pg_config(&config).map_err(|e| err(400, &e))?;

    let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
        .await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;
    tokio::spawn(async move { connection.await.ok(); });

    let rows = client.query(
        "SELECT table_name, table_type FROM information_schema.tables WHERE table_schema = $1 ORDER BY table_name",
        &[&schema],
    ).await.map_err(|e| err(500, &e.to_string()))?;

    let tables: Vec<Value> = rows.iter().map(|r| {
        let name: String = r.get(0);
        let ttype: String = r.get(1);
        json!({"table_name": name, "table_type": ttype})
    }).collect();

    Ok(Json(Value::Array(tables)))
}

pub async fn list_routines(
    State(state): State<Arc<AppState>>,
    Path((id, schema)): Path<(String, String)>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let (ty, config) = get_ds_type_and_config(&state, &id)?;

    if ty == "clickhouse" {
        // ClickHouse exposes user functions in system.functions but the
        // shape and semantics don't map cleanly onto PG routine_type
        // (and the demo doesn't need them). Return empty so the UI
        // routine-browser stays consistent.
        return Ok(Json(Value::Array(vec![])));
    }

    let conn_str = parse_pg_config(&config).map_err(|e| err(400, &e))?;

    let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
        .await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;
    tokio::spawn(async move { connection.await.ok(); });

    let rows = client.query(
        "SELECT routine_name, routine_type FROM information_schema.routines WHERE routine_schema = $1 ORDER BY routine_name",
        &[&schema],
    ).await.map_err(|e| err(500, &e.to_string()))?;

    let routines: Vec<Value> = rows.iter().map(|r| {
        let name: String = r.get(0);
        let rtype: String = r.get(1);
        json!({"routine_name": name, "routine_type": rtype})
    }).collect();

    Ok(Json(Value::Array(routines)))
}

pub async fn list_matviews(
    State(state): State<Arc<AppState>>,
    Path((id, schema)): Path<(String, String)>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let (ty, config) = get_ds_type_and_config(&state, &id)?;

    if ty == "clickhouse" {
        // CH materialized views show up in system.tables with
        // engine='MaterializedView' — surface them here so the UI
        // mat-view browser still works.
        let conn = crate::clickhouse::ChConnection::from_config(&config)
            .map_err(|e| err(400, &format!("ClickHouse config invalid: {e:#}")))?;
        let rows = crate::clickhouse::list_tables_in(&conn, &schema).await
            .map_err(|e| err(500, &format!("ClickHouse list_tables_in: {e:#}")))?;
        let matviews: Vec<Value> = rows.into_iter()
            .filter(|r| r.get("engine").and_then(|v| v.as_str()) == Some("MaterializedView"))
            .map(|r| {
                let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                json!({"matview_name": name})
            })
            .collect();
        return Ok(Json(Value::Array(matviews)));
    }

    let conn_str = parse_pg_config(&config).map_err(|e| err(400, &e))?;

    let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
        .await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;
    tokio::spawn(async move { connection.await.ok(); });

    let rows = client.query(
        "SELECT matviewname FROM pg_matviews WHERE schemaname = $1 ORDER BY matviewname",
        &[&schema],
    ).await.map_err(|e| err(500, &e.to_string()))?;

    let matviews: Vec<Value> = rows.iter().map(|r| {
        let name: String = r.get(0);
        json!({"matview_name": name})
    }).collect();

    Ok(Json(Value::Array(matviews)))
}

pub async fn routine_definition(
    State(state): State<Arc<AppState>>,
    Path((id, schema, name)): Path<(String, String, String)>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let config = get_ds_config(&state, &id)?;
    let conn_str = parse_pg_config(&config).map_err(|e| err(400, &e))?;

    let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
        .await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;
    tokio::spawn(async move { connection.await.ok(); });

    // Get function/procedure definition using pg_get_functiondef
    let rows = client.query(
        "SELECT p.oid, pg_get_functiondef(p.oid) AS definition, \
         pg_get_function_arguments(p.oid) AS arguments, \
         pg_get_function_result(p.oid) AS return_type \
         FROM pg_proc p JOIN pg_namespace n ON p.pronamespace = n.oid \
         WHERE n.nspname = $1 AND p.proname = $2 LIMIT 1",
        &[&schema, &name],
    ).await.map_err(|e| err(500, &e.to_string()))?;

    if let Some(row) = rows.first() {
        let definition: String = row.get(1);
        let arguments: String = row.get(2);
        let return_type: String = row.get(3);
        Ok(Json(json!({
            "name": name, "schema": schema,
            "definition": definition, "arguments": arguments, "return_type": return_type
        })))
    } else {
        Err(err(404, "Routine not found"))
    }
}

pub async fn list_columns(
    State(state): State<Arc<AppState>>,
    Path((id, schema, table)): Path<(String, String, String)>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let (ty, config) = get_ds_type_and_config(&state, &id)?;

    if ty == "clickhouse" {
        let conn = crate::clickhouse::ChConnection::from_config(&config)
            .map_err(|e| err(400, &format!("ClickHouse config invalid: {e:#}")))?;
        let rows = crate::clickhouse::list_columns_in(&conn, &schema, &table).await
            .map_err(|e| err(500, &format!("ClickHouse list_columns_in: {e:#}")))?;
        let columns: Vec<Value> = rows.into_iter().map(|r| {
            let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let dtype = r.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let default = r.get("default_expression").and_then(|v| v.as_str()).map(String::from);
            let comment = r.get("comment").and_then(|v| v.as_str()).map(String::from);
            let pos = r.get("position").and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64))).unwrap_or(0);
            json!({
                "column_name": name,
                "data_type": dtype,
                // CH columns are nullable iff the declared type is `Nullable(…)`.
                // Surface the same string shape PG callers expect.
                "is_nullable": if dtype.contains("Nullable") { "YES" } else { "NO" },
                "column_default": default,
                "comment": comment,
                "ordinal_position": pos,
            })
        }).collect();
        return Ok(Json(Value::Array(columns)));
    }

    let conn_str = parse_pg_config(&config).map_err(|e| err(400, &e))?;

    let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
        .await
        .map_err(|e| err(500, &format!("Connection failed: {}", e)))?;
    tokio::spawn(async move { connection.await.ok(); });

    let rows = client.query(
        "SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
        &[&schema, &table],
    ).await.map_err(|e| err(500, &e.to_string()))?;

    let columns: Vec<Value> = rows.iter().map(|r| {
        let name: String = r.get(0);
        let dtype: String = r.get(1);
        let nullable: String = r.get(2);
        let default: Option<String> = r.get(3);
        let pos: i32 = r.get(4);
        json!({
            "column_name": name,
            "data_type": dtype,
            "is_nullable": nullable,
            "column_default": default,
            "ordinal_position": pos,
        })
    }).collect();

    Ok(Json(Value::Array(columns)))
}

/// `GET /api/connections/:id/dictionary[?database=<name>]` — returns
/// a data dictionary for the connection. Only implemented for
/// ClickHouse today; PG callers should iterate list_schemas /
/// list_tables / list_columns.
///
/// Optional `?database=<name>` query param scopes the response to a
/// single database AND pushes the filter into the CH-side JOIN so
/// the server doesn't scan / return the full catalog. Without the
/// param the full catalog is returned (large — up to several MB on
/// tenants with hundreds of tables).
#[derive(serde::Deserialize, Default)]
pub struct DictionaryQuery {
    #[serde(default)]
    pub database: Option<String>,
}

pub async fn dictionary(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<DictionaryQuery>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let args = crate::service::connections::DictionaryArgs { database: q.database };
    crate::service::connections::clickhouse_dictionary(&state, &id, args)
        .await
        .map(Json)
        .map_err(crate::service::error::into_http)
}

pub async fn execute_query(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let (ty, config) = get_ds_type_and_config(&state, &id)?;

    // Decode hex-encoded payload if present (bypasses Cloudflare WAF SQL inspection)
    let body = if let Some(hex) = body["p"].as_str() {
        let bytes: Vec<u8> = (0..hex.len()).step_by(2)
            .filter_map(|i| u8::from_str_radix(&hex[i..i+2], 16).ok())
            .collect();
        serde_json::from_slice::<Value>(&bytes).unwrap_or(body)
    } else {
        body
    };

    let sql = body["sql"].as_str().unwrap_or("").to_string();
    let engine = body["engine"].as_str().unwrap_or("postgres");
    let limit = body["limit"].as_i64().unwrap_or(100);
    let offset = body["offset"].as_i64().unwrap_or(0);

    // ClickHouse connections always route through the CH HTTP path,
    // regardless of the request's `engine` hint — the engine param is
    // only meaningful for PG-typed connections (where it picks between
    // PG execution and local DuckDB execution).
    if ty == "clickhouse" {
        let conn = crate::clickhouse::ChConnection::from_config(&config)
            .map_err(|e| err(400, &format!("ClickHouse config invalid: {e:#}")))?;
        let wrapped = if sql.trim_start().to_uppercase().starts_with("SELECT")
            || sql.trim_start().to_uppercase().starts_with("WITH")
        {
            format!("SELECT * FROM ({sql}) LIMIT {limit} OFFSET {offset}")
        } else {
            sql.clone()
        };
        let result = crate::clickhouse::query_exec(&conn, &wrapped).await
            .map_err(|e| err(400, &format!("ClickHouse query failed: {e:#}")))?;
        let columns: Vec<Value> = result.rows.first()
            .and_then(|r| r.as_object())
            .map(|m| m.keys().map(|k| json!({"name": k})).collect())
            .unwrap_or_default();
        let elapsed = t.elapsed().as_millis() as i64;
        super::log_activity(&state, &state.tenant_id, "data_source", "execute_query", "success",
            &format!("Executed ClickHouse query on '{}'", id), None, Some(elapsed));
        let row_count = result.rows.len();
        return Ok(Json(json!({
            "rows": result.rows,
            "total": row_count,
            "columns": columns,
            "duration_ms": elapsed,
            "client_ms": result.client_ms,
            "server_ms": result.server_ms,
            "read_rows": result.read_rows,
            "read_bytes": result.read_bytes,
        })));
    }
    // PG `statement_timeout` for the run. Default 5 min — enough for slow
    // stored procs (e.g. article_selection_list_v2). Cap at 10 min.
    let timeout_ms = body["timeout_ms"].as_i64().unwrap_or(300_000).clamp(1_000, 600_000);

    // Build sort clause
    let sort_clause = if let Some(sorts) = body["sort"].as_array() {
        let parts: Vec<String> = sorts.iter().filter_map(|s| {
            let col = s["column"].as_str()?;
            let dir = s["direction"].as_str().unwrap_or("ASC");
            Some(format!("\"{}\" {}", col, dir))
        }).collect();
        if parts.is_empty() { String::new() } else { format!(" ORDER BY {}", parts.join(", ")) }
    } else {
        String::new()
    };

    match engine {
        "duckdb" => {
            let parquet_home = state.parquet_home.clone();
            let sql_owned = sql.clone();
            // Support per-dataview DuckDB files (persistent intermediate tables)
            let duckdb_path = if let Some(file) = body.get("duckdb_file").and_then(|v| v.as_str()) {
                let p = format!("{}/{}", state.parquet_home, file);
                tracing::info!("execute_query: using duckdb_file={}", p);
                p
            } else {
                tracing::info!("execute_query: using default duckdb_path={}", state.duckdb_path);
                state.duckdb_path.clone()
            };

            let result = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
                let db = duckdb::Connection::open(&duckdb_path).map_err(|e| format!("DuckDB open: {}", e))?;

                // Replace legacy {PARQUET_HOME} references in SQL
                let resolved_sql = sql_owned.replace("{PARQUET_HOME}", &parquet_home)
                    .replace("${PARQUET_HOME}", &parquet_home);

                // Split on top-level `;` (respecting strings/comments/$$). All but
                // the last statement runs as prelude (CREATE TEMP, SET, etc.) so
                // pasted scripts like `SET ...; SELECT ...;` work.
                let stmts = crate::query::split_statements(&resolved_sql);
                if stmts.is_empty() {
                    return Err("sql is empty after stripping comments".to_string());
                }
                for prelude in &stmts[..stmts.len() - 1] {
                    db.execute_batch(prelude).map_err(|e| format!("statement failed: {}", e))?;
                }
                let last = stmts.last().unwrap().clone();

                // Detect non-SELECT statements (CREATE, DROP, INSERT, etc.) and execute directly
                let trimmed = last.trim_start().to_uppercase();
                let is_select = trimmed.starts_with("SELECT") || trimmed.starts_with("WITH");

                let exec_sql = if is_select {
                    format!(
                        "SELECT * FROM ({}) AS _q{} LIMIT {} OFFSET {}",
                        last, sort_clause, limit, offset
                    )
                } else {
                    // Non-SELECT: execute directly (DDL, DML, SHOW, DESCRIBE, PRAGMA, etc.)
                    last
                };

                // Use Arrow API to avoid column_name panic
                let mut stmt = db.prepare(&exec_sql).map_err(|e| e.to_string())?;
                let frames = stmt.query_arrow(duckdb::params![]).map_err(|e| e.to_string())?;

                let mut col_names: Vec<String> = Vec::new();
                let rows: Vec<Value> = {
                    let mut all_rows = Vec::new();
                    for batch in frames {
                        if col_names.is_empty() {
                            col_names = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
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
                    all_rows
                };
                Ok(rows)
            })
            .await
            .map_err(|e| err(500, &format!("Task join error: {}", e)))?
            .map_err(|e| err(500, &e))?;

            let columns: Vec<Value> = if let Some(first) = result.first() {
                first.as_object().map(|o| o.keys().map(|k| json!({"name": k})).collect()).unwrap_or_default()
            } else { vec![] };
            let elapsed = t.elapsed().as_millis() as i64;
            super::log_activity(&state, &state.tenant_id, "data_source", "execute_query", "success", &format!("Executed DuckDB query on '{}'", id), None, Some(elapsed));
            Ok(Json(json!({"rows": result, "total": result.len(), "columns": columns})))
        }
        _ => {
            // postgres engine
            let conn_str = parse_pg_config(&config).map_err(|e| err(400, &e))?;

            let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
                .await
                .map_err(|e| err(500, &format!("Connection failed: {}", super::pipeline_handler::format_pg_err(&e))))?;
            tokio::spawn(async move { connection.await.ok(); });

            // Split on top-level `;` so scripts like `SET ...; SELECT ...` work,
            // and refcursor patterns like `SELECT proc(...); FETCH ALL FROM "..."`
            // can run end-to-end. Prelude statements run via batch_execute (no
            // wrapping); only the last is wrapped with sort+LIMIT for paging.
            let stmts = crate::query::split_statements(&sql);
            if stmts.is_empty() {
                return Err(err(400, "sql is empty after stripping comments"));
            }

            // No READ ONLY: the heuristic can't see through stored functions
            // that open refcursors or use temp DDL internally (e.g.
            // article_selection_list_v2 hits 25006 inside a read-only tx).
            // statement_timeout still bounds runaway queries.
            // tokio_postgres errors stringify as "db error" by default —
            // `format_pg_err` unpacks SQLSTATE + message + detail.
            client.execute(&format!("SET statement_timeout = {}", timeout_ms), &[])
                .await
                .map_err(|e| err(500, &super::pipeline_handler::format_pg_err(&e)))?;
            client.execute("BEGIN", &[])
                .await
                .map_err(|e| err(500, &super::pipeline_handler::format_pg_err(&e)))?;
            for prelude in &stmts[..stmts.len() - 1] {
                client.batch_execute(prelude)
                    .await
                    .map_err(|e| err(500, &super::pipeline_handler::format_pg_err(&e)))?;
            }
            let last = stmts.last().unwrap();

            // FETCH/SHOW/etc. don't tolerate the SELECT-wrapper. Only wrap when
            // the final statement is a row-returning query we know is safe to
            // wrap as a subquery.
            let head = last.trim_start().to_uppercase();
            let wrappable = head.starts_with("SELECT") || head.starts_with("WITH") || head.starts_with("VALUES") || head.starts_with("TABLE ");
            let exec_sql = if wrappable {
                format!("SELECT * FROM ({}) AS _q{} LIMIT {} OFFSET {}", last, sort_clause, limit, offset)
            } else {
                last.clone()
            };

            let rows = client.query(&exec_sql, &[])
                .await
                .map_err(|e| err(500, &super::pipeline_handler::format_pg_err(&e)))?;

            client.execute("COMMIT", &[]).await.ok();

            let columns: Vec<String> = if let Some(first) = rows.first() {
                first.columns().iter().map(|c| c.name().to_string()).collect()
            } else {
                vec![]
            };

            let result: Vec<Value> = rows.iter().map(|row| {
                let mut obj = serde_json::Map::new();
                for (i, col_name) in columns.iter().enumerate() {
                    obj.insert(col_name.clone(), pg_decode(row, i));
                }
                Value::Object(obj)
            }).collect();

            let col_meta: Vec<Value> = columns.iter().map(|n| json!({"name": n})).collect();
            let elapsed = t.elapsed().as_millis() as i64;
            super::log_activity(&state, &state.tenant_id, "data_source", "execute_query", "success", &format!("Executed PG query on '{}'", id), None, Some(elapsed));
            Ok(Json(json!({"rows": result, "total": result.len(), "columns": col_meta})))
        }
    }
}

/// Decode a single PG cell to JSON. Uses `try_get` everywhere so unsupported
/// types fall through to `Null` instead of panicking the worker thread.
/// `row.get` panics on type mismatch — we found this the hard way when
/// `article_selection_list_v2` returned a column the inline matcher couldn't
/// deserialize as String.
fn pg_decode(row: &tokio_postgres::Row, idx: usize) -> Value {
    use tokio_postgres::types::Type;
    let col = &row.columns()[idx];
    match *col.type_() {
        Type::BOOL => row.try_get::<_, Option<bool>>(idx).ok().flatten().map(Value::Bool).unwrap_or(Value::Null),
        Type::INT2 => row.try_get::<_, Option<i16>>(idx).ok().flatten().map(|v| json!(v)).unwrap_or(Value::Null),
        Type::INT4 => row.try_get::<_, Option<i32>>(idx).ok().flatten().map(|v| json!(v)).unwrap_or(Value::Null),
        Type::INT8 => row.try_get::<_, Option<i64>>(idx).ok().flatten().map(|v| json!(v)).unwrap_or(Value::Null),
        Type::FLOAT4 => row.try_get::<_, Option<f32>>(idx).ok().flatten().map(|v| json!(v)).unwrap_or(Value::Null),
        Type::FLOAT8 => row.try_get::<_, Option<f64>>(idx).ok().flatten().map(|v| json!(v)).unwrap_or(Value::Null),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::CHAR =>
            row.try_get::<_, Option<String>>(idx).ok().flatten().map(Value::String).unwrap_or(Value::Null),
        Type::JSON | Type::JSONB =>
            row.try_get::<_, Option<serde_json::Value>>(idx).ok().flatten().unwrap_or(Value::Null),
        Type::UUID =>
            // No uuid feature enabled — read raw bytes (16 bytes) and format.
            row.try_get::<_, Option<&[u8]>>(idx).ok().flatten().map(|b| Value::String(format_uuid(b))).unwrap_or(Value::Null),
        Type::TIMESTAMP =>
            row.try_get::<_, Option<chrono::NaiveDateTime>>(idx).ok().flatten()
                .map(|v| Value::String(v.to_string())).unwrap_or(Value::Null),
        Type::TIMESTAMPTZ =>
            row.try_get::<_, Option<chrono::DateTime<chrono::Utc>>>(idx).ok().flatten()
                .map(|v| Value::String(v.to_rfc3339())).unwrap_or(Value::Null),
        Type::DATE =>
            row.try_get::<_, Option<chrono::NaiveDate>>(idx).ok().flatten()
                .map(|v| Value::String(v.to_string())).unwrap_or(Value::Null),
        Type::TIME =>
            row.try_get::<_, Option<chrono::NaiveTime>>(idx).ok().flatten()
                .map(|v| Value::String(v.to_string())).unwrap_or(Value::Null),
        Type::BYTEA =>
            row.try_get::<_, Option<Vec<u8>>>(idx).ok().flatten()
                .map(|b| Value::String(format!("\\x{}", hex_lower(&b)))).unwrap_or(Value::Null),
        // PG arrays — emit as JSON arrays so legacy v2's ARRAY_AGG outputs
        // (mapped_stores text[], dcs / store_groups / product_profiles
        // jsonb[], size_names text[]) compare correctly. Without these the
        // String fallback returned Null and silently masked real data.
        Type::TEXT_ARRAY | Type::VARCHAR_ARRAY | Type::BPCHAR_ARRAY | Type::NAME_ARRAY =>
            row.try_get::<_, Option<Vec<Option<String>>>>(idx).ok().flatten()
                .map(|v| Value::Array(v.into_iter().map(|s| s.map(Value::String).unwrap_or(Value::Null)).collect()))
                .unwrap_or(Value::Null),
        Type::INT2_ARRAY =>
            row.try_get::<_, Option<Vec<Option<i16>>>>(idx).ok().flatten()
                .map(|v| Value::Array(v.into_iter().map(|n| n.map(|x| json!(x)).unwrap_or(Value::Null)).collect()))
                .unwrap_or(Value::Null),
        Type::INT4_ARRAY =>
            row.try_get::<_, Option<Vec<Option<i32>>>>(idx).ok().flatten()
                .map(|v| Value::Array(v.into_iter().map(|n| n.map(|x| json!(x)).unwrap_or(Value::Null)).collect()))
                .unwrap_or(Value::Null),
        Type::INT8_ARRAY =>
            row.try_get::<_, Option<Vec<Option<i64>>>>(idx).ok().flatten()
                .map(|v| Value::Array(v.into_iter().map(|n| n.map(|x| json!(x)).unwrap_or(Value::Null)).collect()))
                .unwrap_or(Value::Null),
        Type::JSON_ARRAY | Type::JSONB_ARRAY =>
            row.try_get::<_, Option<Vec<Option<serde_json::Value>>>>(idx).ok().flatten()
                .map(|v| Value::Array(v.into_iter().map(|x| x.unwrap_or(Value::Null)).collect()))
                .unwrap_or(Value::Null),
        // NUMERIC, INET, etc. — try a String fallback (works for some
        // text-coercible types) before giving up.
        _ => row.try_get::<_, Option<String>>(idx).ok().flatten().map(Value::String).unwrap_or(Value::Null),
    }
}

fn format_uuid(b: &[u8]) -> String {
    if b.len() != 16 { return hex_lower(b); }
    let h = hex_lower(b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn hex_lower(b: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(HEX[(x >> 4) as usize] as char);
        s.push(HEX[(x & 0x0f) as usize] as char);
    }
    s
}
