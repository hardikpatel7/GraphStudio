//! Filter-config services. v1 surfaces `resolve_values` — given a filter
//! config id and an optional `context` (parent-column selections), returns
//! the distinct values per filter column from the dimension's master_table
//! parquet source. Cascading rules narrow each column's values by the
//! parents present in `context`.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

use super::error::ServiceError;
use super::ServiceResult;

#[derive(Debug, Deserialize, Default)]
pub struct ResolveValuesArgs {
    /// Map of parent column → selected values. Empty selections are ignored.
    #[serde(default)]
    pub context: HashMap<String, Vec<String>>,
}

fn resolve_dimension_source(
    state: &AppState,
    dimension_ref: &str,
) -> ServiceResult<(String, String)> {
    let dim = state
        .db
        .query_one(
            "SELECT * FROM dimensions WHERE id = ?1",
            &[&dimension_ref as &dyn rusqlite::types::ToSql],
        )
        .map_err(|_| {
            ServiceError::not_found(format!("Dimension '{dimension_ref}' not found"))
        })?;
    let master_table = dim
        .get("master_table")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let source = format!(
        "read_parquet('{}/{master_table}/**/*.parquet')",
        state.parquet_home
    );
    Ok((master_table, source))
}

pub async fn resolve_values(
    state: &AppState,
    id: &str,
    args: ResolveValuesArgs,
) -> ServiceResult<Value> {
    let fc = state
        .db
        .query_one(
            "SELECT * FROM filter_configs WHERE id = ?1",
            &[&id as &dyn rusqlite::types::ToSql],
        )
        .map_err(|_| ServiceError::not_found("Filter config not found"))?;

    let dimension_ref = fc.get("dimension_ref").and_then(|v| v.as_str()).unwrap_or("");
    let (_master_table, parquet_source) = resolve_dimension_source(state, dimension_ref)?;

    let filter_columns = fc
        .get("filter_columns")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();

    // Cascading rules: map of child-column → list of parent columns that
    // narrow it. A filter column whose entry exists in `cascade_parents`
    // and whose parents are present in `args.context` gets a WHERE clause.
    let cascading_rules = fc
        .get("cascading_rules")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let mut cascade_parents: HashMap<String, Vec<String>> = HashMap::new();
    for rule in &cascading_rules {
        if let (Some(trigger), Some(affects)) = (
            rule.get("trigger").and_then(|v| v.as_str()),
            rule.get("affects").and_then(|v| v.as_array()),
        ) {
            for a in affects {
                if let Some(col) = a.as_str() {
                    cascade_parents
                        .entry(col.to_string())
                        .or_default()
                        .push(trigger.to_string());
                }
            }
        }
    }

    let mut result_columns: serde_json::Map<String, Value> = serde_json::Map::new();
    for fc_col in &filter_columns {
        let col_name = fc_col.get("column").and_then(|v| v.as_str()).unwrap_or("");
        if col_name.is_empty() {
            continue;
        }
        // Identifier check matches the handler's prior behavior — skip any
        // column whose name isn't `[A-Za-z0-9_]+`. This is the only SQL-
        // injection guard before interpolating into the WHERE clause.
        if !col_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }

        let parents = cascade_parents.get(col_name).cloned().unwrap_or_default();
        let mut where_parts: Vec<String> = Vec::new();
        for parent_col in &parents {
            if let Some(parent_vals) = args.context.get(parent_col) {
                if parent_vals.is_empty() {
                    continue;
                }
                let quoted: Vec<String> = parent_vals
                    .iter()
                    .map(|v| format!("'{}'", v.replace('\'', "''")))
                    .collect();
                where_parts.push(format!(
                    r#""{parent_col}" IN ({})"#,
                    quoted.join(", ")
                ));
            }
        }
        let query = if where_parts.is_empty() {
            format!("SELECT DISTINCT \"{col_name}\" FROM {parquet_source} ORDER BY 1")
        } else {
            format!(
                "SELECT DISTINCT \"{col_name}\" FROM {parquet_source} WHERE {} ORDER BY 1",
                where_parts.join(" AND ")
            )
        };

        let q = query.clone();
        let values: Vec<String> =
            tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<String>> {
                let db = duckdb::Connection::open_in_memory()?;
                let mut stmt = db.prepare(&q)?;
                let mut rows = stmt.query(duckdb::params![])?;
                let mut vals = Vec::new();
                while let Some(row) = rows.next()? {
                    if let Ok(v) = row.get::<_, String>(0) {
                        vals.push(v);
                    }
                }
                Ok(vals)
            })
            .await
            .map_err(|e| ServiceError::internal(anyhow::anyhow!("task: {e}")))?
            .map_err(ServiceError::internal)?;

        result_columns.insert(col_name.to_string(), json!(values));
    }

    Ok(json!({ "columns": result_columns }))
}
