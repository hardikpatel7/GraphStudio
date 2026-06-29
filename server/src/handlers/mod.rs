pub mod modules;
pub mod graph_articles;
pub mod article_selection;
pub mod cross_filter;
pub mod dataview_source;
pub mod pipeline_v2;
pub mod sources;
pub mod submodules;
pub mod components;
pub mod dataviews;
pub mod dimensions;
pub mod datasources;
pub mod filter_configs;
pub mod templates;
pub mod language_packs;
pub mod pipeline_handler;
pub mod derived_tables;
pub mod parquet_browse;
pub mod activity;
pub mod saved_queries;
pub mod ingest;
pub mod snapshots;
pub mod config_toml;
pub mod viewports;
pub mod generate;
pub mod shared_pipelines;
pub mod duckdb_query;
pub mod bundle;
pub mod graphs;
pub mod feedback;

use axum::{Json, extract::State};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;

/// `GET /api/health` — liveness + a snapshot of the active config.
///
/// Paths are read from `AppState` (which got them from `instance_config`
/// at boot), not from process env vars. The only env var still surfaced
/// here is `DIST_DIR` because that one *is* a runtime override on top of
/// the discovery rules — keeping it in the response lets operators
/// confirm which override (if any) is in effect.
pub async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "config": {
            "tenant_id": state.tenant_id,
            "client": state.client,
            "app_type": state.app_type,
            "environment": state.environment,
            "db_path": state.db_path,
            "duckdb_path": state.duckdb_path,
            "parquet_home": state.parquet_home,
            "port": state.port,
        },
        "runtime": {
            "DIST_DIR": std::env::var("DIST_DIR").unwrap_or_default(),
            "cwd": std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
            "exe": std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        }
    }))
}

/// Tenant identity for the running instance. Read from environment.toml at startup.
/// This is the single source of truth — no client_id/app_id keying needed since each
/// instance hosts exactly one tenant.
///
/// `display_name` carries only `<client> <app_type>` ("who"). The environment
/// is exposed separately via the `environment` field ("where") so the UI can
/// render it as its own badge without duplicating the value.
pub async fn identity(axum::extract::State(state): axum::extract::State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "id":           state.tenant_id,
        "client":       state.client,
        "app_type":     state.app_type,
        "environment":  state.environment,
        "display_name": format!("{} {}", state.client, state.app_type),
    }))
}

/// Standard JSON error response.
pub fn err(status: u16, msg: &str) -> (axum::http::StatusCode, Json<Value>) {
    (axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
     Json(json!({"error": msg})))
}

/// Helper: stringify a serde_json::Value for SQLite TEXT storage.
pub fn stringify(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

/// Log an activity event (best-effort, never fails the request).
/// `tenant_id` is typically the app_id or client_id.
pub fn log_activity(state: &Arc<AppState>, tenant_id: &str, category: &str, action: &str, status: &str, message: &str, detail: Option<&str>, duration_ms: Option<i64>) {
    if let Err(e) = state.traces.log_activity(tenant_id, category, action, status, message, detail, duration_ms) {
        tracing::warn!(error = %e, "Failed to log activity");
    }
}

/// Wire attribute name → graph kind name. Shared across
/// `handlers::cross_filter` and `handlers::dataview_source` since both
/// translate the same wire vocabulary before feeding
/// `graph::cross_filter`.
pub fn normalize_bealls_attribute(s: &str) -> String {
    match s {
        "l0_name" => "l0".to_string(),
        "l1_name" => "l1".to_string(),
        "l2_name" => "l2".to_string(),
        "l3_name" => "l3".to_string(),
        "l4_name" => "l4".to_string(),
        "l5_name" => "l5".to_string(),
        other => other.to_string(),
    }
}

/// Fetch the default graph snapshot named by `[graphs] default_id` in
/// environment.toml, if one is built. Returns `None` when:
///   - `default_graph_id` isn't configured
///   - no `POST /api/graphs/:id/build` has populated the slot
///   - the slot was cleared (post-build reset)
///
/// The returned `Arc` is a cheap clone — readers traverse without
/// locking.
pub async fn get_default_graph(state: &Arc<AppState>) -> Option<Arc<crate::graph::Graph>> {
    let id = state.default_graph_id.as_deref()?;
    let slot = {
        let graphs = state.graphs.read().await;
        graphs.get(id).cloned()
    };
    let slot = slot?;
    let guard = slot.load();
    guard.as_ref().map(|arc| arc.clone())
}
