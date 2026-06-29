//! Bundle export/import — one JSON for many objects across kinds.
//!
//! Lets the user pick a set of objects (dataviews, pipelines, connections,
//! sources, dimensions, filter_configs, saved_queries) and round-trip them
//! as a single document. Use cases: ship a client's whole config to another
//! tenant, snapshot the current state for review, restore after a tenant
//! reset.
//!
//! Format:
//! ```json
//! {
//!   "version": 1,
//!   "exported_at": "2026-05-06T...",
//!   "tenant_id": "...",
//!   "objects": {
//!     "dataviews":      [...],
//!     "pipelines":      [...],
//!     "connections":    [...],
//!     "sources":        [...],
//!     "dimensions":     [...],
//!     "filter_configs": [...],
//!     "saved_queries":  [...]
//!   }
//! }
//! ```
//!
//! Each kind's rows are the table rows themselves with `created_at` /
//! `updated_at` stripped so the document round-trips cleanly.

use axum::{Json, extract::State};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::AppState;
use super::err;

const KIND_DATAVIEWS: &str = "dataviews";
const KIND_PIPELINES: &str = "pipelines";
const KIND_SOURCES: &str = "sources";
const KIND_DIMENSIONS: &str = "dimensions";
const KIND_FILTER_CONFIGS: &str = "filter_configs";
const KIND_SAVED_QUERIES: &str = "saved_queries";

// `connections` deliberately omitted: it's the only table that carries
// PG passwords + internal hostnames in plain text, and exporting that
// blob trips WAF / reverse-proxy rules at the edge (403 before the
// request reaches us). Connections must be re-created per tenant.
const ALL_KINDS: &[&str] = &[
    KIND_DATAVIEWS,
    KIND_PIPELINES,
    KIND_SOURCES,
    KIND_DIMENSIONS,
    KIND_FILTER_CONFIGS,
    KIND_SAVED_QUERIES,
];

/// `GET /api/bundle/inventory` — list every object across every supported
/// kind so the UI can render a "pick what to export" tree without doing
/// 7 separate fetches. Returns the minimal projection needed to render
/// (id + display_name).
pub async fn inventory(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let mut out = serde_json::Map::new();
    for kind in ALL_KINDS {
        let rows = list_min(&state, kind)
            .map_err(|e| err(500, &format!("inventory({}): {}", kind, e)))?;
        out.insert((*kind).to_string(), Value::Array(rows));
    }
    Ok(Json(Value::Object(out)))
}

/// `POST /api/bundle/export` — produce a downloadable JSON containing the
/// requested objects.
///
/// Body:
/// ```json
/// { "kinds": { "dataviews": ["dv_a", "dv_b"], "pipelines": ["pl_x"], ... } }
/// ```
/// Empty arrays / missing keys are skipped. Unknown kinds error 400.
pub async fn export(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<Value>)> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let kinds = body.get("kinds").and_then(|v| v.as_object()).cloned()
        .ok_or_else(|| err(400, "body must include `kinds`: { kind: [ids] }"))?;

    let mut objects = serde_json::Map::new();
    let mut total: usize = 0;
    for (kind, ids_val) in kinds.iter() {
        if !ALL_KINDS.contains(&kind.as_str()) {
            return Err(err(400, &format!("unknown kind '{}'", kind)));
        }
        let ids: Vec<String> = ids_val.as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if ids.is_empty() { continue; }
        let rows = fetch_by_ids(&state, kind, &ids)
            .map_err(|e| err(500, &format!("export({}): {}", kind, e)))?;
        total += rows.len();
        objects.insert(kind.clone(), Value::Array(rows));
    }
    if total == 0 {
        return Err(err(400, "no objects selected"));
    }

    let payload = json!({
        "version": 1,
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "tenant_id": state.tenant_id,
        "objects": objects,
    });
    let body = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    let filename = format!("smartstudio-bundle-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S"));
    let resp = (
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", filename)),
        ],
        body,
    ).into_response();
    Ok(resp)
}

/// `POST /api/bundle/import` — apply the inverse of `export`.
///
/// Body: `{ data: <bundle>, mode: "new" | "replace" }`
/// - `mode = "replace"`: upsert by id (matching the per-kind importers'
///   replace semantics).
/// - `mode = "new"`: rename clashing ids to `<id>_imported_<unix>` so a
///   second import doesn't clobber the existing rows.
pub async fn import(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let data = body.get("data").cloned()
        .ok_or_else(|| err(400, "body must include `data`"))?;
    let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("new").to_string();
    if mode != "new" && mode != "replace" {
        return Err(err(400, &format!("invalid mode '{}'", mode)));
    }

    let objects = data.get("objects").and_then(|v| v.as_object()).cloned()
        .ok_or_else(|| err(400, "bundle missing `objects`"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Per-kind summary so the UI can show "imported: 4 dataviews + 2 pipelines".
    let mut summary = serde_json::Map::new();
    let mut total_inserted: usize = 0;
    let mut total_replaced: usize = 0;

    for (kind, rows_val) in objects.iter() {
        if !ALL_KINDS.contains(&kind.as_str()) {
            return Err(err(400, &format!("unknown kind in bundle: '{}'", kind)));
        }
        let rows = match rows_val.as_array() {
            Some(a) => a,
            None => continue,
        };

        let mut inserted = 0;
        let mut replaced = 0;
        for row in rows {
            let mut row = row.clone();
            // Strip transient fields if the source carried them.
            if let Some(obj) = row.as_object_mut() {
                obj.remove("created_at");
                obj.remove("updated_at");
            }
            let original_id = row.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if original_id.is_empty() {
                continue;
            }
            let exists_at_original = id_exists(&state, kind, &original_id)?;
            let target_id = if mode == "new" && exists_at_original {
                let new_id = format!("{}_imported_{}", original_id, now);
                if let Some(obj) = row.as_object_mut() {
                    obj.insert("id".into(), Value::String(new_id.clone()));
                }
                new_id
            } else {
                original_id.clone()
            };
            upsert(&state, kind, &row)
                .map_err(|e| err(500, &format!("import({}, {}): {}", kind, target_id, e)))?;
            if mode == "replace" && exists_at_original {
                replaced += 1;
            } else {
                inserted += 1;
            }
        }
        total_inserted += inserted;
        total_replaced += replaced;
        summary.insert(kind.clone(), json!({
            "inserted": inserted,
            "replaced": replaced,
            "total":    rows.len(),
        }));
    }

    Ok(Json(json!({
        "ok": true,
        "mode": mode,
        "inserted": total_inserted,
        "replaced": total_replaced,
        "by_kind": Value::Object(summary),
    })))
}

// ─── per-kind helpers ──────────────────────────────────────────────────

fn list_min(state: &AppState, kind: &str) -> anyhow::Result<Vec<Value>> {
    let table = table_for(kind)?;
    let sql = format!("SELECT id, display_name FROM {} ORDER BY display_name", table);
    Ok(state.db.query(&sql, &[])?)
}

fn fetch_by_ids(state: &AppState, kind: &str, ids: &[String]) -> anyhow::Result<Vec<Value>> {
    if ids.is_empty() { return Ok(Vec::new()); }
    let table = table_for(kind)?;
    let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
    let sql = format!("SELECT * FROM {} WHERE id IN ({})", table, placeholders.join(", "));
    let params: Vec<&dyn rusqlite::types::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let mut rows = state.db.query(&sql, &params)?;
    for r in rows.iter_mut() {
        if let Some(obj) = r.as_object_mut() {
            obj.remove("created_at");
            obj.remove("updated_at");
        }
    }
    Ok(rows)
}

fn id_exists(state: &AppState, kind: &str, id: &str) -> Result<bool, (axum::http::StatusCode, Json<Value>)> {
    let table = table_for(kind).map_err(|e| err(400, &e.to_string()))?;
    let sql = format!("SELECT id FROM {} WHERE id = ?1", table);
    Ok(state.db.query_one(&sql, &[&id as &dyn rusqlite::types::ToSql]).is_ok())
}

/// Upsert a row using the kind's column set. Each kind has a fixed shape
/// so we list the columns explicitly — this avoids ambiguity vs. trying
/// to introspect the row's keys (which would let a malformed bundle
/// inject extra columns).
fn upsert(state: &AppState, kind: &str, row: &Value) -> anyhow::Result<usize> {
    use crate::handlers::stringify;
    let table = table_for(kind)?;
    let cols = columns_for(kind)?;
    let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{}", i)).collect();

    // Build the SET clause for ON CONFLICT(id) DO UPDATE — every column
    // except `id` and `created_at`.
    let set_clauses: Vec<String> = cols.iter()
        .filter(|c| **c != "id" && **c != "created_at")
        .map(|c| format!("{} = excluded.{}", c, c))
        .collect();
    let updated_at_clause = if cols.contains(&"updated_at") {
        ", updated_at = datetime('now')".to_string()
    } else { String::new() };

    let sql = format!(
        "INSERT INTO {table} ({col_list}) VALUES ({phs}) \
         ON CONFLICT(id) DO UPDATE SET {set_list}{updated_at}",
        table = table,
        col_list = cols.join(", "),
        phs = placeholders.join(", "),
        set_list = set_clauses.join(", "),
        updated_at = updated_at_clause,
    );

    // Marshal each column from the row. JSON sub-objects are stringified
    // into TEXT (matches how the per-kind handlers persist these fields).
    let mut owned: Vec<String> = Vec::with_capacity(cols.len());
    for c in &cols {
        let v = row.get(*c).cloned().unwrap_or(Value::Null);
        let s = match (&v, *c) {
            (Value::Null, _) => String::new(),
            (Value::String(s), _) => s.clone(),
            // Most JSON-typed columns (config, levels, columns, …) get
            // stringified; numerics/bools land via to_string().
            _ => stringify(&v),
        };
        owned.push(s);
    }
    let params: Vec<&dyn rusqlite::types::ToSql> = owned.iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();
    Ok(state.db.execute(&sql, &params)?)
}

fn table_for(kind: &str) -> anyhow::Result<&'static str> {
    Ok(match kind {
        KIND_DATAVIEWS      => "dataviews",
        KIND_PIPELINES      => "pipelines",
        KIND_SOURCES        => "sources",
        KIND_DIMENSIONS     => "dimensions",
        KIND_FILTER_CONFIGS => "filter_configs",
        KIND_SAVED_QUERIES  => "saved_queries",
        other => return Err(anyhow::anyhow!("unknown kind '{}'", other)),
    })
}

/// Column list per kind. Kept in lock-step with `schema.sql`. The lists
/// exclude transient columns (`created_at`/`updated_at` are managed by the
/// `INSERT` itself; we don't accept them from the bundle).
fn columns_for(kind: &str) -> anyhow::Result<Vec<&'static str>> {
    Ok(match kind {
        KIND_DATAVIEWS => vec![
            "id", "display_name", "contract", "dimensions", "columns",
            "sort", "backend_workflow", "cascading_filters", "source",
        ],
        KIND_PIPELINES => vec![
            "id", "display_name", "pipeline", "trigger", "placement",
            "execution", "description",
        ],
        KIND_SOURCES => vec![
            "id", "display_name", "kind", "connection_ref", "config",
            "target_table", "primary_key", "cdc_enabled", "last_populated_at",
            "status",
        ],
        KIND_DIMENSIONS => vec![
            "id", "display_name", "master_table", "datasource_ref", "levels",
            "additional_filter_cols",
        ],
        KIND_FILTER_CONFIGS => vec![
            "id", "display_name", "dimension_ref", "filter_columns",
            "mandatory_columns", "cascading_rules", "config",
        ],
        KIND_SAVED_QUERIES => vec![
            "id", "display_name", "sql_text", "engine", "description",
        ],
        other => return Err(anyhow::anyhow!("unknown kind '{}'", other)),
    })
}
