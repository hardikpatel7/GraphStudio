//! Upsert `<tenant_data_dir>/dataviews/<id>.toml` files into the
//! `dataviews` SQLite table at boot. Pattern mirrors `seed::sources`:
//! missing dir → skip, per-file failure → warn, idempotent on `id`.
//!
//! TOML shape:
//!   id           = "dv_xxx"
//!   display_name = "..."
//!
//!   [source]
//!   source_id = "src_xxx"        # required; bound source must exist
//!
//!   [[columns]]
//!   name = "article"
//!   type = "VARCHAR"
//!   sortable   = true
//!   searchable = true
//!   visible    = true
//!   group      = "identity"
//!
//!   # ...more columns...
//!
//!   # Optional structural fields — empty by default:
//!   dimensions        = []
//!   sort              = []
//!   cascading_filters = []
//!   contract          = {}
//!   backend_workflow  = {}
//!
//! The on-disk shape is intentionally narrower than the in-row JSON: we
//! collapse `source = {type: "source", config: {source_id}}` to just
//! `[source] source_id = "..."` because every modern DataView binds that
//! way (per resolve_source_binding contract in dataview_source.rs).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

#[derive(Debug, Deserialize)]
struct SourceBindingFile {
    source_id: String,
}

#[derive(Debug, Deserialize)]
struct DataViewFile {
    id: String,
    display_name: String,
    source: SourceBindingFile,
    #[serde(default)]
    columns: Vec<toml::Value>,
    #[serde(default)]
    dimensions: Option<toml::Value>,
    #[serde(default)]
    sort: Option<toml::Value>,
    #[serde(default)]
    cascading_filters: Option<toml::Value>,
    #[serde(default)]
    contract: Option<toml::Value>,
    #[serde(default)]
    backend_workflow: Option<toml::Value>,
}

pub fn seed_dataviews(state: &Arc<AppState>) {
    let dir = Path::new(&state.data_dir).join("dataviews");
    let entries = match std::fs::read_dir(&dir) {
        Ok(d) => d,
        Err(e) => {
            tracing::info!(error = %e, path = %dir.display(),
                "[seed_dataviews] directory missing, skipping");
            return;
        }
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    files.sort();

    let total = files.len();
    let mut applied = 0usize;
    let mut failed = 0usize;
    for path in files {
        match upsert_one(state, &path) {
            Ok(id) => {
                tracing::info!(id, path = %path.display(), "[seed_dataviews] upserted");
                applied += 1;
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "[seed_dataviews] failed");
                failed += 1;
            }
        }
    }
    tracing::info!(total, applied, failed, "[seed_dataviews] done");
}

fn upsert_one(state: &AppState, path: &PathBuf) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let dv: DataViewFile = toml::from_str(&text).map_err(|e| format!("parse: {e}"))?;

    // Verify the source binding exists. We don't auto-create here — the
    // sources seed runs first; if the referenced source isn't in the
    // sources table, that's a real misconfiguration the operator should
    // fix in TOML.
    let bound_source_exists = state.db.query_one(
        "SELECT id FROM sources WHERE id = ?1",
        &[&dv.source.source_id as &dyn rusqlite::types::ToSql],
    ).is_ok();
    if !bound_source_exists {
        return Err(format!(
            "source binding {:?} not found in sources table — check seed order or add the source TOML first",
            dv.source.source_id
        ));
    }

    // Build the SQLite-stored JSON values.
    let source_json = json!({
        "type": "source",
        "config": { "source_id": dv.source.source_id, "output": null }
    }).to_string();

    let columns_json = Value::Array(
        dv.columns.into_iter().map(toml_to_json).collect()
    ).to_string();

    let dims_json = dv.dimensions
        .map(toml_to_json)
        .unwrap_or_else(|| json!([]))
        .to_string();
    let sort_json = dv.sort
        .map(toml_to_json)
        .unwrap_or_else(|| json!([]))
        .to_string();
    let cascading_json = dv.cascading_filters
        .map(toml_to_json)
        .unwrap_or_else(|| json!([]))
        .to_string();
    let contract_json = dv.contract
        .map(toml_to_json)
        .unwrap_or_else(|| json!({}))
        .to_string();
    let workflow_json = dv.backend_workflow
        .map(toml_to_json)
        .unwrap_or_else(|| json!([]))
        .to_string();

    state.db.execute(
        "INSERT INTO dataviews \
            (id, display_name, contract, dimensions, columns, sort, \
             backend_workflow, cascading_filters, source) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(id) DO UPDATE SET \
            display_name      = excluded.display_name, \
            contract          = excluded.contract, \
            dimensions        = excluded.dimensions, \
            columns           = excluded.columns, \
            sort              = excluded.sort, \
            backend_workflow  = excluded.backend_workflow, \
            cascading_filters = excluded.cascading_filters, \
            source            = excluded.source, \
            updated_at        = CURRENT_TIMESTAMP",
        &[
            &dv.id as &dyn rusqlite::types::ToSql,
            &dv.display_name as _,
            &contract_json as _,
            &dims_json as _,
            &columns_json as _,
            &sort_json as _,
            &workflow_json as _,
            &cascading_json as _,
            &source_json as _,
        ],
    ).map_err(|e| format!("upsert: {e}"))?;

    Ok(dv.id)
}

/// Same toml→json bridge as `seed::sources::toml_to_json`. Kept local to
/// avoid leaking a util across modules with no clear home.
fn toml_to_json(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s)   => Value::String(s),
        toml::Value::Integer(i)  => Value::Number(serde_json::Number::from(i)),
        toml::Value::Float(f)    => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        toml::Value::Boolean(b)  => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(arr)  => Value::Array(arr.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(t)    => {
            let mut m = serde_json::Map::new();
            for (k, v) in t { m.insert(k, toml_to_json(v)); }
            Value::Object(m)
        }
    }
}
