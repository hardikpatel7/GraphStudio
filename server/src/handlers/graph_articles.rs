//! HTTP bridge for the article_graph (V8) gRPC service.
//!
//! Calls `services::graph_articles_grpc::ArticleGraphGrpcService` in
//! process — no network round-trip, no tonic-web. Frontend consumes
//! these as plain JSON over Axum on port 3002.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{Json, extract::State};
use serde_json::{Value, json};
use tonic::Request;

use crate::AppState;
use crate::services::graph_articles_grpc::{
    ArticleGraphGrpcService,
    proto::{
        AggregateAtRequest, MatchProductRequest, NodeKind, ResolveRclRequest, RuleKind,
        article_graph_service_server::ArticleGraphService,
    },
};

use super::err;

/// `POST /api/article-graph/match-product`
/// Body: `{ "product_code": "..." }` or `{ "article": "..." }`
pub async fn match_product(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let svc = ArticleGraphGrpcService::new(state);
    let req = MatchProductRequest {
        key: extract_key(&body),
    };
    let resp = svc
        .match_product(Request::new(req))
        .await
        .map_err(|s| err(map_status(&s), s.message()))?;
    Ok(Json(serde_json::to_value(resp.into_inner()).unwrap_or(json!({}))))
}

/// `POST /api/article-graph/resolve-rcl`
/// Body: `{ "product_code": "..." }` or `{ "article": "..." }`,
/// optional `"kinds": ["dc_policy","constraints","psm"]`.
pub async fn resolve_rcl(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let svc = ArticleGraphGrpcService::new(state);
    let kinds = body["kinds"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|s| match s {
                    "dc_policy" => Some(RuleKind::DcPolicy as i32),
                    "constraints" => Some(RuleKind::Constraints as i32),
                    "psm" => Some(RuleKind::Psm as i32),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let req = ResolveRclRequest {
        key: extract_key_for_resolve(&body),
        kinds,
    };
    let resp = svc
        .resolve_rcl(Request::new(req))
        .await
        .map_err(|s| err(map_status(&s), s.message()))?;
    Ok(Json(serde_json::to_value(resp.into_inner()).unwrap_or(json!({}))))
}

/// `POST /api/graph/traverse`
/// Body: `{ "from": {"kind":"ARTICLE","name":"…"}, "edge": "children" }`
/// Returns rows of the destination nodes, projected the same way
/// DataView preview projects them. Each output row is itself a thing
/// the caller can traverse from — clicks chain.
pub async fn traverse(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    use crate::graph::legacy::traverse::{traverse as do_traverse, TraverseRequest};
    let req: TraverseRequest = serde_json::from_value(body)
        .map_err(|e| err(400, &format!("invalid traverse request: {e}")))?;
    let graph = state.legacy_graph.load_full().ok_or_else(|| {
        err(503, "article_graph not built yet — run pipeline pl_build_article_graph first")
    })?;
    let ruleset = snapshot_ruleset(&state);
    let started = std::time::Instant::now();
    let rows = do_traverse(&graph, &req, ruleset.as_deref()).map_err(|e| err(400, &e))?;
    let elapsed = started.elapsed().as_millis() as i64;
    Ok(Json(json!({
        "rows": rows,
        "total": rows.len() as i64,
        "duration_ms": elapsed,
    })))
}

/// `POST /api/article-graph/aggregate-at`
/// Body: `{ "kind": "L1", "name": "3510-LADIES FOOTWEAR" }`
pub async fn aggregate_at(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let svc = ArticleGraphGrpcService::new(state);
    let kind_str = body["kind"].as_str().unwrap_or("");
    let kind = match kind_str.to_ascii_uppercase().as_str() {
        "L0" => NodeKind::L0,
        "L1" => NodeKind::L1,
        "L2" => NodeKind::L2,
        "L3" => NodeKind::L3,
        "L4" => NodeKind::L4,
        "L5" => NodeKind::L5,
        "ARTICLE" => NodeKind::Article,
        "PRODUCT_CODE" | "PRODUCTCODE" => NodeKind::ProductCode,
        "CHANNEL" => NodeKind::Channel,
        "STORE_CODE" | "STORECODE" => NodeKind::StoreCode,
        _ => return Err(err(400, &format!("invalid kind '{}'", kind_str))),
    };
    let name = body["name"].as_str().unwrap_or("").to_string();
    if name.is_empty() {
        return Err(err(400, "name is required"));
    }
    let req = AggregateAtRequest {
        kind: kind as i32,
        name,
    };
    let resp = svc
        .aggregate_at(Request::new(req))
        .await
        .map_err(|s| err(map_status(&s), s.message()))?;
    Ok(Json(serde_json::to_value(resp.into_inner()).unwrap_or(json!({}))))
}

/// Extract the product_code/article oneof key for `MatchProductRequest`
/// from a JSON body. Returns `None` if neither field is present.
fn extract_key(
    body: &Value,
) -> Option<crate::services::graph_articles_grpc::proto::match_product_request::Key> {
    use crate::services::graph_articles_grpc::proto::match_product_request::Key;
    if let Some(pc) = body["product_code"].as_str().filter(|s| !s.is_empty()) {
        return Some(Key::ProductCode(pc.to_string()));
    }
    if let Some(art) = body["article"].as_str().filter(|s| !s.is_empty()) {
        return Some(Key::Article(art.to_string()));
    }
    None
}

fn extract_key_for_resolve(
    body: &Value,
) -> Option<crate::services::graph_articles_grpc::proto::resolve_rcl_request::Key> {
    use crate::services::graph_articles_grpc::proto::resolve_rcl_request::Key;
    if let Some(pc) = body["product_code"].as_str().filter(|s| !s.is_empty()) {
        return Some(Key::ProductCode(pc.to_string()));
    }
    if let Some(art) = body["article"].as_str().filter(|s| !s.is_empty()) {
        return Some(Key::Article(art.to_string()));
    }
    None
}

/// Map a tonic Status code to an HTTP status integer.
fn map_status(s: &tonic::Status) -> u16 {
    match s.code() {
        tonic::Code::InvalidArgument => 400,
        tonic::Code::NotFound => 404,
        tonic::Code::FailedPrecondition => 503,
        tonic::Code::Unimplemented => 501,
        _ => 500,
    }
}

/// `POST /api/article-graph/memory-stats`
/// Body: `{}` (no params).
/// Returns a heuristic memory breakdown of the in-memory ArticleGraph.
///
/// Numbers are approximations — Rust doesn't expose live allocator
/// telemetry per object. We sum `mem::size_of::<T>() × len` plus
/// rough estimates for heap-resident strings (Arc<str>, String) and
/// HashMap bucket overhead (1.5× entry size). Good enough to spot
/// the dominant cost; not authoritative for capacity planning.
pub async fn memory_stats(
    State(state): State<Arc<AppState>>,
    Json(_body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    use crate::graph::legacy::{NodeKind as GraphNodeKind, RulePtr, Node};
    use std::mem::size_of;

    let graph = state.legacy_graph.load_full().ok_or_else(|| {
        err(503, "article_graph not built yet — run pipeline pl_build_article_graph first")
    })?;
    let started = std::time::Instant::now();

    // ── Nodes (per-kind breakdown) ──
    // The arena is `Vec<Node>`, all nodes interleaved. We bucket by
    // kind for the chart; struct cost is identical per node, so the
    // variation comes from heap-allocated `children` and
    // `rule_pointers` (SmallVec spills past its inline capacity).
    let mut by_kind_count = [0usize; 8];
    let mut by_kind_heap = [0usize; 8];
    for n in &graph.nodes {
        let idx = n.kind as usize;
        if idx < by_kind_count.len() {
            by_kind_count[idx] += 1;
            // SmallVec inline capacity: children=4, rule_pointers=3.
            // Anything beyond that is a heap allocation.
            if n.children.len() > 4 {
                by_kind_heap[idx] += n.children.capacity() * size_of::<crate::graph::legacy::NodeId>();
            }
            if n.rule_pointers.len() > 3 {
                by_kind_heap[idx] += n.rule_pointers.capacity() * size_of::<RulePtr>();
            }
        }
    }
    let node_struct_size = size_of::<Node>();
    let nodes_breakdown: Vec<Value> = (0..8usize)
        .filter_map(|i| {
            let kind = match i {
                0 => Some("ROOT"),
                1 => Some("L0"),
                2 => Some("L1"),
                3 => Some("L2"),
                4 => Some("L3"),
                5 => Some("L4"),
                6 => Some("L5"),
                7 => Some("ARTICLE"),
                _ => None,
            }?;
            let count = by_kind_count[i];
            let bytes_struct = count * node_struct_size;
            let bytes_heap = by_kind_heap[i];
            Some(json!({
                "kind": kind,
                "count": count as i64,
                "bytes_struct": bytes_struct as i64,
                "bytes_heap": bytes_heap as i64,
                "bytes_total": (bytes_struct + bytes_heap) as i64,
            }))
        })
        .collect();
    // Above only covers 8 kinds; the actual NodeKind enum has more
    // variants (PRODUCT_CODE / CHANNEL / STORE_CODE). Walk again with
    // the real enum so the report includes all of them.
    let mut full_by_kind: std::collections::HashMap<&'static str, (usize, usize)> =
        std::collections::HashMap::new();
    for n in &graph.nodes {
        let label = match n.kind {
            GraphNodeKind::Root => "ROOT",
            GraphNodeKind::L0 => "L0",
            GraphNodeKind::L1 => "L1",
            GraphNodeKind::L2 => "L2",
            GraphNodeKind::L3 => "L3",
            GraphNodeKind::L4 => "L4",
            GraphNodeKind::L5 => "L5",
            GraphNodeKind::Article => "ARTICLE",
            GraphNodeKind::ProductCode => "PRODUCT_CODE",
            GraphNodeKind::Channel => "CHANNEL",
            GraphNodeKind::StoreCode => "STORE_CODE",
        };
        let entry = full_by_kind.entry(label).or_insert((0, 0));
        entry.0 += 1;
        if n.children.len() > 4 {
            entry.1 += n.children.capacity() * size_of::<crate::graph::legacy::NodeId>();
        }
        if n.rule_pointers.len() > 3 {
            entry.1 += n.rule_pointers.capacity() * size_of::<RulePtr>();
        }
    }
    let mut nodes_per_kind: Vec<Value> = full_by_kind
        .into_iter()
        .map(|(k, (count, heap))| {
            let bytes_struct = count * node_struct_size;
            json!({
                "kind": k,
                "count": count as i64,
                "bytes_struct": bytes_struct as i64,
                "bytes_heap": heap as i64,
                "bytes_total": (bytes_struct + heap) as i64,
            })
        })
        .collect();
    nodes_per_kind.sort_by(|a, b| {
        b["bytes_total"].as_i64().unwrap_or(0)
            .cmp(&a["bytes_total"].as_i64().unwrap_or(0))
    });
    let nodes_total_bytes: i64 = nodes_per_kind
        .iter()
        .map(|v| v["bytes_total"].as_i64().unwrap_or(0))
        .sum();

    // Suppress unused-warning on the per-8 buckets (they were only the
    // first pass; the full walk is the canonical breakdown).
    let _ = nodes_breakdown;

    // ── String pool ──
    // Every interned string is an Arc<str>. The struct itself is two
    // pointers; the heap allocation is the str bytes plus a small
    // ArcInner header (16 bytes for strong/weak refcounts on 64-bit).
    let str_count = graph.string_pool.len();
    let str_total_chars: usize = graph.string_pool.iter().map(|s| s.len()).sum();
    // Arc heap allocation overhead: 16 bytes header + the str bytes.
    let str_heap_bytes = str_total_chars + str_count * 16;
    let str_struct_bytes = str_count * size_of::<Arc<str>>();
    let strings_total = str_heap_bytes + str_struct_bytes;

    // ── by_kind index ──
    // 8 HashMaps from StrId → NodeId. Approximate the per-entry cost
    // at (key + value + 1.5× bucket overhead).
    let mut by_kind_entries = 0usize;
    for m in graph.by_kind.iter() {
        by_kind_entries += m.len();
    }
    let by_kind_per_entry = size_of::<crate::graph::legacy::StrId>()
        + size_of::<crate::graph::legacy::NodeId>();
    let by_kind_bytes = (by_kind_entries as f64 * by_kind_per_entry as f64 * 1.5) as i64;

    // ── Cross indices ──
    // Each entry is HashMap<KeyType, ValueType>, value is a Vec or
    // SmallVec. Approximate similar to by_kind but include the value
    // payloads.
    let xi = &graph.cross_indices;
    let brand_to_articles_entries = xi.brand_to_articles.len();
    let brand_to_articles_values: usize = xi.brand_to_articles.values().map(|v| v.len()).sum();
    let article_to_brand_entries = xi.article_to_brand.len();
    let article_to_channel_entries = xi.article_to_channel.len();
    let product_code_to_dcs_entries = xi.product_code_to_dcs.len();
    let store_code_to_dcs_entries = xi.store_code_to_dcs.len();
    let store_code_to_sgs_entries = xi.store_code_to_sgs.len();

    let approx_map_bytes = |entries: usize, key: usize, value: usize| -> i64 {
        ((entries as f64) * ((key + value) as f64) * 1.5) as i64
    };
    let cross_indices_breakdown = vec![
        json!({
            "name": "brand_to_articles",
            "entries": brand_to_articles_entries as i64,
            "value_total": brand_to_articles_values as i64,
            "bytes_total":
                approx_map_bytes(brand_to_articles_entries, 4, 24)
                + brand_to_articles_values as i64 * size_of::<crate::graph::legacy::NodeId>() as i64,
        }),
        json!({
            "name": "article_to_brand",
            "entries": article_to_brand_entries as i64,
            "bytes_total": approx_map_bytes(article_to_brand_entries,
                size_of::<crate::graph::legacy::NodeId>(),
                size_of::<crate::graph::legacy::StrId>()),
        }),
        json!({
            "name": "article_to_channel",
            "entries": article_to_channel_entries as i64,
            "bytes_total": approx_map_bytes(article_to_channel_entries,
                size_of::<crate::graph::legacy::NodeId>(),
                size_of::<crate::graph::legacy::StrId>()),
        }),
        json!({
            "name": "product_code_to_dcs",
            "entries": product_code_to_dcs_entries as i64,
            "bytes_total": approx_map_bytes(product_code_to_dcs_entries,
                size_of::<crate::graph::legacy::StrId>(), 32),
        }),
        json!({
            "name": "store_code_to_dcs",
            "entries": store_code_to_dcs_entries as i64,
            "bytes_total": approx_map_bytes(store_code_to_dcs_entries,
                size_of::<crate::graph::legacy::StrId>(), 32),
        }),
        json!({
            "name": "store_code_to_sgs",
            "entries": store_code_to_sgs_entries as i64,
            "bytes_total": approx_map_bytes(store_code_to_sgs_entries,
                size_of::<crate::graph::legacy::StrId>(), 32),
        }),
    ];
    let cross_indices_total: i64 = cross_indices_breakdown
        .iter()
        .map(|v| v["bytes_total"].as_i64().unwrap_or(0))
        .sum();

    // ── PSM resolver (on-the-fly) ──
    // priorities: Vec<(String, i32)>
    // by_rcl: HashMap<rcl_code, RclIndex { buckets: Vec<RclBucket> }>
    //   each bucket = { schema_fields: Vec<String>, by_tuple: HashMap<Vec<String>, String> }
    let psm_priorities = graph.psm.priorities.len();
    let psm_rcl_codes = graph.psm.by_rcl.len();
    let psm_buckets: usize = graph.psm.by_rcl.values().map(|i| i.buckets.len()).sum();
    let psm_rule_count: usize = graph
        .psm
        .by_rcl
        .values()
        .flat_map(|i| i.buckets.iter())
        .map(|b| b.by_tuple.len())
        .sum();
    let psm_priorities_bytes = psm_priorities * (24 + 24 + 4);
    // Each rule entry: tuple of N strings (avg N=3 fields, 24B each)
    // + rule_code String. ~120B per entry × 1.5× HashMap overhead.
    let psm_rule_bytes = (psm_rule_count as f64 * 120.0 * 1.5) as i64;
    let psm_total = psm_priorities_bytes as i64 + psm_rule_bytes;
    // Kept-for-wire-compat ints so the existing memory_stats response
    // shape doesn't break the frontend; new on-the-fly numbers are
    // exposed via the additional fields below.
    let psm_pc_count: usize = 0;
    let psm_pc_inner_total: usize = 0;
    let psm_rule_dim_entries: usize = psm_rule_count;

    // ── Grand total ──
    let grand_total = nodes_total_bytes
        + strings_total as i64
        + by_kind_bytes
        + cross_indices_total
        + psm_total;

    Ok(Json(json!({
        "graph_version": graph.graph_version,
        "rule_pointers_version": graph.rule_pointers_version,
        "duration_ms": started.elapsed().as_millis() as i64,
        "node_struct_size_bytes": node_struct_size as i64,

        "nodes": {
            "by_kind": nodes_per_kind,
            "total_count": graph.nodes.len() as i64,
            "total_bytes": nodes_total_bytes,
        },
        "strings": {
            "count": str_count as i64,
            "total_chars": str_total_chars as i64,
            "struct_bytes": str_struct_bytes as i64,
            "heap_bytes": str_heap_bytes as i64,
            "total_bytes": strings_total as i64,
        },
        "by_kind_index": {
            "kinds": graph.by_kind.len() as i64,
            "entries": by_kind_entries as i64,
            "total_bytes": by_kind_bytes,
        },
        "cross_indices": {
            "breakdown": cross_indices_breakdown,
            "total_bytes": cross_indices_total,
        },
        "psm": {
            "priorities": psm_priorities as i64,
            "rule_dim_entries": psm_rule_dim_entries as i64,
            "products_with_rcl_hash": psm_pc_count as i64,
            "inner_hash_entries_total": psm_pc_inner_total as i64,
            "total_bytes": psm_total,
        },
        "grand_total_bytes": grand_total,
    })))
}

/// `POST /api/article-graph/brands`
/// Body: `{ "limit"?: N }` — if omitted, returns every brand.
/// Returns `{ "brands": [{ "name": "...", "article_count": N, "oh": N,
///   "lw_units": N, "lw_revenue": N }] }`. Sorted by oh DESC so the
/// dominant brands surface first.
///
/// Used by the Detail View tab's "Brands" pseudo-root: one row per
/// brand, ranked by total OH so the operator can scan the most-stocked
/// brands at the top.
pub async fn brands_list(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    use crate::graph::legacy::MetricKind;
    let limit = body
        .get("limit")
        .and_then(|v| v.as_i64())
        .filter(|n| *n > 0)
        .map(|n| n as usize);
    let graph = state.legacy_graph.load_full().ok_or_else(|| {
        err(503, "article_graph not built yet — run pipeline pl_build_article_graph first")
    })?;
    let started = std::time::Instant::now();

    // Per-brand aggregation. brand_to_articles is HashMap<StrId, Vec<NodeId>>;
    // we walk each entry once and roll up the article-level metrics.
    let mut rows: Vec<Value> = graph
        .cross_indices
        .brand_to_articles
        .iter()
        .map(|(brand_id, article_ids)| {
            let name = graph.get_str(*brand_id).to_string();
            let mut oh = 0i64;
            let mut lw_units = 0i64;
            let mut lw_revenue = 0i64;
            for &id in article_ids {
                let m = &graph.node(id).metrics;
                oh += m[MetricKind::Oh.idx()] as i64;
                lw_units += m[MetricKind::LwUnits.idx()] as i64;
                lw_revenue += m[MetricKind::LwRevenue.idx()] as i64;
            }
            json!({
                "name": name,
                "article_count": article_ids.len() as i64,
                "oh": oh,
                "lw_units": lw_units,
                "lw_revenue": lw_revenue,
            })
        })
        .collect();
    // Sort by OH DESC so the most-stocked brands surface at the top.
    rows.sort_by(|a, b| {
        let ax = a.get("oh").and_then(|v| v.as_i64()).unwrap_or(0);
        let bx = b.get("oh").and_then(|v| v.as_i64()).unwrap_or(0);
        bx.cmp(&ax)
    });
    if let Some(n) = limit { rows.truncate(n); }
    Ok(Json(json!({
        "brands": rows,
        "duration_ms": started.elapsed().as_millis() as i64,
    })))
}

/// `POST /api/article-graph/article-detail`
/// Body: `{ "article": "..." }` (or `{ "product_code": "..." }`).
/// Returns a single bundled payload with everything the Detail View
/// pane needs for an article focus: hierarchy, rolled-up metrics,
/// RCL trace, per-size OH, and exception flags. One round-trip.
pub async fn article_detail(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    use crate::graph::legacy::projection::project_single;
    use crate::graph::legacy::NodeKind as GraphNodeKind;
    use crate::graph::legacy::exception::flag_article;

    let graph = state.legacy_graph.load_full().ok_or_else(|| {
        err(503, "article_graph not built yet — run pipeline pl_build_article_graph first")
    })?;

    // Resolve key (article OR product_code) to article NodeId. Mirrors
    // the gRPC ResolveRcl path's resolve_key, but inline for the HTTP
    // bundle.
    let pc = body.get("product_code").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    let article = body.get("article").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    let article_node_id = {
        let needle = if let Some(pc) = pc {
            pc
        } else if let Some(a) = article {
            a
        } else {
            return Err(err(400, "missing 'article' or 'product_code'"));
        };
        let str_id = match graph.string_pool.iter().position(|s| s.as_ref() == needle) {
            Some(i) => crate::graph::legacy::StrId(i as u32),
            None => return Err(err(404, &format!("'{}' not found in graph", needle))),
        };
        let kind_to_find = if pc.is_some() {
            GraphNodeKind::ProductCode
        } else {
            GraphNodeKind::Article
        };
        let id = graph
            .find(kind_to_find, str_id)
            .ok_or_else(|| err(404, "node not found"))?;
        if pc.is_some() {
            graph.node(id).parent
        } else {
            id
        }
    };

    let started = std::time::Instant::now();
    // Snapshot ruleset so the projection populates rcl-resolved cols.
    let ruleset = snapshot_ruleset(&state);

    // Article row (hierarchy + brand + channel + metrics + rcl-resolved cols).
    let row = project_single(&graph, GraphNodeKind::Article, article_node_id, ruleset.as_deref())
        .unwrap_or(json!({}));

    // RCL trace via the gRPC service so dc_policy / constraints / psm
    // come out in the same shape /resolve-rcl returns.
    let svc = ArticleGraphGrpcService::new(state.clone());
    let resolve_req = ResolveRclRequest {
        kinds: vec![],
        key: Some(crate::services::graph_articles_grpc::proto::resolve_rcl_request::Key::Article(
            graph.get_str(graph.node(article_node_id).name).to_string(),
        )),
    };
    let rcl = svc
        .resolve_rcl(Request::new(resolve_req))
        .await
        .ok()
        .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(json!({})))
        .unwrap_or(json!({}));

    // Risk flags — same predicates the Exception View uses.
    let flags = flag_article(&graph, ruleset.as_deref(), article_node_id);
    let flag_strs: Vec<Value> = flags.iter().map(|r| json!(r.as_wire())).collect();

    // Per-size OH via the same DuckDB join the exception list uses.
    let article_name = graph.get_str(graph.node(article_node_id).name).to_string();
    let duckdb_path = state.duckdb_path.clone();
    let sizes: Vec<Value> = tokio::task::spawn_blocking(move || -> Vec<Value> {
        let by_article = fetch_sizes_blocking(&duckdb_path, &[article_name.clone()])
            .unwrap_or_default();
        by_article
            .get(&article_name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(s, oh)| json!({ "size": s, "oh": oh }))
            .collect()
    })
    .await
    .unwrap_or_default();

    Ok(Json(json!({
        "article": graph.get_str(graph.node(article_node_id).name),
        "row": row,
        "rcl": rcl,
        "sizes": sizes,
        "risk_flags": flag_strs,
        "duration_ms": started.elapsed().as_millis() as i64,
    })))
}

// ── Exception view (Phase 1) ───────────────────────────────────────────────
//
// Two routes back the new "Exception View" tab:
//   - `/exceptions/counts` powers the chip badges (counts per rule, AND-
//     composed with the active cross-filter selections).
//   - `/exceptions/list` returns the matching article rows for one or
//     more selected rule chips. Reuses `project_single` so the row
//     payload is identical to what the Live View renders.

#[derive(serde::Deserialize, Default)]
struct ExceptionRequest {
    #[serde(default)]
    filters: Vec<crate::cross_filter::model::Filter>,
    /// For `/list`: which rule chips are selected (OR-within). Empty
    /// list = no filter on rules (return all flagged articles).
    #[serde(default)]
    rules: Vec<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

fn snapshot_ruleset(state: &AppState) -> Option<std::sync::Arc<rcl::RuleSet>> {
    // The rcl_store may be absent (rcl service not configured) — that's
    // fine, the rule predicates that need it just won't fire.
    let guard = match state.rcl_store.try_read() {
        Ok(g) => g,
        Err(_) => return None,
    };
    guard.as_ref().map(|store| store.snapshot())
}

/// `POST /api/article-graph/exceptions/counts`
/// Body: `{ "filters": [...] }`
/// Returns `{ "total_articles": N, "counts": { "stockout": N, ... } }`.
pub async fn exceptions_counts(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    use crate::graph::legacy::exception::{count_exceptions, Rule};
    let req: ExceptionRequest = serde_json::from_value(body).unwrap_or_default();
    let graph = state.legacy_graph.load_full().ok_or_else(|| {
        err(503, "article_graph not built yet — run pipeline pl_build_article_graph first")
    })?;
    let candidates = if req.filters.is_empty() {
        None
    } else {
        Some(crate::cross_filter::resolver::apply_filters(
            &graph,
            &req.filters,
            None,
        ))
    };
    let ruleset = snapshot_ruleset(&state);
    let started = std::time::Instant::now();
    let counts = count_exceptions(&graph, ruleset.as_deref(), candidates.as_ref());
    let by_rule: serde_json::Map<String, Value> = Rule::ALL
        .iter()
        .map(|r| {
            (
                r.as_wire().to_string(),
                json!(counts.by_rule.get(r).copied().unwrap_or(0)),
            )
        })
        .collect();
    Ok(Json(json!({
        "total_articles": counts.total_articles,
        "counts": by_rule,
        "duration_ms": started.elapsed().as_millis() as i64,
    })))
}

/// `POST /api/article-graph/exceptions/list`
/// Body: `{ "filters": [...], "rules": ["stockout", "overstock"], "limit": N, "offset": N }`
/// Returns `{ "rows": [...], "total": N }`. Each row carries the same
/// payload as the Live View (article + hierarchy + rcl-resolved cols +
/// metrics) plus a `risk_flags` array listing which rules fired.
pub async fn exceptions_list(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    use crate::graph::legacy::exception::{list_exception_ids, Rule};
    use crate::graph::legacy::projection::project_single;
    use crate::graph::legacy::NodeKind as GraphNodeKind;

    let req: ExceptionRequest = serde_json::from_value(body).unwrap_or_default();
    let graph = state.legacy_graph.load_full().ok_or_else(|| {
        err(503, "article_graph not built yet — run pipeline pl_build_article_graph first")
    })?;
    let selected: Vec<Rule> = req
        .rules
        .iter()
        .filter_map(|s| Rule::from_wire(s.as_str()))
        .collect();
    if selected.is_empty() {
        return Ok(Json(json!({ "rows": [], "total": 0, "duration_ms": 0 })));
    }
    let candidates = if req.filters.is_empty() {
        None
    } else {
        Some(crate::cross_filter::resolver::apply_filters(
            &graph,
            &req.filters,
            None,
        ))
    };
    let ruleset = snapshot_ruleset(&state);
    let started = std::time::Instant::now();

    let mut hits = list_exception_ids(&graph, ruleset.as_deref(), &selected, candidates.as_ref());
    let total = hits.len() as i64;
    // Default sort: OH DESC. Operators want the biggest piles at the top
    // (overstock by largest stuck inventory; stockouts by lost demand).
    // Tiebreak by article name for determinism.
    hits.sort_by(|a, b| {
        use crate::graph::legacy::MetricKind;
        let ax = graph.node(a.0).metrics[MetricKind::Oh.idx()];
        let bx = graph.node(b.0).metrics[MetricKind::Oh.idx()];
        bx.partial_cmp(&ax).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
            let na = graph.get_str(graph.node(a.0).name);
            let nb = graph.get_str(graph.node(b.0).name);
            na.cmp(nb)
        })
    });
    let off = req.offset.unwrap_or(0).max(0) as usize;
    let lim = req.limit.unwrap_or(100).max(0) as usize;
    let page: Vec<(crate::graph::legacy::NodeId, smallvec::SmallVec<[crate::graph::legacy::exception::Rule; 4]>)>
        = hits.into_iter().skip(off).take(lim).collect();

    // Collect article names for the page so we can batch-query per-size OH.
    let page_articles: Vec<String> = page
        .iter()
        .map(|(id, _)| graph.get_str(graph.node(*id).name).to_string())
        .collect();

    // Project rows first (without sizes); we'll merge sizes in below.
    let mut rows: Vec<Value> = page
        .iter()
        .filter_map(|(id, flags)| {
            let mut row = project_single(&graph, GraphNodeKind::Article, *id, ruleset.as_deref())?;
            if let Some(obj) = row.as_object_mut() {
                let flag_strs: Vec<Value> = flags.iter().map(|r| json!(r.as_wire())).collect();
                obj.insert("risk_flags".to_string(), Value::Array(flag_strs));
                // Empty default; fill in below if the size lookup succeeds.
                obj.insert("sizes".to_string(), Value::Array(Vec::new()));
            }
            Some(row)
        })
        .collect();

    // Per-size OH for the page. One DuckDB round-trip joining
    // raw_ph_master (article → ph_code) with asv2_inventory_per_size_dc
    // (per-(ph_code, size) on-hand). Failure here doesn't break the
    // response — sizes just stay empty and the UI degrades gracefully.
    if !page_articles.is_empty() {
        let duckdb_path = state.duckdb_path.clone();
        let articles_owned = page_articles.clone();
        let by_article: HashMap<String, Vec<(String, i64)>> =
            tokio::task::spawn_blocking(move || -> HashMap<String, Vec<(String, i64)>> {
                fetch_sizes_blocking(&duckdb_path, &articles_owned).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
        for row in rows.iter_mut() {
            let Some(obj) = row.as_object_mut() else { continue };
            let Some(article) = obj.get("article").and_then(|v| v.as_str()) else { continue };
            if let Some(sizes) = by_article.get(article) {
                let arr: Vec<Value> = sizes
                    .iter()
                    .map(|(size, oh)| json!({ "size": size, "oh": oh }))
                    .collect();
                obj.insert("sizes".to_string(), Value::Array(arr));
            }
        }
    }

    Ok(Json(json!({
        "rows": rows,
        "total": total,
        "duration_ms": started.elapsed().as_millis() as i64,
    })))
}

/// Fetch per-(article, size) OH for the given articles, summed across
/// DCs. One DuckDB query joining raw_ph_master + asv2_inventory_per_size_dc.
/// Returns `article → [(size, oh)]`; sizes ordered ASC for stable display.
fn fetch_sizes_blocking(
    duckdb_path: &str,
    articles: &[String],
) -> Result<HashMap<String, Vec<(String, i64)>>, String> {
    if articles.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = duckdb::Connection::open(duckdb_path)
        .map_err(|e| format!("DuckDB open: {}", e))?;
    // Quote each article defensively. Single-tenant local file, advisory only.
    let in_list = articles
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT m.article, i.size, SUM(i.oh) AS oh \
         FROM raw_ph_master m \
         JOIN asv2_inventory_per_size_dc i ON i.ph_code = m.ph_code::VARCHAR \
         WHERE m.article IN ({}) \
         GROUP BY m.article, i.size \
         HAVING SUM(i.oh) > 0 \
         ORDER BY m.article, i.size",
        in_list
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, String>(1).unwrap_or_default(),
                row.get::<_, i128>(2).unwrap_or(0) as i64,
            ))
        })
        .map_err(|e| format!("query_map: {}", e))?;
    let mut out: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    for r in rows.flatten() {
        out.entry(r.0).or_default().push((r.1, r.2));
    }
    Ok(out)
}
