//! Generic post-order rollup over the hierarchy spine.
//!
//! Reuses the iterative two-stack walk from
//! `graph::legacy::rollup::post_order_rollup` (which is sub-second on
//! ~715 K-node bealls), generalized to drive an arbitrary per-metric
//! operator instead of the hard-coded `[f64; 8]` array sum.
//!
//! Same operator applies at attach time and rollup time
//! (Decision 29) — that's `merge`, called both by `build` when source
//! rows fold into a node and by the post-order walk when children
//! fold into a parent. Operators must therefore be associative.
//!
//! Composite-attach metrics (the cube-rollup case, Decision 30) are
//! NOT handled here. Their cells live in `Graph::composite_metrics`,
//! not on `Node`, and roll up per-axis independently — landing in a
//! Phase 3 sub-module so this file stays focused on the common case.

use smallvec::SmallVec;

use super::graph::{CrossEdgeId, Graph, MetricValue, NodeId};
use super::spec::Rollup;

/// Combine `src` into `dst` using `op`. Identity-element initialization
/// (done by `Graph::insert_node`) means `merge(op, dst, identity) ==
/// dst` for every operator — the engine never special-cases "first
/// child" vs "subsequent child".
///
/// Type mismatches (e.g. `Sum` over `Set` cells) are silently no-op:
/// `validate()` should have caught them at definition time, and
/// crashing in the middle of a build over user data is the worst time
/// to fail. The log line from `build` already records which operators
/// fired so a mismatch is debuggable from telemetry.
pub fn merge(op: Rollup, dst: &mut MetricValue, src: &MetricValue) {
    use MetricValue::*;
    match (op, dst, src) {
        // Scalar pairs
        (Rollup::Sum, Scalar(a), Scalar(b)) | (Rollup::Count, Scalar(a), Scalar(b)) => {
            *a += *b;
        }
        // Avg is approximated as "sum" in Phase 2 — true mean requires
        // a sideband count tracker. Decision 13 marks Avg as
        // nice-to-have; we keep the operator dispatch in place so
        // adding the count tracker is a localized change later.
        (Rollup::Avg, Scalar(a), Scalar(b)) => {
            *a += *b;
        }
        (Rollup::Min, Scalar(a), Scalar(b)) => {
            *a = a.min(*b);
        }
        (Rollup::Max, Scalar(a), Scalar(b)) => {
            *a = a.max(*b);
        }
        // Collection pairs — `set`/`count_distinct` share Set storage
        // (distinctness comes for free; the engine surfaces .len() to
        // count_distinct callers at read time).
        (Rollup::Set | Rollup::CountDistinct, Set(a), Set(b)) => {
            for &id in b.iter() {
                a.insert(id);
            }
        }
        (Rollup::List, List(a), List(b)) => {
            a.extend(b.iter().copied());
        }
        // Boolean pairs
        (Rollup::Any, Bool(a), Bool(b)) => {
            *a = *a || *b;
        }
        (Rollup::All, Bool(a), Bool(b)) => {
            *a = *a && *b;
        }
        // Type mismatch (e.g. Sum over Set cells). Silently noop —
        // validate should have prevented this.
        _ => {}
    }
}

/// Post-order DFS from root; each non-leaf node merges every child's
/// metric slot into its own via the slot's declared operator. In-place
/// on `graph.nodes`.
///
/// Idempotency: not idempotent — calling twice double-counts on inner
/// nodes (just like the V1 path). `build_graph` invokes this once
/// after attaching leaf metrics.
pub fn post_order_rollup(graph: &mut Graph) {
    if graph.root.is_none() {
        return;
    }

    // Two-stack iterative post-order. `order` ends up as pre-order;
    // iterating it in reverse gives us children-before-parents. We
    // avoid stack overflow on deep arenas this way (the bealls graph
    // has ~715 K nodes, well past what the kernel stack handles).
    let mut order: Vec<NodeId> = Vec::with_capacity(graph.node_count());
    let mut stack: Vec<NodeId> = vec![graph.root];
    while let Some(id) = stack.pop() {
        order.push(id);
        for &child in &graph.node(id).children {
            stack.push(child);
        }
    }

    // Resolve operator-per-slot once. Slot index in `Node.metrics`
    // corresponds to position in `primary_metric_ids`, by construction
    // in `Graph::insert_node`.
    let primary_ids = graph.metrics.primary_metric_ids();
    let ops: Vec<Rollup> = primary_ids
        .iter()
        .map(|m| graph.metrics.get(*m).rollup)
        .collect();
    if ops.is_empty() {
        return;
    }

    for &id in order.iter().rev() {
        // Snapshot children ids before we mutate any node — the
        // borrow checker won't let us hold &graph.nodes[id].children
        // and &mut graph.nodes[parent] at the same time.
        let children: SmallVec<[NodeId; 4]> = graph.node(id).children.clone();
        for child in children {
            // Cloning the child's metric box is the simplest dodge for
            // the two-index split-borrow problem. The cost is ~64 B/node
            // (8 scalars) for a primary-only graph — ≈45 MB of churn on
            // the bealls dataset, single-pass, well below the rollup's
            // sub-second budget.
            let child_metrics = graph.node(child).metrics.clone();
            let parent = graph.node_mut(id);
            for (i, op) in ops.iter().enumerate() {
                merge(*op, &mut parent.metrics[i], &child_metrics[i]);
            }
        }
    }
}

/// Cross-hierarchy rollup: push each metric across registered
/// cross-edges from the side that natively owns it to the side that
/// doesn't.
///
/// **Why this exists.** `post_order_rollup` only walks parent-child
/// edges within one hierarchy spine. A metric attached at `article`
/// reaches every product-hierarchy ancestor (ph_code, l5, …, l0) but
/// stays at sum-identity on nodes in OTHER hierarchies (brand,
/// store_code, dc_code, …) even when a `[[sources]]` bridge connects
/// them. That's the "top 10 brands by revenue" question coming back as
/// all zeros — brand is a different hierarchy from article.
///
/// **What this does.** For each registered cross-edge `(kind_a,
/// kind_b)`, walk every primary metric:
///
/// - If `meta.attach_kind == kind_a`, push slot values from `kind_a`
///   nodes to their `kind_b` neighbors (via the forward index).
/// - If `meta.attach_kind == kind_b`, push from `kind_b` to `kind_a`
///   (via the reverse index).
/// - Otherwise the metric isn't natively attached at either endpoint
///   of this bridge — skip it. The metric is *transitively* reachable
///   via spine ancestors of one endpoint in principle, but the bridge
///   doesn't carry those node identities, so a meaningful push would
///   require a multi-hop walk that's out of scope here.
///
/// **Why this is safe to run after spine rollup.** The merge uses the
/// metric's declared operator. Sum just adds; the destination kind
/// starts at sum-identity (0) for the slots we're touching (it never
/// natively saw an attach for them), so we're not double-counting
/// anything that spine rollup already accumulated. Min/max/etc.
/// behave identically — taking min across connected nodes is the
/// natural cross-edge semantic.
///
/// **What happens after.** Some downstream rollups would benefit from
/// re-walking the spine on the now-populated destination side (e.g.
/// brand's parent kinds, if any). The bealls spec has single-level
/// brand/store_group hierarchies so no further propagation is needed
/// today; when a multi-level cross-edge-target hierarchy ships, follow
/// this with a second `post_order_rollup` (or a per-hierarchy variant)
/// and the values will thread up correctly.
pub fn cross_edge_rollup(graph: &mut Graph) {
    let primary_ids = graph.metrics.primary_metric_ids();
    if primary_ids.is_empty() {
        return;
    }
    // Per-slot: (rollup operator, push direction).
    // Direction encodes which side of the edge to read from for THIS
    // metric — derived from the metric's attach_kind. `None` means
    // skip this slot for that edge.
    let ops: Vec<Rollup> = primary_ids
        .iter()
        .map(|m| graph.metrics.get(*m).rollup)
        .collect();
    let attach_kinds: Vec<Option<super::graph::KindId>> = primary_ids
        .iter()
        .map(|m| graph.metrics.get(*m).attach_kind)
        .collect();

    let edge_count = graph.cross_edges.metas.len();
    for i in 0..edge_count {
        let eid = CrossEdgeId(i as u32);
        let meta = graph.cross_edges.metas[i].clone();
        // Collect the (src, dst) node pairs per slot up front so we
        // can mutate `graph.nodes` without borrowing the cross-edge
        // index at the same time.
        for (slot, op) in ops.iter().enumerate() {
            let attach = match attach_kinds[slot] {
                Some(k) => k,
                None => continue,
            };
            let (sources, _direction): (Vec<(NodeId, SmallVec<[NodeId; 4]>)>, &'static str) =
                if attach == meta.kind_a {
                    let idx = graph.cross_edges.get(eid);
                    (
                        idx.forward
                            .iter()
                            .map(|(k, v)| (*k, v.clone()))
                            .collect(),
                        "a->b",
                    )
                } else if attach == meta.kind_b {
                    let idx = graph.cross_edges.get(eid);
                    (
                        idx.reverse
                            .iter()
                            .map(|(k, v)| (*k, v.clone()))
                            .collect(),
                        "b->a",
                    )
                } else {
                    continue;
                };
            for (src_node, dsts) in &sources {
                let src_value = graph.node(*src_node).metrics[slot].clone();
                for dst in dsts {
                    let dst_node = graph.node_mut(*dst);
                    merge(*op, &mut dst_node.metrics[slot], &src_value);
                }
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::graph::{
        CrossEdgeRegistry, Graph, KindRegistry, MetricMeta, MetricRegistry, MetricValue,
    };

    fn build_test_graph(rollup: Rollup) -> Graph {
        let mut kinds = KindRegistry::default();
        kinds.register("__root__".to_string(), "".to_string());
        kinds.register("l0".to_string(), "product".to_string());
        kinds.register("article".to_string(), "product".to_string());

        let mut metrics = MetricRegistry::default();
        metrics.register(MetricMeta {
            name: "v".to_string(),
            source_alias: "src".to_string(),
            column: "v".to_string(),
            rollup,
            is_composite: false,
            attach_kind: None,
        });

        Graph::new_with_registries(kinds, metrics, CrossEdgeRegistry::default(), 1)
    }

    /// 3-level fixture: root → l0 → 2 articles. Returns the built graph
    /// plus the l0 node id so callers can read the rolled-up value
    /// without re-resolving from the string pool.
    fn fixture_with_two_articles(
        rollup: Rollup,
        a_val: MetricValue,
        b_val: MetricValue,
    ) -> (Graph, NodeId) {
        let mut g = build_test_graph(rollup);
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let art_kind = g.kinds.id_of("article").unwrap();

        let l0_name = g.intern("L0_A");
        let l0_id = g.upsert_node(l0_kind, l0_name, g.root);

        let a1 = g.intern("a1");
        let a2 = g.intern("a2");
        let a1_id = g.upsert_node(art_kind, a1, l0_id);
        let a2_id = g.upsert_node(art_kind, a2, l0_id);

        g.node_mut(a1_id).metrics[0] = a_val;
        g.node_mut(a2_id).metrics[0] = b_val;

        post_order_rollup(&mut g);
        (g, l0_id)
    }

    #[test]
    fn sum_rolls_up() {
        let (g, l0_id) = fixture_with_two_articles(
            Rollup::Sum,
            MetricValue::Scalar(10.0),
            MetricValue::Scalar(25.0),
        );
        assert!(matches!(g.node(l0_id).metrics[0], MetricValue::Scalar(v) if (v - 35.0).abs() < 1e-9));
        assert!(matches!(g.node(g.root).metrics[0], MetricValue::Scalar(v) if (v - 35.0).abs() < 1e-9));
    }

    #[test]
    fn min_rolls_up() {
        let (g, l0_id) = fixture_with_two_articles(
            Rollup::Min,
            MetricValue::Scalar(10.0),
            MetricValue::Scalar(25.0),
        );
        assert!(matches!(g.node(l0_id).metrics[0], MetricValue::Scalar(v) if (v - 10.0).abs() < 1e-9));
    }

    #[test]
    fn max_rolls_up() {
        let (g, l0_id) = fixture_with_two_articles(
            Rollup::Max,
            MetricValue::Scalar(10.0),
            MetricValue::Scalar(25.0),
        );
        assert!(matches!(g.node(l0_id).metrics[0], MetricValue::Scalar(v) if (v - 25.0).abs() < 1e-9));
    }

    #[test]
    fn set_rollup_deduplicates() {
        let mut s1 = indexmap::IndexSet::new();
        s1.insert(crate::graph::graph::StrId(1));
        let mut s2 = indexmap::IndexSet::new();
        s2.insert(crate::graph::graph::StrId(1));
        s2.insert(crate::graph::graph::StrId(2));
        let (g, _l0_id) = fixture_with_two_articles(
            Rollup::Set,
            MetricValue::Set(s1),
            MetricValue::Set(s2),
        );
        match &g.node(g.root).metrics[0] {
            MetricValue::Set(set) => assert_eq!(set.len(), 2),
            other => panic!("expected Set slot, got {other:?}"),
        }
    }

    #[test]
    fn empty_metrics_no_op() {
        // A graph with zero metrics shouldn't panic on rollup.
        let mut kinds = KindRegistry::default();
        kinds.register("__root__".to_string(), "".to_string());
        kinds.register("l0".to_string(), "product".to_string());

        let metrics = MetricRegistry::default();
        let mut g = Graph::new_with_registries(kinds, metrics, CrossEdgeRegistry::default(), 1);
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let n = g.intern("x");
        g.upsert_node(l0_kind, n, g.root);
        post_order_rollup(&mut g);
    }
}
