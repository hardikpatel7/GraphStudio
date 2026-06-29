//! Source read services. The `sources` table stores per-kind source bindings
//! (`pg_query`, `bq_query`, `duckdb_query`, `parquet_glob`, `duckdb_table`,
//! `cdc_pg`, `graph`, `ch_query`). The agent's `list_sources` /
//! `describe_source` tools and the corresponding HTTP routes both call here.

use anyhow::Result;
use serde_json::Value;

use crate::AppState;

/// Columns stored as JSON text in SQLite. The query layer roundtrips JSON
/// inline (parsed in `db::row_value_to_json`) for most cases, but
/// `parse_json_fields` belt-and-braces this for the small set of columns
/// that are known JSON. Kept consistent with the equivalent helper in
/// `handlers::sources` — small enough to duplicate rather than expose
/// handler internals.
const JSON_FIELDS: &[&str] = &["config", "primary_key"];

fn parse_json_fields(mut row: Value) -> Value {
    for field in JSON_FIELDS {
        if let Some(s) = row.get(*field).and_then(|v| v.as_str()) {
            if let Ok(v) = serde_json::from_str::<Value>(s) {
                if let Some(obj) = row.as_object_mut() {
                    obj.insert((*field).to_string(), v);
                }
            }
        }
    }
    row
}

pub async fn list(state: &AppState) -> Result<Vec<Value>> {
    let rows = state
        .db
        .query("SELECT * FROM sources ORDER BY display_name", &[])?;
    Ok(rows.into_iter().map(parse_json_fields).collect())
}

pub async fn describe(state: &AppState, id: &str) -> Result<Value> {
    let row = state.db.query_one(
        "SELECT * FROM sources WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )?;
    Ok(parse_json_fields(row))
}
