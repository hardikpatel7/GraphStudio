//! Upsert `<tenant_data_dir>/sources/<id>.toml` files into the
//! `sources` SQLite table at boot. Pattern mirrors `seed::duckdb_views`:
//! missing directory is a skip, per-file failures are logged warnings,
//! idempotent.
//!
//! TOML shape (all kinds):
//!   id             = "src_xxx"
//!   display_name   = "..."
//!   kind           = "pg_query" | "duckdb_table" | "duckdb_query" | "parquet_glob" | "bq_query" | "cdc_pg"
//!   connection_ref = "uat"          (optional; pg_query / bq_query / cdc_pg)
//!   target_table   = "v_xxx"        (optional; duckdb_table / cdc_pg)
//!   primary_key    = ["col1", ...]  (optional)
//!   cdc_enabled    = false          (optional)
//!
//!   [config]                        (kind-specific; opaque to the loader)
//!   sql = """SELECT ..."""          (pg_query / duckdb_query / bq_query)
//!   path = "..."                    (parquet_glob)
//!
//! Status is set to `not_yet_populated` on insert; on conflict, status is
//! preserved (a tenant may have populated the source between boots).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

const ALLOWED_KINDS: &[&str] = &[
    "pg_query",
    "bq_query",
    "duckdb_query",
    "parquet_glob",
    "duckdb_table",
    "cdc_pg",
];

#[derive(Debug, Deserialize)]
struct SourceFile {
    id: String,
    display_name: String,
    kind: String,
    #[serde(default)]
    connection_ref: Option<String>,
    #[serde(default)]
    target_table: Option<String>,
    #[serde(default)]
    primary_key: Vec<String>,
    #[serde(default)]
    cdc_enabled: bool,
    #[serde(default)]
    config: Option<toml::Value>,
}

pub fn seed_sources(state: &Arc<AppState>) {
    let dir = Path::new(&state.data_dir).join("sources");
    let entries = match std::fs::read_dir(&dir) {
        Ok(d) => d,
        Err(e) => {
            tracing::info!(error = %e, path = %dir.display(),
                "[seed_sources] directory missing, skipping");
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
                tracing::info!(id, path = %path.display(), "[seed_sources] upserted");
                applied += 1;
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "[seed_sources] failed");
                failed += 1;
            }
        }
    }
    tracing::info!(total, applied, failed, "[seed_sources] done");
}

fn upsert_one(state: &AppState, path: &PathBuf) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let s: SourceFile = toml::from_str(&text).map_err(|e| format!("parse: {e}"))?;

    if !ALLOWED_KINDS.contains(&s.kind.as_str()) {
        return Err(format!(
            "invalid kind {:?} (allowed: {:?})", s.kind, ALLOWED_KINDS
        ));
    }

    let config_json: Value = match s.config {
        Some(v) => toml_to_json(v),
        None => json!({}),
    };
    let config_str = config_json.to_string();
    let pk_str = serde_json::to_string(&s.primary_key).unwrap_or_else(|_| "[]".to_string());

    // Upsert. Preserve `status` and `last_populated_at` on conflict — the
    // tenant may have populated the source between boots and we don't want
    // to reset that bookkeeping on every reseed.
    state.db.execute(
        "INSERT INTO sources \
            (id, display_name, kind, connection_ref, config, target_table, \
             primary_key, cdc_enabled, status) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'not_yet_populated') \
         ON CONFLICT(id) DO UPDATE SET \
            display_name = excluded.display_name, \
            connection_ref = excluded.connection_ref, \
            config = excluded.config, \
            target_table = excluded.target_table, \
            primary_key = excluded.primary_key, \
            cdc_enabled = excluded.cdc_enabled, \
            updated_at = CURRENT_TIMESTAMP",
        &[
            &s.id as &dyn rusqlite::types::ToSql,
            &s.display_name as _,
            &s.kind as _,
            &s.connection_ref as _,
            &config_str as _,
            &s.target_table as _,
            &pk_str as _,
            &(s.cdc_enabled as i64) as _,
        ],
    ).map_err(|e| format!("upsert: {e}"))?;

    Ok(s.id)
}

/// Convert a TOML value to a serde_json::Value preserving structure.
/// Used to stringify the `[config]` section without forcing the loader
/// to know its schema (it varies per source kind).
fn toml_to_json(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s)  => Value::String(s),
        toml::Value::Integer(i) => Value::Number(serde_json::Number::from(i)),
        toml::Value::Float(f)   => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(arr) => Value::Array(arr.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(t)   => {
            let mut m = serde_json::Map::new();
            for (k, v) in t { m.insert(k, toml_to_json(v)); }
            Value::Object(m)
        }
    }
}
