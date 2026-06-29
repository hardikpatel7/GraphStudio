//! Generic, metadata-driven `Graph`.
//!
//! Mirrors `graph::legacy::ArticleGraph` in shape — arena-backed nodes,
//! interned strings, per-kind `(StrId → NodeId)` lookup, an
//! `ArcSwap`-friendly snapshot lifecycle — but with everything that was
//! a Rust enum in the original (`NodeKind`, `MetricKind`, `CrossEdges`'
//! named fields) replaced by runtime registries populated from the
//! `GraphSpec`.
//!
//! The runtime registries are the *only* difference. Traversal,
//! interning, and the post-order rollup walk are kept identical so
//! Phase 3's parity test has a chance.
//!
//! ## ID conventions
//!
//! - `KindId(0)` is reserved for the synthetic root. The root carries no
//!   hierarchy name and no level name; it exists so every spine is a
//!   tree rather than a forest.
//! - `StrId(0)` is the empty string — useful as a "missing" sentinel
//!   that callers can compare against without an `Option<StrId>` round-trip.
//! - `NodeId::NONE` (`u32::MAX`) is the parent pointer for the root.

use indexmap::IndexSet;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::sync::Arc;

use super::spec::Rollup;

// ──────────────────────────────────────────────────────────────────────────
// Ids
// ──────────────────────────────────────────────────────────────────────────

/// Index into `Graph.nodes`. `u32` is plenty (the bealls hierarchy tops
/// out around 715 K nodes, and we never expect a tenant to exceed that
/// by orders of magnitude — multi-tenant separation comes from running
/// multiple `Graph` snapshots, not one huge one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const NONE: NodeId = NodeId(u32::MAX);
    pub fn is_none(self) -> bool { self.0 == u32::MAX }
}

/// Index into `Graph.string_pool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StrId(pub u32);

/// Runtime identifier for a kind (level) declared in the spec. Resolved
/// once at the start of `build_graph`; `0` is reserved for the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KindId(pub u32);

impl KindId {
    pub const ROOT: KindId = KindId(0);
}

/// Runtime identifier for a metric. Same model as `KindId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MetricId(pub u32);

/// Identifier for a cross-edge type — e.g. `(brand_kind → article_kind)`.
/// Cross-edges are declared implicitly by bridge sources (Decision 36).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CrossEdgeId(pub u32);

// ──────────────────────────────────────────────────────────────────────────
// Metric values
// ──────────────────────────────────────────────────────────────────────────

/// Per-node, per-metric storage cell. Variant is fixed at registry-build
/// time by the metric's `Rollup` operator — `Set` rollup ⇒ `Set` cell,
/// `Sum` ⇒ `Scalar`, etc. The engine never mutates the variant; it only
/// updates the inner data via `attach_value` / `merge_into_parent`.
///
/// `IndexSet` (order-preserving) over `HashSet` so the contents of a
/// `Set` metric serialize deterministically in API responses.
#[derive(Debug, Clone)]
pub enum MetricValue {
    /// `sum`, `min`, `max`, `count`. Initial value depends on operator
    /// (see `Rollup::identity`).
    Scalar(f64),
    /// `set`, `count_distinct`. Stores interned `StrId`s for compact
    /// merging; the engine resolves to strings only at read time.
    Set(IndexSet<StrId>),
    /// `list`. Preserves duplicates. `SmallVec` keeps the common small-
    /// list case allocation-free.
    List(SmallVec<[StrId; 4]>),
    /// `any`, `all`. Init is `false` for `any`, `true` for `all`.
    Bool(bool),
}

impl MetricValue {
    /// Identity element for an operator — the value that, when combined
    /// with any other via the operator, yields the other value
    /// unchanged. This is what every node's metric slot is initialized
    /// to before attach.
    pub fn identity_for(rollup: Rollup) -> MetricValue {
        match rollup {
            Rollup::Sum | Rollup::Count | Rollup::Avg => MetricValue::Scalar(0.0),
            Rollup::Min => MetricValue::Scalar(f64::INFINITY),
            Rollup::Max => MetricValue::Scalar(f64::NEG_INFINITY),
            Rollup::Set | Rollup::CountDistinct => MetricValue::Set(IndexSet::new()),
            Rollup::List => MetricValue::List(SmallVec::new()),
            Rollup::Any => MetricValue::Bool(false),
            Rollup::All => MetricValue::Bool(true),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Registries
// ──────────────────────────────────────────────────────────────────────────

/// Runtime kind metadata. The name + hierarchy fields make logging /
/// error messages readable; the engine itself only compares `KindId`s.
#[derive(Debug, Clone)]
pub struct KindMeta {
    /// Level id from the spec (e.g. `"l0"`, `"article"`), or `"__root__"`
    /// for the synthetic root.
    pub name: String,
    /// Owning hierarchy name (`"product"`, `"store"`, …). Empty for root.
    pub hierarchy: String,
}

/// Runtime metric metadata.
#[derive(Debug, Clone)]
pub struct MetricMeta {
    /// Metric id from the spec (the TOML key in `[metrics.<src>]`).
    pub name: String,
    /// Source alias the metric reads from.
    pub source_alias: String,
    /// Column to read on the source. Defaults to `name` when the spec
    /// omits an explicit `column = "..."`.
    pub column: String,
    /// Aggregation operator. Determines slot type via
    /// `MetricValue::identity_for`.
    pub rollup: Rollup,
    /// Composite-attach metrics live in a sideband `composite_metrics`
    /// store rather than on `Node.metrics`. Phase 2 stores the flag but
    /// the build code skips composite sources until Phase 3.
    pub is_composite: bool,
    /// Kind the source's row attaches at (== `source.attaches_at` for
    /// single-attach sources). Cross-edge rollup uses this to decide
    /// which side of a bridge naturally owns the metric so it can push
    /// it across. `None` for composite-attach sources, where the
    /// cube-lookup API takes over.
    pub attach_kind: Option<KindId>,
}

/// Append-only kind registry. KindIds are monotonic indices into
/// `kinds`; `name_index` is a build-time reverse lookup. The registry
/// is finalized (= built) once at the start of `build_graph` and
/// never mutated afterwards.
#[derive(Debug, Default, Clone)]
pub struct KindRegistry {
    kinds: Vec<KindMeta>,
    /// Level name → KindId. Level names are globally unique across
    /// hierarchies (enforced by `spec::validate::HIER_LEVEL_NAME_COLLISION`),
    /// so we don't need a (hierarchy, level) compound key.
    name_index: HashMap<String, KindId>,
}

impl KindRegistry {
    /// Register a new kind. Returns its `KindId`. Caller is responsible
    /// for not double-registering (validation guarantees uniqueness).
    pub fn register(&mut self, name: String, hierarchy: String) -> KindId {
        let id = KindId(self.kinds.len() as u32);
        self.name_index.insert(name.clone(), id);
        self.kinds.push(KindMeta { name, hierarchy });
        id
    }

    pub fn get(&self, id: KindId) -> &KindMeta {
        &self.kinds[id.0 as usize]
    }

    pub fn id_of(&self, name: &str) -> Option<KindId> {
        self.name_index.get(name).copied()
    }

    pub fn len(&self) -> usize { self.kinds.len() }
    pub fn iter(&self) -> impl Iterator<Item = (KindId, &KindMeta)> {
        self.kinds.iter().enumerate().map(|(i, k)| (KindId(i as u32), k))
    }
}

/// Metric registry. Same shape as `KindRegistry`; metric names are
/// disambiguated by `source_alias` at the source layer, but the
/// registry itself just uses the metric id (`<src>.<name>`-flattened)
/// as its `name_index` key to allow O(1) lookup from a spec entry.
#[derive(Debug, Default, Clone)]
pub struct MetricRegistry {
    metrics: Vec<MetricMeta>,
    /// `"<source_alias>.<metric_name>"` → MetricId. The two-part key
    /// avoids collisions when two sources publish a metric named "oh".
    name_index: HashMap<String, MetricId>,
}

impl MetricRegistry {
    pub fn register(&mut self, meta: MetricMeta) -> MetricId {
        let id = MetricId(self.metrics.len() as u32);
        let key = format!("{}.{}", meta.source_alias, meta.name);
        self.name_index.insert(key, id);
        self.metrics.push(meta);
        id
    }

    pub fn get(&self, id: MetricId) -> &MetricMeta {
        &self.metrics[id.0 as usize]
    }

    pub fn id_of(&self, source_alias: &str, metric_name: &str) -> Option<MetricId> {
        self.name_index.get(&format!("{source_alias}.{metric_name}")).copied()
    }

    pub fn len(&self) -> usize { self.metrics.len() }
    pub fn iter(&self) -> impl Iterator<Item = (MetricId, &MetricMeta)> {
        self.metrics.iter().enumerate().map(|(i, m)| (MetricId(i as u32), m))
    }

    /// Subset of metrics whose values live on `Node.metrics` (i.e. all
    /// non-composite metrics). The returned slice's *position* matches
    /// the corresponding slot index in `Node.metrics`, since slot index
    /// equals the metric's count among primary metrics in declaration
    /// order.
    pub fn primary_metric_ids(&self) -> Vec<MetricId> {
        self.iter()
            .filter(|(_, m)| !m.is_composite)
            .map(|(id, _)| id)
            .collect()
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Cross-edges
// ──────────────────────────────────────────────────────────────────────────

/// One cross-edge type. The engine uses `kind_a` → `Vec<NodeId>` to
/// answer "which nodes of kind B does this kind-A node link to". Both
/// directions are stored so projections in either direction are O(1).
#[derive(Debug, Default, Clone)]
pub struct CrossIndex {
    /// Forward direction: `kind_a` node → `[kind_b]` nodes.
    pub forward: HashMap<NodeId, SmallVec<[NodeId; 4]>>,
    /// Reverse direction, populated alongside `forward` so reverse
    /// projections (e.g. "which articles carry brand X") don't have to
    /// scan the whole forward map.
    pub reverse: HashMap<NodeId, SmallVec<[NodeId; 4]>>,
}

/// Cross-edge type metadata — the two kinds the edge connects, plus
/// the bridge source that produced it. Logged on build for debuggability.
#[derive(Debug, Clone)]
pub struct CrossEdgeMeta {
    pub kind_a: KindId,
    pub kind_b: KindId,
    pub bridge_source: String,
}

#[derive(Debug, Default, Clone)]
pub struct CrossEdgeRegistry {
    pub metas: Vec<CrossEdgeMeta>,
    pub indices: Vec<CrossIndex>,
}

impl CrossEdgeRegistry {
    pub fn register(&mut self, meta: CrossEdgeMeta) -> CrossEdgeId {
        let id = CrossEdgeId(self.metas.len() as u32);
        self.metas.push(meta);
        self.indices.push(CrossIndex::default());
        id
    }

    pub fn get(&self, id: CrossEdgeId) -> &CrossIndex {
        &self.indices[id.0 as usize]
    }

    pub fn get_mut(&mut self, id: CrossEdgeId) -> &mut CrossIndex {
        &mut self.indices[id.0 as usize]
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Node
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: KindId,
    pub name: StrId,
    pub parent: NodeId,
    pub children: SmallVec<[NodeId; 4]>,
    /// One slot per *primary* metric, in `MetricRegistry::primary_metric_ids`
    /// order. Boxed slice rather than `Vec` because every node carries
    /// the same length — no per-node growth, so the `Vec` cap field
    /// would just be dead weight.
    pub metrics: Box<[MetricValue]>,
}

// ──────────────────────────────────────────────────────────────────────────
// Graph
// ──────────────────────────────────────────────────────────────────────────

/// Composite-metric storage key. Composite-attach metrics (Decision 30)
/// don't fit on a `Node` because their value is per-cell, not per-node;
/// e.g. WOC at `(l4, store)` would explode primary-node storage if the
/// cell were stored there.
///
/// Phase 2 stores the field but build never populates it — composite
/// support lands in Phase 3 with the cube rollup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompositeKey {
    pub metric: MetricId,
    /// Node ids for each axis of the composite, in declaration order
    /// (= `AttachesAt::Composite` order in the spec).
    pub axes: SmallVec<[NodeId; 2]>,
}

#[derive(Debug)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub string_pool: Vec<Arc<str>>,
    /// Build-time reverse lookup; emptied by `finalize_strings()` after
    /// build to reclaim the memory (readers never need value → StrId).
    string_index: HashMap<Arc<str>, StrId>,

    pub kinds: KindRegistry,
    pub metrics: MetricRegistry,
    pub cross_edges: CrossEdgeRegistry,

    /// Per-kind reverse index: `kinds[KindId.0] : { name_str_id → node_id }`.
    /// Sized to `kinds.len()` at build time; never resized after.
    by_kind: Vec<HashMap<StrId, NodeId>>,

    /// Composite-attach metric cells. Empty in Phase 2.
    pub composite_metrics: HashMap<CompositeKey, MetricValue>,

    /// PSM resolver — built from `raw_rcl_psm_*` tables when present.
    /// Default-constructed (empty) when RCL isn't wired or the tenant
    /// has no PSM data; `is_ready()` reports false in that case and
    /// downstream consumers skip RCL-dependent paths.
    pub psm: super::rcl::PsmResolver,

    /// Pre-bound rule pointers per article node. Sideband HashMap
    /// rather than a `Node` field so non-RCL graphs don't pay any
    /// per-node memory cost. Populated by a separate `bind_rules`
    /// step after `build_graph` returns (when an `rcl::RuleSet`
    /// is available); empty until then.
    pub rule_pointers: HashMap<NodeId, SmallVec<[super::rcl::RulePtr; 3]>>,

    /// Tracks the `RuleSet.version` that `rule_pointers` were bound
    /// against. A mismatch with the live RuleSet means the bindings
    /// are stale and need a re-bind. Same semantics as v1.
    pub rule_pointers_version: u64,

    pub root: NodeId,
    pub graph_version: u64,
}

impl Graph {
    /// Allocate a graph with the registries already populated. The
    /// caller wires up `kinds`, `metrics`, and `cross_edges` from the
    /// spec; `Graph::new_with_registries` creates the root node sized
    /// to those registries' metric slot count.
    pub fn new_with_registries(
        kinds: KindRegistry,
        metrics: MetricRegistry,
        cross_edges: CrossEdgeRegistry,
        graph_version: u64,
    ) -> Self {
        // The synthetic root must be in `kinds` already (registered as
        // KindId(0) by `build`'s registry-construction phase).
        debug_assert!(
            !kinds.kinds.is_empty() && kinds.get(KindId::ROOT).name == "__root__",
            "KindRegistry must register __root__ as KindId(0) before Graph::new_with_registries",
        );
        let by_kind = vec![HashMap::new(); kinds.len()];
        let mut g = Self {
            nodes: Vec::new(),
            string_pool: Vec::new(),
            string_index: HashMap::new(),
            kinds,
            metrics,
            cross_edges,
            by_kind,
            composite_metrics: HashMap::new(),
            psm: super::rcl::PsmResolver::default(),
            rule_pointers: HashMap::new(),
            rule_pointers_version: 0,
            root: NodeId::NONE,
            graph_version,
        };
        // Reserve StrId(0) for the empty string so callers can treat it
        // as a "missing" sentinel without an Option round-trip.
        let empty = g.intern("");
        g.root = g.insert_node(KindId::ROOT, empty, NodeId::NONE);
        g
    }

    /// Intern a string into `string_pool`. Returns the existing `StrId`
    /// if the string is already present.
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

    pub fn get_str(&self, id: StrId) -> &str {
        self.string_pool
            .get(id.0 as usize)
            .map(|a| a.as_ref())
            .unwrap_or("")
    }

    /// Allocate a new node — registers it in `by_kind`, appends it to
    /// `parent.children`, and zero-initializes its metric slots from
    /// the registry's primary-metric list.
    pub fn insert_node(&mut self, kind: KindId, name: StrId, parent: NodeId) -> NodeId {
        // Initialize metric slots in primary-metric declaration order
        // with each operator's identity element. The slot count is
        // identical for every node; sizing it on every `insert_node`
        // call is the simplest place to derive it from the registry.
        let primary = self.metrics.primary_metric_ids();
        let slots: Vec<MetricValue> = primary
            .iter()
            .map(|mid| MetricValue::identity_for(self.metrics.get(*mid).rollup))
            .collect();

        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node {
            kind,
            name,
            parent,
            children: SmallVec::new(),
            metrics: slots.into_boxed_slice(),
        });
        self.by_kind[kind.0 as usize].insert(name, id);
        if !parent.is_none() {
            self.nodes[parent.0 as usize].children.push(id);
        }
        id
    }

    /// Find-or-insert. Common during spine-walking when many rows share
    /// the same `(kind, name)` prefix (e.g. all rows under one L0).
    pub fn upsert_node(&mut self, kind: KindId, name: StrId, parent: NodeId) -> NodeId {
        if let Some(&id) = self.by_kind[kind.0 as usize].get(&name) {
            return id;
        }
        self.insert_node(kind, name, parent)
    }

    pub fn node(&self, id: NodeId) -> &Node { &self.nodes[id.0 as usize] }
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node { &mut self.nodes[id.0 as usize] }

    pub fn find(&self, kind: KindId, name: StrId) -> Option<NodeId> {
        self.by_kind[kind.0 as usize].get(&name).copied()
    }

    /// Look up a node by `(kind, string content)`. After
    /// `finalize_strings()` the build-time string_index is dropped, so
    /// callers that arrive with a `&str` (most HTTP handlers, the
    /// traverse module) have to scan `string_pool` for the StrId. O(n)
    /// in the pool size; bealls' ~700 K strings → low single-digit ms
    /// for a single lookup, fine for ad-hoc clicks. If this becomes hot
    /// (many lookups per request), rebuild a permanent reverse map
    /// here rather than scanning.
    pub fn find_by_name(&self, kind: KindId, name: &str) -> Option<NodeId> {
        let str_id = self
            .string_pool
            .iter()
            .position(|s| s.as_ref() == name)
            .map(|i| StrId(i as u32))?;
        self.find(kind, str_id)
    }

    pub fn count_kind(&self, kind: KindId) -> usize {
        self.by_kind[kind.0 as usize].len()
    }

    /// Iterate every `NodeId` of `kind`. Backed by the per-kind
    /// `HashMap<StrId, NodeId>` index, so cost is O(nodes of that kind)
    /// — not O(total nodes). Callers that filter the whole arena by
    /// kind should always go through this; the linear `nodes.iter()`
    /// scan is O(N) and pays for every off-kind node on every call.
    ///
    /// Order is HashMap iteration order — not stable across runs.
    /// Callers that need a deterministic ordering should sort
    /// afterwards (the alternative — keeping an insertion-ordered
    /// `Vec<NodeId>` per kind — costs a duplicate pointer per node).
    pub fn iter_kind(&self, kind: KindId) -> impl Iterator<Item = NodeId> + '_ {
        self.by_kind[kind.0 as usize].values().copied()
    }

    pub fn node_count(&self) -> usize { self.nodes.len() }

    /// Walk from `id` to root, yielding each ancestor up to and
    /// including the root's child. The root itself is excluded — most
    /// callers want the spine of named nodes, not the synthetic root.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = id;
        let root = self.root;
        std::iter::from_fn(move || {
            if cur.is_none() || cur == root {
                return None;
            }
            let out = cur;
            cur = self.node(cur).parent;
            Some(out)
        })
    }

    /// Drop the build-time `string_index`. Call once after all interning
    /// is done. Mirrors `graph::legacy::ArticleGraph::finalize_strings`.
    pub fn finalize_strings(&mut self) {
        self.string_index = HashMap::new();
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal registry shape — `__root__` + two levels under
    /// a single hierarchy + one scalar metric — and verify arena
    /// insert/upsert/find behave like the V1 graph.
    fn fixture() -> Graph {
        let mut kinds = KindRegistry::default();
        kinds.register("__root__".to_string(), "".to_string());
        let _l0 = kinds.register("l0".to_string(), "product".to_string());
        let _l1 = kinds.register("l1".to_string(), "product".to_string());

        let mut metrics = MetricRegistry::default();
        metrics.register(MetricMeta {
            name: "v".to_string(),
            source_alias: "src".to_string(),
            column: "v".to_string(),
            rollup: Rollup::Sum,
            is_composite: false,
            attach_kind: None,
        });

        Graph::new_with_registries(kinds, metrics, CrossEdgeRegistry::default(), 1)
    }

    #[test]
    fn registry_assigns_root_id_zero() {
        let g = fixture();
        assert_eq!(g.kinds.id_of("__root__"), Some(KindId::ROOT));
        assert_eq!(g.node(g.root).kind, KindId::ROOT);
    }

    #[test]
    fn upsert_dedupes_by_kind_and_name() {
        let mut g = fixture();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let n = g.intern("alpha");
        let first = g.upsert_node(l0_kind, n, g.root);
        let second = g.upsert_node(l0_kind, n, g.root);
        assert_eq!(first, second);
        assert_eq!(g.count_kind(l0_kind), 1);
    }

    #[test]
    fn node_metric_slot_sized_to_primary_metric_count() {
        let g = fixture();
        // One scalar metric registered → one slot per node, initialized
        // to Sum's identity (0.0).
        let slots = &g.node(g.root).metrics;
        assert_eq!(slots.len(), 1);
        assert!(matches!(slots[0], MetricValue::Scalar(v) if v == 0.0));
    }

    #[test]
    fn ancestors_excludes_root() {
        let mut g = fixture();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let l1_kind = g.kinds.id_of("l1").unwrap();
        let n0 = g.intern("a");
        let n1 = g.intern("b");
        let l0_id = g.upsert_node(l0_kind, n0, g.root);
        let l1_id = g.upsert_node(l1_kind, n1, l0_id);
        let chain: Vec<NodeId> = g.ancestors(l1_id).collect();
        assert_eq!(chain, vec![l1_id, l0_id]);
    }

    #[test]
    fn intern_zero_is_empty_string() {
        let g = fixture();
        assert_eq!(g.get_str(StrId(0)), "");
    }
}
