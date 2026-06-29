use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use std::collections::HashMap;
use crate::AppState;
use super::{err, stringify};
use std::time::Instant;

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query(
        "SELECT * FROM filter_configs ORDER BY display_name",
        &[],
    )
    .map(|rows| Json(Value::Array(rows)))
    .map_err(|e| err(500, &e.to_string()))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query_one(
        "SELECT * FROM filter_configs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
    .map(Json)
    .map_err(|_| err(404, "Filter config not found"))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let display_name = body["display_name"].as_str().unwrap_or("");
    let dimension_ref = body["dimension_ref"].as_str().unwrap_or("");
    let filter_columns = body.get("filter_columns").map(stringify).unwrap_or_else(|| "[]".into());
    let mandatory_columns = body.get("mandatory_columns").map(stringify).unwrap_or_else(|| "[]".into());
    let cascading_rules = body.get("cascading_rules").map(stringify).unwrap_or_else(|| "[]".into());
    let config = body.get("config").map(stringify).unwrap_or_else(|| "{}".into());

    state.db.execute(
        "INSERT INTO filter_configs (id, display_name, dimension_ref, filter_columns, mandatory_columns, cascading_rules, config) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        &[&id as &dyn rusqlite::types::ToSql, &display_name as _, &dimension_ref as _, &filter_columns as _, &mandatory_columns as _, &cascading_rules as _, &config as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM filter_configs WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "filter_config", "create", "success", &format!("Created filter config '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

    if let Some(v) = body.get("display_name").and_then(|v| v.as_str()) { sets.push("display_name = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("dimension_ref").and_then(|v| v.as_str()) { sets.push("dimension_ref = ?"); vals.push(Box::new(v.to_string())); }
    if body.get("filter_columns").is_some() { sets.push("filter_columns = ?"); vals.push(Box::new(stringify(&body["filter_columns"]))); }
    if body.get("mandatory_columns").is_some() { sets.push("mandatory_columns = ?"); vals.push(Box::new(stringify(&body["mandatory_columns"]))); }
    if body.get("cascading_rules").is_some() { sets.push("cascading_rules = ?"); vals.push(Box::new(stringify(&body["cascading_rules"]))); }
    if body.get("config").is_some() { sets.push("config = ?"); vals.push(Box::new(stringify(&body["config"]))); }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE filter_configs SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Filter config not found")); }

    let row = state.db.query_one("SELECT * FROM filter_configs WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "filter_config", "update", "success", &format!("Updated filter config '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute(
        "DELETE FROM filter_configs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Filter config not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "filter_config", "delete", "success", &format!("Deleted filter config '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}

pub async fn by_dimension(
    State(state): State<Arc<AppState>>,
    Path(dim_ref): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query(
        "SELECT * FROM filter_configs WHERE dimension_ref = ?1 ORDER BY display_name",
        &[&dim_ref as &dyn rusqlite::types::ToSql],
    )
    .map(|rows| Json(Value::Array(rows)))
    .map_err(|e| err(500, &e.to_string()))
}

/// Helper: load a dimension by ref and derive the parquet source path.
/// Returns (master_table, parquet_source_expr) e.g. ("product_master", "read_parquet('.../product_master/**/*.parquet')")
fn resolve_dimension_source(
    state: &AppState,
    dimension_ref: &str,
) -> Result<(String, String), (axum::http::StatusCode, Json<Value>)> {
    let dim = state.db.query_one(
        "SELECT * FROM dimensions WHERE id = ?1",
        &[&dimension_ref as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, &format!("Dimension '{}' not found", dimension_ref)))?;

    let master_table = dim.get("master_table")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let source = format!(
        "read_parquet('{}/{master_table}/**/*.parquet')",
        state.parquet_home
    );
    Ok((master_table, source))
}

/// Resolve dropdown values for each filter column in a filter config.
/// Derives queries automatically from the dimension's master_table — no values_source needed in config.
/// Supports cascading: parent selections narrow child column values.
///
/// POST /filter-configs/{id}/resolve-values
/// Body: { "context": { "l1_name": ["Apparel"] } }  (optional parent selections)
/// Returns: { "columns": { "l1_name": ["Apparel","Footwear",...], "l2_name": ["Mens","Womens",...] } }
pub async fn resolve_values(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    // Decode the `context` block from the raw body into a typed map.
    // Empty selections are dropped to match prior handler behavior.
    let context: HashMap<String, Vec<String>> = body
        .get("context")
        .and_then(|c| c.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| {
                    let vals: Vec<String> = v
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    if vals.is_empty() { None } else { Some((k.clone(), vals)) }
                })
                .collect()
        })
        .unwrap_or_default();

    let args = crate::service::filter_configs::ResolveValuesArgs { context };
    let result = crate::service::filter_configs::resolve_values(&state, &id, args)
        .await
        .map_err(crate::service::error::into_http)?;

    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(
        &state,
        &state.tenant_id,
        "filter_config",
        "resolve_values",
        "success",
        &format!("Resolved values for filter config '{}'", id),
        None,
        Some(elapsed),
    );
    Ok(Json(result))
}

/// Resolve a named filter instance (user selections) to a WHERE clause + CTE.
/// The filter config defines the structure; the client sends the instance (selections).
///
/// POST /filter-configs/{id}/resolve
/// Body (instance): { "selections": { "l1_name": ["Apparel"], "l2_name": ["Mens"] } }
/// Returns: { "where_clause": "\"l1_name\" IN ('Apparel') AND \"l2_name\" IN ('Mens')",
///            "cte": "WITH _filter_product AS (SELECT DISTINCT product_code FROM ... WHERE ...)",
///            "entity_count": 1234 }
pub async fn resolve_filter(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let fc = state.db.query_one(
        "SELECT * FROM filter_configs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "Filter config not found"))?;

    let dimension_ref = fc.get("dimension_ref").and_then(|v| v.as_str()).unwrap_or("dim");

    let selections: HashMap<String, Vec<String>> = body.get("selections")
        .and_then(|s| s.as_object())
        .map(|obj| {
            obj.iter().filter_map(|(k, v)| {
                let vals: Vec<String> = v.as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                if vals.is_empty() { None } else { Some((k.clone(), vals)) }
            }).collect()
        })
        .unwrap_or_default();

    if selections.is_empty() {
        return Ok(Json(json!({ "where_clause": "", "cte": "", "entity_count": null })));
    }

    // Build WHERE clause from the instance selections
    let where_parts: Vec<String> = selections.iter()
        .filter(|(col, _)| col.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .map(|(col, vals)| {
            let quoted: Vec<String> = vals.iter()
                .map(|v| format!("'{}'", v.replace('\'', "''")))
                .collect();
            format!(r#""{}" IN ({})"#, col, quoted.join(", "))
        })
        .collect();

    let where_clause = where_parts.join(" AND ");

    // Load the dimension to get the master_table and entity column (last level)
    let dim = state.db.query_one(
        "SELECT * FROM dimensions WHERE id = ?1",
        &[&dimension_ref as &dyn rusqlite::types::ToSql],
    ).ok();

    let (master_table, entity_column) = if let Some(ref d) = dim {
        let mt = d.get("master_table").and_then(|v| v.as_str()).unwrap_or("unknown");
        let levels = d.get("levels").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        // Entity column is the last level (most granular: product_code, store_code, etc.)
        let ec = levels.last()
            .and_then(|l| l.get("column").and_then(|v| v.as_str()))
            .unwrap_or("id");
        (mt.to_string(), ec.to_string())
    } else {
        ("unknown".to_string(), "id".to_string())
    };

    // Check for explicit resolution_template in config, else auto-generate
    let config = fc.get("config").cloned().unwrap_or(json!({}));
    let resolution_template = config.get("resolution_template").and_then(|v| v.as_str());

    let parquet_home = &state.parquet_home;
    let cte_sql = if let Some(tmpl) = resolution_template {
        // Use explicit template with placeholder substitution
        tmpl.replace("{{dimension_ref}}", dimension_ref)
            .replace("{{master_table}}", &master_table)
            .replace("{{entity_column}}", &entity_column)
            .replace("{{where_clause}}", &where_clause)
            .replace("{PARQUET_HOME}", parquet_home)
            .replace("${PARQUET_HOME}", parquet_home)
    } else {
        // Auto-generate CTE from dimension metadata
        format!(
            "WITH _filter_{dim} AS (SELECT DISTINCT \"{entity}\" FROM read_parquet('{ph}/{mt}/**/*.parquet') WHERE {wc})",
            dim = dimension_ref,
            entity = entity_column,
            ph = parquet_home,
            mt = master_table,
            wc = where_clause,
        )
    };

    // Try to count entities
    let count_sql = format!("{} SELECT COUNT(*) FROM _filter_{}", cte_sql, dimension_ref);
    let count_clone = count_sql.clone();
    let count: Option<i64> = tokio::task::spawn_blocking(move || -> anyhow::Result<i64> {
        let db = duckdb::Connection::open_in_memory()?;
        let count: i64 = db.query_row(&count_clone, [], |row| row.get(0))?;
        Ok(count)
    }).await.ok().and_then(|r| r.ok());

    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "filter_config", "resolve_filter", "success", &format!("Resolved filter for config '{}'", id), None, Some(elapsed));
    Ok(Json(json!({
        "where_clause": where_clause,
        "cte": cte_sql,
        "entity_count": count,
        "dimension_ref": dimension_ref,
        "entity_column": entity_column,
    })))
}
