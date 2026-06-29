//! Graph services. List/describe over the SQLite `graphs` table, plus the
//! three traversal entry points the agent and HTTP routes both consume:
//!
//! - `node`         — project a single `(kind, name)` from the live snapshot
//! - `traverse`     — walk an `Edge` from a starting node, page + project
//! - `cross_filter` — apply a v1 `FilterPayload` against a snapshot and
//!                    return distinct attribute values per the requested
//!                    projection.
//!
//! Snapshot lookup, kind/name resolution, and projection all delegate to
//! `crate::graph::*`. The corresponding handlers in `handlers::graphs`
//! become thin: decode args, call here, surface `ServiceError`.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cross_filter::model::FilterPayload;
use crate::graph::{
    self,
    cross_filter::{apply_filters, filters_from_payload, project_distinct, EntitledSet},
    project::{project, ProjectionOptions},
    traverse::{traverse, Edge},
    uam_adapter::{entitled_set_for, Lookup as UamLookup},
};
use crate::AppState;

use super::error::ServiceError;
use super::ServiceResult;

// ── List / describe (already lived here before the trio landed) ──────────

pub async fn list(state: &AppState) -> Result<Vec<Value>> {
    state.db.query(
        "SELECT id, display_name, last_validated_at, error_log, created_at, updated_at \
         FROM graphs ORDER BY display_name",
        &[],
    )
}

pub async fn describe(state: &AppState, id: &str) -> Result<Value> {
    state.db.query_one(
        "SELECT * FROM graphs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
}

/// Build (or rebuild) a graph from its stored TOML spec and atomically
/// publish into `state.graphs[id]`. Used both by the HTTP build handler
/// and the boot-time eager-build task — without the latter, every
/// server restart leaves graphs unbuilt and any dataview / agent call
/// that depends on them 404s until someone POSTs `/build`, which has
/// burned several dashboard sessions.
pub async fn build_by_id(state: &Arc<AppState>, id: &str) -> Result<graph::BuildStats> {
    let row = state.db.query_one(
        "SELECT toml_text FROM graphs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )?;
    let toml_text = row["toml_text"].as_str().unwrap_or("").to_string();

    let spec = graph::from_toml(&toml_text)?;
    let issues = graph::validate(&spec);
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| matches!(i.severity, graph::Severity::Error))
        .collect();
    if !errors.is_empty() {
        anyhow::bail!(
            "graph `{id}` failed validation with {} error(s); not building",
            errors.len()
        );
    }

    let duckdb_path = state.duckdb_path.clone();
    let spec_arc = Arc::new(spec);
    let spec_for_build = spec_arc.clone();
    let (built, stats) = tokio::task::spawn_blocking(move || -> Result<_> {
        let reader = graph::source::duckdb::DuckDbSourceReader::open(&duckdb_path)?;
        graph::build_graph(&spec_for_build, &reader, 1)
    })
    .await??;

    let slot: Arc<arc_swap::ArcSwapOption<graph::Graph>> = {
        let mut graphs = state.graphs.write().await;
        graphs
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(arc_swap::ArcSwapOption::from(None)))
            .clone()
    };
    slot.store(Some(Arc::new(built)));
    Ok(stats)
}

// ── Wire request shapes ──────────────────────────────────────────────────
//
// These were previously private to `handlers::graphs`. Hoisted here so the
// agent's tool layer can construct them too without re-deriving Deserialize
// from scratch.

#[derive(Debug, Deserialize)]
pub struct FromRef {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeRequest {
    Children,
    Parent,
    Ancestors,
    DescendantsOfKind(String),
    CrossEdge(String),
}

impl EdgeRequest {
    fn as_edge(&self) -> Edge<'_> {
        match self {
            EdgeRequest::Children => Edge::Children,
            EdgeRequest::Parent => Edge::Parent,
            EdgeRequest::Ancestors => Edge::Ancestors,
            EdgeRequest::DescendantsOfKind(s) => Edge::DescendantsOfKind(s.as_str()),
            EdgeRequest::CrossEdge(s) => Edge::CrossEdge(s.as_str()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TraverseRequest {
    pub from: FromRef,
    pub edge: EdgeRequest,
    #[serde(default)]
    pub project: ProjectionOptions,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct NodeRequest {
    pub from: FromRef,
    #[serde(default)]
    pub project: ProjectionOptions,
}

#[derive(Debug, Deserialize, Default)]
pub struct CrossFilterQuery {
    #[serde(default)]
    pub target_kind: Option<String>,
}

// ── Internal: pull a snapshot or surface a `NotFound` ────────────────────

type GraphSlot = Arc<arc_swap::ArcSwapOption<graph::Graph>>;

async fn snapshot_for(state: &AppState, id: &str) -> ServiceResult<Arc<graph::Graph>> {
    let slot: Option<GraphSlot> = {
        let graphs = state.graphs.read().await;
        graphs.get(id).cloned()
    };
    let slot = slot.ok_or_else(|| {
        ServiceError::not_found("graph not built — call POST /api/graphs/:id/build")
    })?;
    let snap = slot.load_full().ok_or_else(|| ServiceError::not_found("graph slot is empty"))?;
    Ok(snap)
}

// ── node / traverse / cross_filter ───────────────────────────────────────

pub async fn node(state: &AppState, id: &str, req: NodeRequest) -> ServiceResult<Value> {
    let graph = snapshot_for(state, id).await?;
    let kind_id = graph
        .kinds
        .id_of(&req.from.kind)
        .ok_or_else(|| ServiceError::bad_request(format!("unknown kind `{}`", req.from.kind)))?;
    let node_id = graph.find_by_name(kind_id, &req.from.name).ok_or_else(|| {
        ServiceError::not_found(format!(
            "node ({}, {}) not found in graph",
            req.from.kind, req.from.name
        ))
    })?;
    let row = project(&graph, node_id, &req.project);
    Ok(json!({
        "id": id,
        "from": { "kind": req.from.kind, "name": req.from.name },
        "row": row,
    }))
}

pub async fn traverse_fn(
    state: &AppState,
    id: &str,
    req: TraverseRequest,
) -> ServiceResult<Value> {
    let graph = snapshot_for(state, id).await?;
    let kind_id = graph
        .kinds
        .id_of(&req.from.kind)
        .ok_or_else(|| ServiceError::bad_request(format!("unknown kind `{}`", req.from.kind)))?;
    let from_id = graph.find_by_name(kind_id, &req.from.name).ok_or_else(|| {
        ServiceError::not_found(format!(
            "node ({}, {}) not found in graph",
            req.from.kind, req.from.name
        ))
    })?;

    let nodes = traverse(&graph, from_id, req.edge.as_edge());
    let total = nodes.len();
    let offset = req.offset.unwrap_or(0).min(total);
    let limit = req.limit.unwrap_or(1000);
    let end = offset.saturating_add(limit).min(total);
    let rows: Vec<Value> = nodes[offset..end]
        .iter()
        .map(|n| project(&graph, *n, &req.project))
        .collect();

    Ok(json!({
        "id": id,
        "from": { "kind": req.from.kind, "name": req.from.name },
        "rows": rows,
        "total": total,
        "offset": offset,
        "limit": limit,
    }))
}

pub async fn cross_filter(
    state: &AppState,
    id: &str,
    q: CrossFilterQuery,
    payload: FilterPayload,
) -> ServiceResult<Value> {
    let graph = snapshot_for(state, id).await?;

    let target_name = q.target_kind.as_deref().unwrap_or("article");
    let target_kind = graph.kinds.id_of(target_name).ok_or_else(|| {
        ServiceError::bad_request(format!("unknown target_kind `{target_name}`"))
    })?;

    let (filters, attributes) = filters_from_payload(&payload);

    let entitled: Option<EntitledSet> = if payload.is_urm_filter {
        let user_code = payload
            .user_code
            .ok_or_else(|| ServiceError::bad_request("is_urm_filter requires user_code"))?;
        let acl_code = payload
            .acl_code
            .ok_or_else(|| ServiceError::bad_request("is_urm_filter requires acl_code"))?;
        match entitled_set_for(&state.uam, user_code, acl_code, &graph, target_kind) {
            UamLookup::Unknown => {
                return Err(ServiceError::not_found(format!(
                    "no entitlements for user_code={user_code}, acl_code={acl_code}"
                )));
            }
            UamLookup::Unrestricted => None,
            UamLookup::Restricted(set) => Some(set),
        }
    } else {
        None
    };

    let candidates = apply_filters(&graph, target_kind, &filters, entitled.as_ref());
    let attr_refs: Vec<&str> = attributes.iter().map(String::as_str).collect();
    let data: HashMap<String, Vec<String>> =
        project_distinct(&graph, target_kind, &candidates, &attr_refs);

    Ok(json!({
        "id": id,
        "target_kind": target_name,
        "count": candidates.len(),
        "status": true,
        "data": data,
        "message": "ok",
    }))
}
