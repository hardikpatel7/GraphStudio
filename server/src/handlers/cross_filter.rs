//! HTTP handler for `POST /api/cross-filter`.
//!
//! Wire shape mirrors `inventory-smart-rust`'s
//! `handle_cross_filter_v2_with_uam` (file
//! `impact_core/src/core/filters/router.rs`): accepts a `FilterPayload`,
//! returns a `FilterResponse`. The graph snapshot resolves through
//! `state.default_graph_id` → `state.graphs[id]`; UAM entitlements
//! (when `is_urm_filter` is true) come from `state.uam`.

use std::sync::Arc;

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::AppState;
use crate::cross_filter::{FilterPayload, FilterResponse};

use super::err;

pub async fn handle_cross_filter(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<FilterPayload>,
) -> Result<Json<FilterResponse>, (axum::http::StatusCode, Json<Value>)> {
    let graph = super::get_default_graph(&state).await.ok_or_else(|| {
        err(
            503,
            "default graph not built — POST /api/graphs/:id/build first (id from [graphs] default_id)",
        )
    })?;
    cross_filter_path(&state, &payload, &graph).await
}

/// `POST /api/uam/refresh` — kicks a manual cold-load of the UAM cache.
/// Phase A only: until CDC lands (Phase B), callers (or a cron task)
/// invoke this when underlying UAM rows change.
pub async fn refresh_uam(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let graph_arc = state.legacy_graph.load_full().ok_or_else(|| {
        err(503, "article_graph not built yet — run pipeline pl_build_article_graph first")
    })?;
    let dsn = resolve_default_pg_dsn(&state).ok_or_else(|| {
        err(400, "no PG data_source available (mark one as default for type=pg)")
    })?;
    let started = std::time::Instant::now();
    let universe = graph_arc.count_kind(crate::graph::legacy::NodeKind::Article) as i64;
    state
        .uam
        .cold_load(&dsn, graph_arc)
        .await
        .map_err(|e| err(500, &format!("UAM cold-load: {e:#}")))?;
    if let Err(e) = state.uam.materialize_to_duckdb(&state.duckdb_path, universe) {
        tracing::warn!(error=%e, "[uam] materialize_to_duckdb after refresh failed");
    }
    Ok(Json(json!({
        "status": "ok",
        "entries": state.uam.entry_count(),
        "restrictive_users": state.uam.restrictive_user_count(),
        "duration_ms": started.elapsed().as_millis() as i64,
    })))
}

/// Same default-PG resolution used by the article_selection handler
/// and the pipeline_assemblies registry. Kept locally to avoid
/// pulling those modules in here.
fn resolve_default_pg_dsn(state: &AppState) -> Option<String> {
    let sources = state.db.query("SELECT * FROM connections", &[]).ok()?;
    let is_pg = |c: &&Value| {
        let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        t == "pg" || t == "postgres"
    };
    let is_default = |c: &&Value| c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
    let conn = sources
        .iter()
        .find(|c| is_pg(c) && is_default(c))
        .or_else(|| sources.iter().find(is_pg))?;
    crate::query::pg_conn_str(conn.get("config")?)
}

/// Cross-filter path for `/api/cross-filter`. Routes through the
/// metadata-driven resolver in `graph::cross_filter`, including
/// the UAM adapter that re-resolves raw filters against the graph
/// snapshot. Target kind hardcoded to `"article"` — the canonical
/// cross-filter dimension.
async fn cross_filter_path(
    state: &Arc<AppState>,
    payload: &FilterPayload,
    graph: &Arc<crate::graph::Graph>,
) -> Result<Json<FilterResponse>, (axum::http::StatusCode, Json<Value>)> {
    use crate::graph::cross_filter::{
        apply_filters as v2_apply_filters, filters_from_payload,
        project_distinct as v2_project_distinct,
    };
    use crate::graph::uam_adapter::{Lookup as UamLookup, entitled_set_for};

    let target_kind = graph.kinds.id_of("article").ok_or_else(|| {
        err(
            503,
            "graph has no `article` kind — TOML must declare `[hierarchy.product.article]`",
        )
    })?;

    let entitled = if payload.is_urm_filter {
        let user = payload
            .user_code
            .ok_or_else(|| err(400, "user_code is required when is_urm_filter=true"))?;
        let acl = payload
            .acl_code
            .ok_or_else(|| err(400, "acl_code is required when is_urm_filter=true"))?;
        match entitled_set_for(&state.uam, user, acl, graph, target_kind) {
            UamLookup::Unknown => {
                return Err(err(
                    403,
                    &format!("no UAM entry for user_code={user} acl_code={acl}"),
                ));
            }
            UamLookup::Unrestricted => None,
            UamLookup::Restricted(set) => Some(set),
        }
    } else {
        None
    };

    // Attribute-name alias: external wire uses `l0_name`..`l5_name`
    // whereas the graph TOML names the kinds `l0`..`l5`. Translate at
    // the boundary so callers don't need to know the internal naming.
    let (mut filters, attr_owned) = filters_from_payload(payload);
    for f in filters.iter_mut() {
        f.attribute_name = super::normalize_bealls_attribute(&f.attribute_name);
    }
    let attr_owned: Vec<String> = attr_owned
        .into_iter()
        .map(|a| super::normalize_bealls_attribute(&a))
        .collect();

    let graph_for_blocking = graph.clone();
    let result = tokio::task::spawn_blocking(move || -> FilterResponse {
        let candidates = v2_apply_filters(&graph_for_blocking, target_kind, &filters, entitled.as_ref());
        let attr_refs: Vec<&str> = attr_owned.iter().map(String::as_str).collect();
        let raw_data =
            v2_project_distinct(&graph_for_blocking, target_kind, &candidates, &attr_refs);
        let data: std::collections::HashMap<String, Vec<String>> = raw_data
            .into_iter()
            .map(|(k, v)| (denormalize_bealls_attribute(&k), v))
            .collect();
        FilterResponse {
            total: None,
            page: None,
            count: data.len() as i32,
            status: true,
            data,
            message: "Successful".to_string(),
        }
    })
    .await
    .map_err(|e| err(500, &format!("cross_filter task join: {e}")))?;

    Ok(Json(result))
}

/// Inverse of `handlers::normalize_bealls_attribute`. Run on the
/// returned `data` keys so clients see the wire vocabulary they sent
/// in. Unknown attribute names pass through unchanged.
fn denormalize_bealls_attribute(s: &str) -> String {
    match s {
        "l0" => "l0_name".to_string(),
        "l1" => "l1_name".to_string(),
        "l2" => "l2_name".to_string(),
        "l3" => "l3_name".to_string(),
        "l4" => "l4_name".to_string(),
        "l5" => "l5_name".to_string(),
        other => other.to_string(),
    }
}
