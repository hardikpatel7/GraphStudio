//! CRUD + `POST /:id/validate` for graph TOML definitions.
//!
//! Phase 1 of article_graph. The `graphs` table is a thin SQLite
//! wrapper around `toml_text`; `validate` parses + runs metadata-only
//! checks and persists the result back to `last_validated_at` / `error_log`.
//! No engine build is invoked here — that's Phase 2.

use axum::{Json, extract::{Path, Query, State}};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::AppState;
use crate::cross_filter::model::FilterPayload;
use crate::graph::{self, Severity, ValidationIssue};
use crate::graph::cross_filter::{
    EntitledSet, apply_filters, filters_from_payload, project_distinct,
};
use crate::graph::uam_adapter::{Lookup as UamLookup, entitled_set_for};
use crate::graph::project::{ProjectionOptions, project};
use crate::graph::traverse::{Edge, traverse};
use super::{err, log_activity};

/// Type alias keeps the `graphs` field's nested generics readable at
/// every call site below.
type GraphSlot = Arc<arc_swap::ArcSwapOption<graph::Graph>>;

/// `GET /api/graphs` — list all graph rows (no TOML body included to
/// keep payloads small; client fetches body via `get_one`).
pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    crate::service::graphs::list(&state)
        .await
        .map(|rows| Json(Value::Array(rows)))
        .map_err(|e| err(500, &e.to_string()))
}

/// `GET /api/graphs/:id` — single row including `toml_text`.
pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    crate::service::graphs::describe(&state, &id)
        .await
        .map(Json)
        .map_err(|_| err(404, "Graph not found"))
}

/// `POST /api/graphs` — create a new graph row. Body: `{ id, display_name,
/// toml_text? }`. Does NOT auto-validate; client should follow up with
/// `POST /:id/validate` once the TOML is in good shape.
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let display_name = body["display_name"].as_str().unwrap_or("");
    let toml_text = body["toml_text"].as_str().unwrap_or("");

    if id.is_empty() || display_name.is_empty() {
        return Err(err(400, "id and display_name are required"));
    }

    state.db.execute(
        "INSERT INTO graphs (id, display_name, toml_text) VALUES (?1, ?2, ?3)",
        &[
            &id as &dyn rusqlite::types::ToSql,
            &display_name as _,
            &toml_text as _,
        ],
    )
    .map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one(
        "SELECT * FROM graphs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
    .map_err(|e| err(500, &e.to_string()))?;

    let elapsed = t.elapsed().as_millis() as i64;
    log_activity(&state, &state.tenant_id, "graph", "create", "success",
        &format!("Created graph '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

/// `PUT /api/graphs/:id` — partial update. Editing `toml_text` does NOT
/// re-validate automatically (validation is potentially expensive and
/// belongs on a discrete user action); callers should follow with
/// `POST /:id/validate` when they want fresh `error_log`/`last_validated_at`.
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

    if let Some(v) = body.get("display_name").and_then(|v| v.as_str()) {
        sets.push("display_name = ?");
        vals.push(Box::new(v.to_string()));
    }
    if let Some(v) = body.get("toml_text").and_then(|v| v.as_str()) {
        sets.push("toml_text = ?");
        vals.push(Box::new(v.to_string()));
        // Updating the TOML invalidates the prior validation result — clear
        // both columns so the UI doesn't show a stale "validated" badge.
        sets.push("last_validated_at = NULL");
        sets.push("error_log = NULL");
    }

    if sets.is_empty() {
        return Err(err(400, "nothing to update"));
    }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE graphs SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> =
        vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 {
        return Err(err(404, "Graph not found"));
    }

    let row = state.db.query_one(
        "SELECT * FROM graphs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
    .map_err(|e| err(500, &e.to_string()))?;

    let elapsed = t.elapsed().as_millis() as i64;
    log_activity(&state, &state.tenant_id, "graph", "update", "success",
        &format!("Updated graph '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

/// `DELETE /api/graphs/:id`.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute(
        "DELETE FROM graphs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
    .map_err(|e| err(500, &e.to_string()))?;
    if n == 0 {
        return Err(err(404, "Graph not found"));
    }
    // Also drop the in-memory snapshot. ArcSwap doesn't free the
    // inner Arc until existing readers drop their references, so
    // any concurrent /traverse / /cross-filter requests in flight
    // finish against the old snapshot — clean cutover, no torn reads.
    {
        let mut graphs = state.graphs.write().await;
        graphs.remove(&id);
    }
    let elapsed = t.elapsed().as_millis() as i64;
    log_activity(&state, &state.tenant_id, "graph", "delete", "success",
        &format!("Deleted graph '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}

/// `POST /api/graphs/:id/validate` — parse `toml_text`, run `validate()`,
/// persist the issues + timestamp, return the issue list. Errors don't
/// fail the request; the client gets `{ issues: [...], ok: bool }` and
/// renders accordingly.
///
/// A parse failure is a single synthetic "PARSE_ERROR" issue — the rest
/// of the metadata-level checks can't run on an unparseable document.
pub async fn validate_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let row = state.db.query_one(
        "SELECT toml_text FROM graphs WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    )
    .map_err(|_| err(404, "Graph not found"))?;

    let toml_text = row["toml_text"].as_str().unwrap_or("");
    let (issues, parsed_ok): (Vec<ValidationIssue>, bool) = match graph::from_toml(toml_text) {
        Ok(spec) => (graph::validate(&spec), true),
        Err(e) => {
            // Parse failure → single synthetic issue. The parser's own
            // Display already includes line/column context.
            let issue = ValidationIssue {
                severity: Severity::Error,
                code: "PARSE_ERROR",
                message: format!("{e:#}"),
                location: None,
            };
            (vec![issue], false)
        }
    };

    let has_errors = issues.iter().any(|i| matches!(i.severity, Severity::Error));
    let issues_json = serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into());

    state.db.execute(
        "UPDATE graphs SET last_validated_at = datetime('now'), error_log = ?1 WHERE id = ?2",
        &[
            &issues_json as &dyn rusqlite::types::ToSql,
            &id as _,
        ],
    )
    .map_err(|e| err(500, &e.to_string()))?;

    let elapsed = t.elapsed().as_millis() as i64;
    log_activity(
        &state, &state.tenant_id, "graph", "validate",
        if has_errors { "failure" } else { "success" },
        &format!("Validated graph '{}' ({} issues)", id, issues.len()),
        None, Some(elapsed),
    );

    Ok(Json(json!({
        "id": id,
        "ok": parsed_ok && !has_errors,
        "issues": issues,
    })))
}

/// `POST /api/graphs/:id/build` — parse + validate the stored TOML,
/// build a `graph::Graph` against the tenant DuckDB, and atomically
/// swap the result into `state.graphs[id]`. DuckDB I/O runs inside
/// `spawn_blocking` so the axum runtime stays free for other requests
/// while a large build (bealls ≈ 10–15s) is in flight.
///
/// This endpoint is the v2 entry point during the staged cutover.
/// Existing v1 paths (`pl_build_article_graph`, `handlers::graph_articles::*`,
/// `services::article_graph_grpc`) continue to flow through the
/// `article_graph` slot on `AppState` unchanged; the eventual handler
/// swap happens once the v2 engine reaches feature parity for
/// cross-edges / traversal / projection.
pub async fn build_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();

    // 1. Read the TOML body from SQLite.
    let row = state
        .db
        .query_one(
            "SELECT toml_text FROM graphs WHERE id = ?1",
            &[&id as &dyn rusqlite::types::ToSql],
        )
        .map_err(|_| err(404, "Graph not found"))?;
    let toml_text = row["toml_text"].as_str().unwrap_or("").to_string();

    // 2. Parse + validate. Validation re-runs at build time even
    // though the UI save path validates too — the underlying DuckDB
    // catalog may have drifted (column renames, dropped tables) since
    // last validate, and we'd rather fail fast here with a clear
    // error than crash mid-build.
    let spec = graph::from_toml(&toml_text)
        .map_err(|e| err(400, &format!("parse: {e:#}")))?;
    let issues = graph::validate(&spec);
    let errors: Vec<&ValidationIssue> = issues
        .iter()
        .filter(|i| matches!(i.severity, Severity::Error))
        .collect();
    if !errors.is_empty() {
        return Err(err(
            400,
            &format!(
                "validation failed with {} error(s); run POST /api/graphs/:id/validate for details",
                errors.len()
            ),
        ));
    }

    // 3. Build off the runtime. `Arc<GraphSpec>` lets the closure own
    // its copy without cloning the (potentially large) IndexMaps inside.
    let duckdb_path = state.duckdb_path.clone();
    let spec_arc = Arc::new(spec);
    let spec_for_build = spec_arc.clone();
    let build_result: anyhow::Result<(graph::Graph, graph::BuildStats)> =
        tokio::task::spawn_blocking(move || {
            let reader = graph::source::duckdb::DuckDbSourceReader::open(&duckdb_path)?;
            graph::build_graph(&spec_for_build, &reader, 1)
        })
        .await
        .map_err(|e| err(500, &format!("join: {e}")))?;
    let (graph, stats) = build_result.map_err(|e| err(500, &format!("build: {e:#}")))?;

    // 4. Publish atomically. ArcSwap means existing readers keep the
    // old snapshot until they re-load, so an in-flight query against
    // a previous build doesn't see a half-built new one.
    let slot: GraphSlot = {
        let mut graphs = state.graphs.write().await;
        graphs
            .entry(id.clone())
            .or_insert_with(|| Arc::new(arc_swap::ArcSwapOption::from(None)))
            .clone()
    };
    slot.store(Some(Arc::new(graph)));

    let elapsed = t.elapsed().as_millis() as i64;
    log_activity(
        &state, &state.tenant_id, "graph", "build", "success",
        &format!(
            "Built graph '{}' ({} nodes, {} primary metrics, {}ms)",
            id, stats.total_nodes, stats.primary_metric_count, stats.elapsed_ms
        ),
        None, Some(elapsed),
    );

    Ok(Json(json!({
        "id": id,
        "ok": true,
        "stats": {
            "total_nodes": stats.total_nodes,
            "primary_metric_count": stats.primary_metric_count,
            "composite_metric_count": stats.composite_metric_count,
            "strings_interned": stats.strings_interned,
            "elapsed_ms": stats.elapsed_ms,
            "nodes_by_kind": stats.nodes_by_kind,
        }
    })))
}

/// `POST /api/graphs/serialize` — stateless GraphSpec JSON → TOML.
/// Mirror of `/parse` going the other direction. Used by FormView
/// to round-trip form mutations back to `toml_text` on save —
/// comments in the original TOML are lost (the parsed `GraphSpec`
/// has no comment carriers); pure-TOML editing via the Advanced
/// tab is the path that preserves comments.
///
/// Body: `{ "spec": <GraphSpec JSON> }`. Response:
/// `{ "ok": bool, "toml_text": "..."?, "error": "..."? }`.
pub async fn serialize_handler(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let spec_value = match body.get("spec") {
        Some(v) => v.clone(),
        None => {
            return Ok(Json(json!({
                "ok": false,
                "error": "missing `spec` field",
            })));
        }
    };
    let spec: crate::graph::spec::GraphSpec = match serde_json::from_value(spec_value) {
        Ok(s) => s,
        Err(e) => {
            return Ok(Json(json!({
                "ok": false,
                "error": format!("spec deserialize: {e}"),
            })));
        }
    };
    match toml::to_string(&spec) {
        Ok(toml_text) => Ok(Json(json!({
            "ok": true,
            "toml_text": toml_text,
        }))),
        Err(e) => Ok(Json(json!({
            "ok": false,
            "error": format!("serialize: {e}"),
        }))),
    }
}

/// `POST /api/graphs/parse` — stateless TOML → JSON. Frontend
/// GraphDesigner calls this on a debounced timer while the user
/// edits, so the schema-sketch visualization can re-render without
/// committing changes or hitting the heavier `validate` path.
///
/// Body: `{ "toml_text": "..." }`. Response: `{ "ok": bool, "spec": <parsed>?, "error": "..."? }`.
pub async fn parse_handler(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let toml_text = body
        .get("toml_text")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if toml_text.trim().is_empty() {
        return Ok(Json(json!({
            "ok": false,
            "error": "toml_text is empty",
        })));
    }
    match graph::from_toml(toml_text) {
        Ok(spec) => {
            // Serialize via serde_json — the spec structs derive
            // Serialize, so this round-trips to the wire shape the
            // frontend visualization expects.
            match serde_json::to_value(&spec) {
                Ok(spec_json) => Ok(Json(json!({
                    "ok": true,
                    "spec": spec_json,
                }))),
                Err(e) => Ok(Json(json!({
                    "ok": false,
                    "error": format!("serialize: {e}"),
                }))),
            }
        }
        Err(e) => Ok(Json(json!({
            "ok": false,
            "error": format!("{e:#}"),
        }))),
    }
}

/// `POST /api/graphs/:id/memory-stats` — heuristic memory breakdown
/// for the live v2 snapshot, mirrored to the same JSON shape as
/// `POST /api/article-graph/memory-stats` so the frontend can render
/// both engines with shared code. See `graph::memory::memory_stats`
/// for the field-by-field accounting.
pub async fn memory_stats_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let slot: Option<GraphSlot> = {
        let graphs = state.graphs.read().await;
        graphs.get(&id).cloned()
    };
    let slot = slot.ok_or_else(|| err(404, "graph not built — call POST /api/graphs/:id/build"))?;
    let snapshot = slot.load();
    let Some(graph) = snapshot.as_ref() else {
        return Err(err(404, "graph slot is empty"));
    };
    let started = Instant::now();
    let mut out = crate::graph::memory::memory_stats(graph);
    if let Some(obj) = out.as_object_mut() {
        obj.insert("id".to_string(), json!(id));
        obj.insert(
            "duration_ms".to_string(),
            json!(started.elapsed().as_millis() as i64),
        );
    }
    Ok(Json(out))
}

/// `GET /api/graphs/:id/stats` — surface the currently-live snapshot's
/// kind counts + metric registry. 404 when the graph hasn't been built
/// since boot (no entry in `state.graphs` or an empty `ArcSwapOption`).
pub async fn stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let slot: Option<GraphSlot> = {
        let graphs = state.graphs.read().await;
        graphs.get(&id).cloned()
    };
    let slot = slot.ok_or_else(|| err(404, "graph not built — call POST /api/graphs/:id/build"))?;
    let snapshot = slot.load();
    let Some(graph) = snapshot.as_ref() else {
        return Err(err(404, "graph slot is empty"));
    };

    // Inventory each kind + each metric. Cheap enough to surface
    // synchronously (registry sizes are O(10s)).
    let kinds: Vec<Value> = graph
        .kinds
        .iter()
        .map(|(kid, meta)| {
            json!({
                "name": meta.name,
                "hierarchy": meta.hierarchy,
                "node_count": graph.count_kind(kid),
            })
        })
        .collect();
    let metrics: Vec<Value> = graph
        .metrics
        .iter()
        .map(|(_, m)| {
            json!({
                "name": m.name,
                "source": m.source_alias,
                "column": m.column,
                "rollup": format!("{:?}", m.rollup),
                "is_composite": m.is_composite,
            })
        })
        .collect();
    // Cross-edges (bridge sources). Each entry is keyed by the bridge
    // source alias — that's the same id the `Edge::CrossEdge(alias)`
    // traversal call expects, so the frontend ExplorePane can wire
    // a button per cross-edge directly off this list.
    let cross_edges: Vec<Value> = graph
        .cross_edges
        .metas
        .iter()
        .map(|meta| {
            json!({
                "alias": meta.bridge_source,
                "kind_a": graph.kinds.get(meta.kind_a).name,
                "kind_b": graph.kinds.get(meta.kind_b).name,
            })
        })
        .collect();

    Ok(Json(json!({
        "id": id,
        "graph_version": graph.graph_version,
        "node_count": graph.node_count(),
        "string_count": graph.string_pool.len(),
        "kinds": kinds,
        "metrics": metrics,
        "cross_edges": cross_edges,
    })))
}

// ──────────────────────────────────────────────────────────────────────────
// Traversal
// ──────────────────────────────────────────────────────────────────────────
// Wire types live in `crate::service::graphs` so the agent's tool layer can
// construct them too without duplicating the Deserialize impls.
pub use crate::service::graphs::{
    CrossFilterQuery, EdgeRequest, FromRef, NodeRequest, TraverseRequest,
};

/// `POST /api/graphs/:id/traverse` — generic graph traversal against
/// the live v2 snapshot. Resolves `from` via `Graph::find_by_name`,
/// walks the requested edge, then projects each `NodeId` per the
/// `project` opt-in flags. Returns `{ "rows": [...] }`.
///
/// 404 when the graph hasn't been built since boot; 400 when the
/// `from` kind isn't a registered level; 404 when the `from` name
/// doesn't resolve to a node.
pub async fn traverse_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TraverseRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    crate::service::graphs::traverse_fn(&state, &id, req)
        .await
        .map(Json)
        .map_err(crate::service::error::into_http)
}

pub async fn node_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<NodeRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    crate::service::graphs::node(&state, &id, req)
        .await
        .map(Json)
        .map_err(crate::service::error::into_http)
}

pub async fn cross_filter_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<CrossFilterQuery>,
    Json(payload): Json<FilterPayload>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    crate::service::graphs::cross_filter(&state, &id, q, payload)
        .await
        .map(Json)
        .map_err(crate::service::error::into_http)
}
