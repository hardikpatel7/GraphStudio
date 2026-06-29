//! Bottom-up metric rollup over the hierarchy spine.
//!
//! Post-order DFS from the root: each non-leaf node sums the per-metric
//! arrays of its children. Leaves keep whatever value `build` already
//! attached (article-level OH/OO/IT, lw_units/revenue/margin).
//!
//! Single pass, sub-second for the full Bealls dataset (~715 K nodes).

use crate::graph::legacy::graph::{ArticleGraph, NodeId, METRIC_COUNT};

/// Roll metrics up from leaves to root in-place. Children's values are
/// added into their parent; root accumulates the global total.
///
/// Idempotency: not idempotent — calling twice doubles the totals on
/// inner nodes. Build code calls this exactly once after attaching
/// leaf metrics.
pub fn post_order_rollup(graph: &mut ArticleGraph) {
    if graph.root.is_none() {
        return;
    }
    // Iterative post-order: avoids stack overflow on deep arenas.
    // Two-stack approach: `pending` are nodes whose children we still
    // need to visit; `order` is the post-order traversal we'll iterate
    // in reverse to do the actual summing.
    let mut order: Vec<NodeId> = Vec::with_capacity(graph.node_count());
    let mut stack: Vec<NodeId> = vec![graph.root];
    while let Some(id) = stack.pop() {
        order.push(id);
        for &child in &graph.node(id).children {
            stack.push(child);
        }
    }
    // `order` is now a pre-order; iterating in reverse gives us
    // post-order (children before parents).
    for &id in order.iter().rev() {
        // Sum each child's metrics into the current node. We can't
        // borrow `node_mut(id)` and `node(child)` simultaneously, so
        // pull children IDs first and then accumulate.
        let children: smallvec::SmallVec<[NodeId; 4]> =
            graph.node(id).children.clone();
        for child in children {
            let child_metrics = graph.node(child).metrics;
            let parent = graph.node_mut(id);
            for i in 0..METRIC_COUNT {
                parent.metrics[i] += child_metrics[i];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::legacy::graph::{ArticleGraph, MetricKind, NodeKind};

    /// 3-level fixture: root → l0 → 2 articles. Verify that l0.OH equals
    /// the sum of the two leaf articles' OH after rollup.
    #[test]
    fn rollup_sums_leaves_into_ancestors() {
        let mut g = ArticleGraph::new(1);
        let l0_name = g.intern("30-bls");
        let l0 = g.upsert_node(NodeKind::L0, l0_name, g.root);

        let a1 = g.intern("art-1");
        let a2 = g.intern("art-2");
        let a1_node = g.upsert_node(NodeKind::Article, a1, l0);
        let a2_node = g.upsert_node(NodeKind::Article, a2, l0);

        g.node_mut(a1_node).metrics[MetricKind::Oh.idx()] = 10.0;
        g.node_mut(a2_node).metrics[MetricKind::Oh.idx()] = 25.0;
        g.node_mut(a1_node).metrics[MetricKind::LwRevenue.idx()] = 100.0;
        g.node_mut(a2_node).metrics[MetricKind::LwRevenue.idx()] = 250.0;

        post_order_rollup(&mut g);

        assert_eq!(g.node(l0).metrics[MetricKind::Oh.idx()], 35.0);
        assert_eq!(g.node(l0).metrics[MetricKind::LwRevenue.idx()], 350.0);
        // Root sums everything → same totals (single l0 child).
        assert_eq!(g.node(g.root).metrics[MetricKind::Oh.idx()], 35.0);
        assert_eq!(g.node(g.root).metrics[MetricKind::LwRevenue.idx()], 350.0);
        // Leaves unchanged.
        assert_eq!(g.node(a1_node).metrics[MetricKind::Oh.idx()], 10.0);
        assert_eq!(g.node(a2_node).metrics[MetricKind::Oh.idx()], 25.0);
    }

    #[test]
    fn rollup_multilevel() {
        let mut g = ArticleGraph::new(1);
        let l0n = g.intern("30");
        let l0 = g.upsert_node(NodeKind::L0, l0n, g.root);
        let l1n = g.intern("3510");
        let l1 = g.upsert_node(NodeKind::L1, l1n, l0);
        let l2n_a = g.intern("3548");
        let l2_a = g.upsert_node(NodeKind::L2, l2n_a, l1);
        let l2n_b = g.intern("3549");
        let l2_b = g.upsert_node(NodeKind::L2, l2n_b, l1);

        let art_a = g.intern("art-a");
        let art_b = g.intern("art-b");
        let art_c = g.intern("art-c");
        let art_a_id = g.upsert_node(NodeKind::Article, art_a, l2_a);
        let art_b_id = g.upsert_node(NodeKind::Article, art_b, l2_a);
        let art_c_id = g.upsert_node(NodeKind::Article, art_c, l2_b);

        g.node_mut(art_a_id).metrics[MetricKind::Oh.idx()] = 1.0;
        g.node_mut(art_b_id).metrics[MetricKind::Oh.idx()] = 2.0;
        g.node_mut(art_c_id).metrics[MetricKind::Oh.idx()] = 4.0;

        post_order_rollup(&mut g);

        assert_eq!(g.node(l2_a).metrics[MetricKind::Oh.idx()], 3.0);
        assert_eq!(g.node(l2_b).metrics[MetricKind::Oh.idx()], 4.0);
        assert_eq!(g.node(l1).metrics[MetricKind::Oh.idx()], 7.0);
        assert_eq!(g.node(l0).metrics[MetricKind::Oh.idx()], 7.0);
    }
}
