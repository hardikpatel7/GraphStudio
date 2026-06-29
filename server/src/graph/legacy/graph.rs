//! Arena-backed hierarchy graph.
//!
//! Nodes are stored in a flat `Vec<Node>`; references between them are
//! `NodeId(u32)` indices. Strings (l0..l5 / brand / article / product_code
//! values) are interned in a side `string_pool`, so each Node carries a
//! cheap `StrId(u32)` instead of a `String`.
//!
//! The graph is the **hierarchy spine only** (l0→…→l5→article→product_code).
//! Cross-edges (brand→article, article→channel, store→sg, store→dc) live
//! in `CrossIndices` as separate `HashMap`s — keeping the spine traversal
//! predicate-free.
//!
//! Concurrency: the live graph is wrapped by callers in `ArcSwapOption`
//! (mirrors `rcl::RuleStore`). Readers clone the Arc; CDC writers build a
//! new graph off-thread and atomically swap.
//!
//! No `petgraph` dependency: 700 K-node hierarchies with fixed
//! parent/children access patterns are smaller and simpler in a custom
//! arena than via petgraph's generic edge list.

use smallvec::SmallVec;
use std::collections::HashMap;
use std::sync::Arc;

/// Index into `ArticleGraph.nodes`. `u32` is plenty (we expect ~700 K nodes).
/// `Ord` is derived so callers can build `BTreeSet<NodeId>` for stable
/// candidate-set intersections (cross-filter resolver, entity-list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    /// Sentinel for "no parent" — the root node uses this.
    pub const NONE: NodeId = NodeId(u32::MAX);

    pub fn is_none(self) -> bool {
        self.0 == u32::MAX
    }
}

/// Index into `ArticleGraph.string_pool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StrId(pub u32);

/// Hierarchy node kinds. The spine ordering matches the PG hierarchy
/// (l0 is the broadest level; product_code is the leaf).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Root,
    L0,
    L1,
    L2,
    L3,
    L4,
    L5,
    Article,
    ProductCode,
    // Store side (spine for the parallel store hierarchy).
    Channel,
    StoreCode,
}

impl NodeKind {
    /// Stable index for `by_kind` array access.
    pub fn idx(self) -> usize {
        match self {
            NodeKind::Root => 0,
            NodeKind::L0 => 1,
            NodeKind::L1 => 2,
            NodeKind::L2 => 3,
            NodeKind::L3 => 4,
            NodeKind::L4 => 5,
            NodeKind::L5 => 6,
            NodeKind::Article => 7,
            NodeKind::ProductCode => 8,
            NodeKind::Channel => 9,
            NodeKind::StoreCode => 10,
        }
    }

    pub const COUNT: usize = 11;
}

/// Metric slots attached to every node. Bottom-up rollup sums these
/// across children. Order is fixed; index via [`MetricKind`].
pub const METRIC_COUNT: usize = 8;

#[derive(Debug, Clone, Copy)]
pub enum MetricKind {
    Oh = 0,
    Oo = 1,
    It = 2,
    ReserveQuantity = 3,
    AllocatedUnits = 4,
    LwUnits = 5,
    LwRevenue = 6,
    LwMargin = 7,
}

impl MetricKind {
    pub fn idx(self) -> usize {
        self as usize
    }
}

/// Which RCL flavor a `RulePtr` resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    DcPolicy,
    Constraints,
    Psm,
}

/// Pre-bound rule pointer attached to an article node. The actual rule
/// payload (DcPolicy / ConstraintRow[] / PSM eligibility) lives in the
/// `rcl::RuleSet` keyed by `(rcl_code, rule_code)` — this is just the
/// pointer. Resolved once per (graph version, RuleSet version).
#[derive(Debug, Clone)]
pub struct RulePtr {
    pub kind: RuleKind,
    pub rcl_code: StrId,
    pub rule_code: StrId,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub name: StrId,
    pub parent: NodeId,
    pub children: SmallVec<[NodeId; 4]>,
    pub metrics: [f64; METRIC_COUNT],
    /// Per-article RCL bindings. Empty for non-article nodes. Bound by
    /// `selector_trie::bind_rules`. Length is small (≤ 3 — one per
    /// `RuleKind`).
    pub rule_pointers: SmallVec<[RulePtr; 3]>,
}

impl Node {
    fn new(kind: NodeKind, name: StrId, parent: NodeId) -> Self {
        Self {
            kind,
            name,
            parent,
            children: SmallVec::new(),
            metrics: [0.0; METRIC_COUNT],
            rule_pointers: SmallVec::new(),
        }
    }
}

/// Per-kind index from `StrId` (the node's name) to its `NodeId`.
/// Avoids scanning the arena to find "the L1 node named '3510'".
type ByKind = [HashMap<StrId, NodeId>; NodeKind::COUNT];

/// Cross-cutting indices that don't fit the parent→children spine. Each
/// is a plain `HashMap<NodeId, …>`, keyed by the source-side node.
#[derive(Debug, Default)]
pub struct CrossIndices {
    /// brand StrId → article NodeIds carrying that brand. Brand is not
    /// itself part of the hierarchy spine (it cross-cuts l3..l5), so we
    /// don't promote it to a Node — just index by brand string.
    /// Used by lookups that pivot on brand (e.g. "all articles for
    /// brand X"). The inverse direction lives in [`Self::article_to_brand`].
    pub brand_to_articles: HashMap<StrId, Vec<NodeId>>,
    /// article NodeId → brand StrId. Inverse of `brand_to_articles`,
    /// populated alongside it. Lets per-article projections do an
    /// O(1) brand lookup instead of scanning the brand→articles map.
    /// Without this, projecting 48 K articles cost ~6 s of
    /// `Vec::contains` work; with this, sub-second.
    pub article_to_brand: HashMap<NodeId, StrId>,
    /// article NodeId → channel StrId (each article has exactly one
    /// channel per `ph_master.channel`).
    pub article_to_channel: HashMap<NodeId, StrId>,
    /// product_code StrId → DCs the product is mapped to (active mappings
    /// from `product_mapping_product_dc`). Strings are interned dc_codes.
    pub product_code_to_dcs: HashMap<StrId, SmallVec<[StrId; 4]>>,
    /// store_code StrId → DCs (active mappings from `product_mapping_store_dc`).
    pub store_code_to_dcs: HashMap<StrId, SmallVec<[StrId; 4]>>,
    /// store_code StrId → store_groups it belongs to (`store_groups_mapping`).
    pub store_code_to_sgs: HashMap<StrId, SmallVec<[StrId; 4]>>,
}

/// In-memory graph snapshot. Built fresh by `build::build_graph`; readers
/// hold an `Arc<ArticleGraph>` and traverse without locking.
#[derive(Debug)]
pub struct ArticleGraph {
    pub nodes: Vec<Node>,
    pub string_pool: Vec<Arc<str>>,
    /// Reverse string-pool index used during build to dedupe interning.
    /// Cleared after build completes (set to empty `HashMap`) to save
    /// memory — readers never need to look up a string by value.
    string_index: HashMap<Arc<str>, StrId>,
    pub by_kind: ByKind,
    pub cross_indices: CrossIndices,
    pub root: NodeId,
    /// Bound to the `RuleSet.version` used when binding `rule_pointers`.
    /// Mismatch with the live RuleSet means the bindings are stale.
    pub rule_pointers_version: u64,
    /// Monotonic graph build version — bumped on every full rebuild.
    pub graph_version: u64,
    /// PSM resolver (RCL module 101). Populated by `build_graph` from
    /// the `raw_rcl_psm_*` + `raw_paf_rcl_hash` extracts when present;
    /// otherwise empty (`is_ready()` reports false). The gRPC
    /// `ResolveRcl` path consults `psm.explain(product_code)` to
    /// surface the matched (rcl_code, rule_code).
    pub psm: crate::graph::legacy::psm_resolver::PsmResolver,
}

impl ArticleGraph {
    /// New empty graph with a single Root node.
    pub fn new(graph_version: u64) -> Self {
        let mut g = Self {
            nodes: Vec::new(),
            string_pool: Vec::new(),
            string_index: HashMap::new(),
            by_kind: Default::default(),
            cross_indices: CrossIndices::default(),
            root: NodeId::NONE,
            rule_pointers_version: 0,
            graph_version,
            psm: Default::default(),
        };
        let root_name = g.intern("");
        g.root = g.insert_node(NodeKind::Root, root_name, NodeId::NONE);
        g
    }

    /// Intern a string into `string_pool`. Returns the existing `StrId`
    /// if the string is already present — empty strings always map to
    /// `StrId(0)` (special-cased so callers can use it as a "missing"
    /// marker without an Option).
    pub fn intern(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.string_index.get(s) {
            return id;
        }
        let arc: Arc<str> = Arc::from(s);
        let id = StrId(self.string_pool.len() as u32);
        self.string_pool.push(arc.clone());
        self.string_index.insert(arc, id);
        id
    }

    /// Look up a previously-interned string. Useful for traversal where
    /// we have a `StrId` and want the printable name.
    pub fn get_str(&self, id: StrId) -> &str {
        self.string_pool
            .get(id.0 as usize)
            .map(|a| a.as_ref())
            .unwrap_or("")
    }

    /// Insert a new node under `parent`, register it in the per-kind
    /// index, and append it to the parent's children list. Returns the
    /// new `NodeId`.
    pub fn insert_node(&mut self, kind: NodeKind, name: StrId, parent: NodeId) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node::new(kind, name, parent));
        self.by_kind[kind.idx()].insert(name, id);
        if !parent.is_none() {
            self.nodes[parent.0 as usize].children.push(id);
        }
        id
    }

    /// Find an existing node of `kind` named `name`, or insert it under
    /// `parent`. The common case during build: walking l0..l5 chains
    /// where many products share the same prefix.
    pub fn upsert_node(&mut self, kind: NodeKind, name: StrId, parent: NodeId) -> NodeId {
        if let Some(&id) = self.by_kind[kind.idx()].get(&name) {
            return id;
        }
        self.insert_node(kind, name, parent)
    }

    /// Get a node by id.
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.0 as usize]
    }

    /// Look up a node by (kind, name). Returns `None` if absent.
    pub fn find(&self, kind: NodeKind, name: StrId) -> Option<NodeId> {
        self.by_kind[kind.idx()].get(&name).copied()
    }

    /// Walk from `id` up to root, yielding each ancestor (excluding root).
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = id;
        std::iter::from_fn(move || {
            if cur.is_none() {
                return None;
            }
            let n = self.node(cur);
            let parent = n.parent;
            let out = cur;
            cur = parent;
            if out == self.root {
                None
            } else {
                Some(out)
            }
        })
    }

    /// Number of nodes (including the synthetic Root).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of nodes of a given kind.
    pub fn count_kind(&self, kind: NodeKind) -> usize {
        self.by_kind[kind.idx()].len()
    }

    /// Drop the build-time `string_index`. Call once after all
    /// interning is done — readers never look up by value, so this
    /// reclaims the memory.
    pub fn finalize_strings(&mut self) {
        self.string_index = HashMap::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_basic_insert_and_walk() {
        let mut g = ArticleGraph::new(1);
        let l0 = g.intern("30-bls");
        let l1 = g.intern("3510-LADIES FOOTWEAR");
        let l0_node = g.upsert_node(NodeKind::L0, l0, g.root);
        let l1_node = g.upsert_node(NodeKind::L1, l1, l0_node);

        assert_eq!(g.node(l0_node).parent, g.root);
        assert_eq!(g.node(l1_node).parent, l0_node);
        assert_eq!(g.node(l0_node).children.as_slice(), &[l1_node]);
        assert_eq!(g.count_kind(NodeKind::L0), 1);
        assert_eq!(g.count_kind(NodeKind::L1), 1);

        // upsert is idempotent
        let l0_again = g.upsert_node(NodeKind::L0, l0, g.root);
        assert_eq!(l0_again, l0_node);
        assert_eq!(g.count_kind(NodeKind::L0), 1);
    }

    #[test]
    fn intern_dedupes_strings() {
        let mut g = ArticleGraph::new(1);
        let a = g.intern("foo");
        let b = g.intern("foo");
        let c = g.intern("bar");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(g.get_str(a), "foo");
        assert_eq!(g.get_str(c), "bar");
    }

    #[test]
    fn ancestors_walks_to_root() {
        let mut g = ArticleGraph::new(1);
        let l0 = g.intern("30-bls");
        let l1 = g.intern("3510-LADIES FOOTWEAR");
        let l0_node = g.upsert_node(NodeKind::L0, l0, g.root);
        let l1_node = g.upsert_node(NodeKind::L1, l1, l0_node);
        let chain: Vec<NodeId> = g.ancestors(l1_node).collect();
        assert_eq!(chain, vec![l1_node, l0_node]);
    }
}
