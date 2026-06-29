//! DataView services. CRUD-write paths stay in `handlers::dataviews`; the
//! read-shaped operations the agent needs live here.
//!
//! `list` + `describe` are extracted normally — small SQL queries that take
//! `&AppState` and return JSON.
//!
//! `read` + `introspect` use a **bridge** pattern: they wrap the existing
//! handler functions in `crate::handlers::dataview_source::{data,
//! introspect_source}` (each ~150–500 lines of multi-engine dispatch over
//! PG/DuckDB/ClickHouse/BQ/parquet/graph). Re-extracting those bodies cleanly
//! would mean threading `ServiceError` through ~10 helper functions; the
//! bridge gives the agent the same capability at near-zero risk.

use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};

use crate::AppState;

use super::error::ServiceError;
use super::ServiceResult;

pub async fn list(state: &AppState) -> Result<Vec<Value>> {
    state.db.query("SELECT * FROM dataviews ORDER BY display_name", &[])
}

pub async fn describe(state: &AppState, id: &str) -> Result<Value> {
    state.db.query_one(
        "SELECT * FROM dataviews WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
}

/// `dataview_read` — bridge into the existing `handlers::dataview_source::data`
/// multi-engine read path. `body` is the typed `DataReq` shape it accepts
/// (limit, offset, sort_col, sort_dir, filters, rules, node_kind, group_by,
/// aggregates, having, skip_total). Missing fields fall back to handler
/// defaults via `serde(default)`.
pub async fn read(state: Arc<AppState>, dv_id: String, body: Value) -> ServiceResult<Value> {
    let res = crate::handlers::dataview_source::data(
        State(state),
        Path(dv_id),
        Json(body),
    ).await;
    handler_to_service(res)
}

/// `introspect_dataview` — bridge into `handlers::dataview_source::introspect_source`.
/// Returns `{ source, columns: [{name, type}], engine }`.
pub async fn introspect(state: Arc<AppState>, dv_id: String) -> ServiceResult<Value> {
    let res = crate::handlers::dataview_source::introspect_source(
        State(state),
        Path(dv_id),
    ).await;
    handler_to_service(res)
}

/// Translate the handler's `Result<Json, (StatusCode, Json)>` into our
/// `ServiceResult`. Maps 404/400 to their dedicated variants and folds
/// everything else into `Internal` so the agent's Tool layer sees the
/// same shape it does for the natively-extracted services.
fn handler_to_service(
    res: Result<Json<Value>, (StatusCode, Json<Value>)>,
) -> ServiceResult<Value> {
    match res {
        Ok(Json(v)) => Ok(v),
        Err((status, Json(body))) => {
            let msg = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .to_string();
            Err(match status {
                StatusCode::NOT_FOUND   => ServiceError::not_found(msg),
                StatusCode::BAD_REQUEST => ServiceError::bad_request(msg),
                _                       => ServiceError::internal(anyhow::anyhow!(msg)),
            })
        }
    }
}

// Touch json! once so rustc doesn't drop the import when serde_json's macro
// isn't used directly above. Stays inert; lets future contributors add
// `json!({...})` literals here without re-adding the macro import.
#[allow(dead_code)]
fn _ensure_json_macro_in_scope() -> Value { json!(null) }
