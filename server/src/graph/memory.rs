//! Heuristic memory breakdown for a v2 `Graph` snapshot.
//!
//! Wire-aligned with v1's `graph::articles::memory_stats`
//! response so the frontend `MemoryWorkspace` can render both engines
//! with the same code. Sections:
//!
//! - **nodes** — per-kind count + struct + heap (children/SmallVec spill)
//! - **strings** — interned `Arc<str>` pool
//! - **by_kind_index** — per-kind `StrId → NodeId` map
//! - **cross_indices** — registered cross-edges (keyed by bridge alias,
//!   v2's natural identifier — v1 names them inline since v1 has a
//!   fixed enum of edge kinds)
//! - **psm** — module-101 priority chain + parsed rule_dim index
//! - **composite_metrics** — sideband cube cells (always 0 in Phase 2)
//! - **rule_pointers** — per-article pre-bound RCL pointers
//!
//! Numbers are estimates (Rust has no per-object allocator telemetry):
//! `mem::size_of::<T>() × count` plus `1.5×` heuristic HashMap overhead
//! and 16-byte ArcInner headers for `Arc<str>`. Good enough to spot the
//! dominant contributor; not authoritative for capacity planning.

use std::mem::size_of;
use std::sync::Arc;

use serde_json::{Value, json};
use smallvec::SmallVec;

use super::graph::{Graph, KindId, NodeId, StrId};
use super::rcl::RulePtr;

/// Walk `graph` and emit the JSON breakdown. The caller (HTTP handler)
/// adds `id` + `graph_version` + `duration_ms` framing on top.
pub fn memory_stats(graph: &Graph) -> Value {
    // ── Nodes (per-kind) ──────────────────────────────────────────
    // Iterate the arena once. For each node, accumulate struct cost
    // (uniform per node) + heap cost from SmallVec spills on
    // `children` (inline capacity = 4). v2 doesn't carry rule_pointers
    // on `Node` itself — those live sideband on `Graph.rule_pointers`,
    // accounted for separately below.
    let node_struct_size = size_of::<super::graph::Node>();
    let mut per_kind_count: Vec<usize> = vec![0; graph.kinds.len()];
    let mut per_kind_heap: Vec<usize> = vec![0; graph.kinds.len()];
    for n in &graph.nodes {
        let i = n.kind.0 as usize;
        if i < per_kind_count.len() {
            per_kind_count[i] += 1;
            if n.children.len() > 4 {
                per_kind_heap[i] += n.children.capacity() * size_of::<NodeId>();
            }
            // `Node.metrics: Box<[MetricValue]>` — the slice itself is
            // sized per-graph and identical across nodes. We charge
            // it under the per-node struct line implicitly (size_of
            // includes the Box pointer pair). The actual heap content
            // of MetricValue::Set / List per node is below.
            for slot in n.metrics.iter() {
                use super::graph::MetricValue::*;
                per_kind_heap[i] += match slot {
                    Set(s) => s.capacity() * size_of::<StrId>(),
                    List(v) => v.capacity() * size_of::<StrId>(),
                    Scalar(_) | Bool(_) => 0,
                };
            }
        }
    }
    let mut nodes_per_kind: Vec<Value> = (0..graph.kinds.len())
        .map(|i| {
            let kid = KindId(i as u32);
            let meta = graph.kinds.get(kid);
            let count = per_kind_count[i];
            let bytes_struct = count * node_struct_size;
            let bytes_heap = per_kind_heap[i];
            json!({
                "kind": meta.name,
                "count": count as i64,
                "bytes_struct": bytes_struct as i64,
                "bytes_heap": bytes_heap as i64,
                "bytes_total": (bytes_struct + bytes_heap) as i64,
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

    // ── String pool ───────────────────────────────────────────────
    // Same accounting v1 uses: pointer pair per Arc<str> entry plus
    // 16-byte ArcInner header + the str bytes per allocation.
    let str_count = graph.string_pool.len();
    let str_total_chars: usize = graph.string_pool.iter().map(|s| s.len()).sum();
    let str_heap_bytes = str_total_chars + str_count * 16;
    let str_struct_bytes = str_count * size_of::<Arc<str>>();
    let strings_total = str_heap_bytes + str_struct_bytes;

    // ── by_kind index ─────────────────────────────────────────────
    // Per-kind HashMap<StrId, NodeId>. Heuristic: 1.5× (key + value)
    // for bucket overhead — same constant v1 uses.
    let mut by_kind_entries = 0usize;
    for kid in 0..graph.kinds.len() {
        by_kind_entries += graph.count_kind(KindId(kid as u32));
    }
    let by_kind_per_entry = size_of::<StrId>() + size_of::<NodeId>();
    let by_kind_bytes = (by_kind_entries as f64 * by_kind_per_entry as f64 * 1.5) as i64;

    // ── Cross-edge indices ────────────────────────────────────────
    // v2 names edges by their bridge source alias (one entry per
    // registered cross-edge). Each row reports entries on the
    // forward map and on the reverse map separately, since both are
    // populated by the build step.
    let mut cross_indices_breakdown: Vec<Value> = graph
        .cross_edges
        .metas
        .iter()
        .enumerate()
        .map(|(i, meta)| {
            let idx = graph.cross_edges.get(super::graph::CrossEdgeId(i as u32));
            let fwd_entries = idx.forward.len();
            let fwd_values: usize = idx.forward.values().map(|v| v.len()).sum();
            let rev_entries = idx.reverse.len();
            let rev_values: usize = idx.reverse.values().map(|v| v.len()).sum();
            let per_entry = size_of::<NodeId>() + size_of::<SmallVec<[NodeId; 4]>>();
            let bytes_maps =
                ((fwd_entries + rev_entries) as f64 * per_entry as f64 * 1.5) as i64;
            // Heap spill — SmallVec inline = 4; anything past spills.
            // Approximation: count over-4 in both maps × NodeId size.
            let spill_estimate = |m: &std::collections::HashMap<NodeId, SmallVec<[NodeId; 4]>>| -> i64 {
                let mut s = 0i64;
                for v in m.values() {
                    if v.len() > 4 {
                        s += (v.capacity() * size_of::<NodeId>()) as i64;
                    }
                }
                s
            };
            let bytes_spill = spill_estimate(&idx.forward) + spill_estimate(&idx.reverse);
            json!({
                "name": meta.bridge_source,
                "entries": (fwd_entries + rev_entries) as i64,
                "value_total": (fwd_values + rev_values) as i64,
                "bytes_total": bytes_maps + bytes_spill,
            })
        })
        .collect();
    cross_indices_breakdown.sort_by(|a, b| {
        b["bytes_total"].as_i64().unwrap_or(0)
            .cmp(&a["bytes_total"].as_i64().unwrap_or(0))
    });
    let cross_indices_total: i64 = cross_indices_breakdown
        .iter()
        .map(|v| v["bytes_total"].as_i64().unwrap_or(0))
        .sum();

    // ── PSM ───────────────────────────────────────────────────────
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
    let psm_rule_bytes = (psm_rule_count as f64 * 120.0 * 1.5) as i64;
    let psm_total = psm_priorities_bytes as i64 + psm_rule_bytes;
    // Wire-compat fields with v1's response so the frontend doesn't
    // branch on engine.
    let psm_pc_count: usize = 0;
    let psm_pc_inner_total: usize = 0;
    let psm_rule_dim_entries: usize = psm_rule_count;

    // ── Composite metrics (Phase 2 sideband; empty today) ────────
    let composite_entries = graph.composite_metrics.len();
    let composite_bytes: i64 = ((composite_entries as f64) * 64.0 * 1.5) as i64;

    // ── Rule pointers (sideband HashMap) ──────────────────────────
    let rp_entries = graph.rule_pointers.len();
    let rp_value_total: usize = graph.rule_pointers.values().map(|v| v.len()).sum();
    let rp_per_entry = size_of::<NodeId>() + size_of::<SmallVec<[RulePtr; 3]>>();
    let rp_bytes_map = (rp_entries as f64 * rp_per_entry as f64 * 1.5) as i64;
    let rp_bytes_spill: i64 = graph
        .rule_pointers
        .values()
        .filter(|v| v.len() > 3)
        .map(|v| (v.capacity() * size_of::<RulePtr>()) as i64)
        .sum();
    let rule_pointers_bytes = rp_bytes_map + rp_bytes_spill;

    let grand_total = nodes_total_bytes
        + strings_total as i64
        + by_kind_bytes
        + cross_indices_total
        + psm_total
        + composite_bytes
        + rule_pointers_bytes;

    json!({
        "engine": "v2",
        "graph_version": graph.graph_version,
        "rule_pointers_version": graph.rule_pointers_version,
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
            "kinds": graph.kinds.len() as i64,
            "entries": by_kind_entries as i64,
            "total_bytes": by_kind_bytes,
        },
        "cross_indices": {
            "breakdown": cross_indices_breakdown,
            "total_bytes": cross_indices_total,
        },
        "psm": {
            "priorities": psm_priorities as i64,
            "rcl_codes": psm_rcl_codes as i64,
            "buckets": psm_buckets as i64,
            "rule_dim_entries": psm_rule_dim_entries as i64,
            "products_with_rcl_hash": psm_pc_count as i64,
            "inner_hash_entries_total": psm_pc_inner_total as i64,
            "total_bytes": psm_total,
        },
        "composite_metrics": {
            "entries": composite_entries as i64,
            "total_bytes": composite_bytes,
        },
        "rule_pointers": {
            "entries": rp_entries as i64,
            "value_total": rp_value_total as i64,
            "total_bytes": rule_pointers_bytes,
        },
        "grand_total_bytes": grand_total,
    })
}
