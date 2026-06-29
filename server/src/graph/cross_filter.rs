//! Pure cross-filter resolution over a `Graph` snapshot.
//!
//! ## Wire compat with v1
//!
//! The bottom of this file has `From<v1::cross_filter::model::Filter>`
//! and `From<v1::cross_filter::model::Operator>` impls so HTTP handlers
//! can deserialize the existing `FilterPayload` shape and pass it
//! straight through to `apply_filters`. The handler wrapper at
//! `handlers::graphs::cross_filter_handler` exercises this path.
//!
//! Two-phase, mirroring v1's `cross_filter::resolver`:
//!   1. `apply_filters` — narrow target-kind candidates by filter set
//!   2. `project_distinct` — for each requested attribute, collect
//!      sorted distinct values across the candidates
//!
//! The "target kind" is the kind whose nodes form the candidate set —
//! for bealls' article filters that's `article`, but the function is
//! generic so the same path serves "filter stores" or any other kind
//! a future tenant declares.
//!
//! ## Attribute resolution rules (metadata-driven, no hard-coded names)
//!
//! Given an attribute name, the resolver picks the first match:
//!
//! 1. **Self-name** — attribute matches `target_kind`'s name → compare
//!    against `Node.name`.
//! 2. **Ancestor kind** — attribute matches a `KindId` that's any
//!    ancestor of the target on the spine → walk parent chain.
//! 3. **Cross-edge target kind** — attribute matches a `KindId` that's
//!    the *other* endpoint of any registered cross-edge with the
//!    target on the near side → walk cross-edge neighbors.
//! 4. **Unknown** — returns no filter narrowing (matches v1's
//!    silent-passthrough for store_* attributes against an article
//!    target).
//!
//! ## Operator support
//!
//! `In` / `InEq` / `Eq` are membership semantics; everything else is
//! logged + ignored (parity with v1, which has the same "extend when
//! a caller needs it" stance).

use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap, HashSet};

use super::graph::{CrossEdgeId, Graph, KindId, NodeId};

/// Concrete entitlement set. Mirrors v1's `EntitledSet` minus the
/// store_codes field — v2 stores store-side restrictions as a separate
/// NodeId set on the store kind (callers can intersect themselves).
#[derive(Debug, Clone, Default)]
pub struct EntitledSet {
    /// Target-kind NodeIds the caller is allowed to see. `None` =
    /// unrestricted.
    pub allowed: Option<HashSet<NodeId>>,
}

/// One filter criterion. `values` are already string-coerced; the
/// wire layer is responsible for the `Values::List` / `Values::Single`
/// normalization.
#[derive(Debug, Clone)]
pub struct FilterCriterion {
    pub attribute_name: String,
    pub values: Vec<String>,
    pub operator: FilterOperator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOperator {
    In,
    Eq,
    Ne,
    /// Anything outside the membership set (`In`/`Eq`/`InEq`) — parity
    /// with v1's "log and skip" path. Includes range/like/etc.
    Unsupported,
}

impl FilterOperator {
    /// Treat as a membership test? `Eq` is equivalent to `In` on a
    /// single-element set (v1 conflates them too).
    fn is_membership(self) -> bool {
        matches!(self, FilterOperator::In | FilterOperator::Eq)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 1 — apply_filters
// ──────────────────────────────────────────────────────────────────────────

/// AND-compose `filters` against the graph and return the surviving
/// `target_kind` `NodeId`s. Empty filter list → all target-kind nodes
/// (excluding empty-named placeholders that come from the
/// `split` collapse case in build).
///
/// UAM intersection happens last and unconditionally — a filter set
/// that would otherwise pass an article cannot bypass entitlement.
pub fn apply_filters(
    graph: &Graph,
    target_kind: KindId,
    filters: &[FilterCriterion],
    entitled: Option<&EntitledSet>,
) -> BTreeSet<NodeId> {
    // Seed with every target-kind node that has a non-empty name. The
    // empty-name placeholder appears when build's split fallback
    // emits one for a fully-empty source row; we don't want it
    // showing up in cross-filter results.
    let empty_str_id = super::graph::StrId(0);
    let mut candidates: BTreeSet<NodeId> = (0..graph.node_count())
        .map(|i| NodeId(i as u32))
        .filter(|id| {
            let n = graph.node(*id);
            n.kind == target_kind && n.name != empty_str_id
        })
        .collect();

    for f in filters {
        if !f.operator.is_membership() {
            tracing::warn!(
                attribute = f.attribute_name,
                operator = ?f.operator,
                "[graph::cross_filter] unsupported operator — filter skipped"
            );
            continue;
        }
        if f.values.is_empty() {
            continue;
        }
        let needles: HashSet<&str> = f.values.iter().map(String::as_str).collect();
        candidates = narrow(graph, target_kind, &candidates, &f.attribute_name, &needles);
        if candidates.is_empty() {
            return candidates;
        }
    }

    if let Some(ent) = entitled {
        if let Some(allowed) = &ent.allowed {
            candidates.retain(|id| allowed.contains(id));
        }
    }

    candidates
}

/// Apply one filter's narrowing pass. Returns a fresh BTreeSet so the
/// caller's outer loop can swap it in atomically (and so an empty
/// result short-circuits cleanly).
fn narrow(
    graph: &Graph,
    target_kind: KindId,
    candidates: &BTreeSet<NodeId>,
    attribute_name: &str,
    needles: &HashSet<&str>,
) -> BTreeSet<NodeId> {
    let strategy = resolve_attribute(graph, target_kind, attribute_name);
    match strategy {
        AttributeStrategy::SelfName => candidates
            .par_iter()
            .copied()
            .filter(|id| needles.contains(graph.get_str(graph.node(*id).name)))
            .collect(),
        AttributeStrategy::Ancestor(kind) => candidates
            .par_iter()
            .copied()
            .filter(|id| {
                ancestor_name(graph, *id, kind)
                    .map(|s| needles.contains(s))
                    .unwrap_or(false)
            })
            .collect(),
        AttributeStrategy::CrossEdge(eid, _kind, direction) => candidates
            .par_iter()
            .copied()
            .filter(|id| cross_edge_matches(graph, *id, eid, direction, needles))
            .collect(),
        AttributeStrategy::Unknown => {
            // Mirrors v1: silently pass through. This lets a request
            // mixing article-side and store-side attributes work
            // against an article target without erroring — the
            // store_* filters become no-ops here, and the store path
            // (if any) handles them separately.
            candidates.clone()
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 2 — project_distinct
// ──────────────────────────────────────────────────────────────────────────

/// For each attribute, collect distinct string values across the
/// candidate set, sorted ASC. Unsupported attributes yield empty
/// vectors (parity with v1's behavior on unknown column names).
pub fn project_distinct(
    graph: &Graph,
    target_kind: KindId,
    candidates: &BTreeSet<NodeId>,
    attribute_names: &[&str],
) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::with_capacity(attribute_names.len());
    for attr in attribute_names {
        out.insert((*attr).to_string(), distinct_for(graph, target_kind, candidates, attr));
    }
    out
}

fn distinct_for(
    graph: &Graph,
    target_kind: KindId,
    candidates: &BTreeSet<NodeId>,
    attribute_name: &str,
) -> Vec<String> {
    let strategy = resolve_attribute(graph, target_kind, attribute_name);
    let mut set: HashSet<String> = HashSet::new();
    match strategy {
        AttributeStrategy::SelfName => {
            for id in candidates {
                set.insert(graph.get_str(graph.node(*id).name).to_string());
            }
        }
        AttributeStrategy::Ancestor(kind) => {
            for id in candidates {
                if let Some(name) = ancestor_name(graph, *id, kind) {
                    set.insert(name.to_string());
                }
            }
        }
        AttributeStrategy::CrossEdge(eid, _kind, direction) => {
            let idx = graph.cross_edges.get(eid);
            for id in candidates {
                let neighbors = match direction {
                    Direction::Forward => idx.forward.get(id),
                    Direction::Reverse => idx.reverse.get(id),
                };
                if let Some(ns) = neighbors {
                    for nid in ns {
                        set.insert(graph.get_str(graph.node(*nid).name).to_string());
                    }
                }
            }
        }
        AttributeStrategy::Unknown => {}
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}

// ──────────────────────────────────────────────────────────────────────────
// Attribute strategy resolution
// ──────────────────────────────────────────────────────────────────────────

/// How an attribute name maps onto graph data. Resolved once per
/// filter (or once per project_distinct call) and dispatched through
/// `narrow` / `distinct_for`. Keeping the dispatch table separate from
/// the per-row check lets us add new strategies (composite-axis,
/// future metadata-defined ad-hoc joins) without touching the row
/// loops.
#[derive(Debug, Clone, Copy)]
enum AttributeStrategy {
    SelfName,
    Ancestor(KindId),
    CrossEdge(CrossEdgeId, KindId, Direction),
    Unknown,
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    /// Target is `kind_a` on the registered edge; walk `forward`.
    Forward,
    /// Target is `kind_b`; walk `reverse`.
    Reverse,
}

fn resolve_attribute(
    graph: &Graph,
    target_kind: KindId,
    attribute_name: &str,
) -> AttributeStrategy {
    // 1. Self-name.
    let target_meta = graph.kinds.get(target_kind);
    if attribute_name == target_meta.name {
        return AttributeStrategy::SelfName;
    }
    // 2. Ancestor kind. Resolve attribute → KindId, then verify the
    // kind appears in the target's ancestor chain. We don't have a
    // per-kind "is-ancestor-of" precomputation, but the spine is
    // shallow (bealls: 8 levels max), so a single-node ancestor walk
    // is enough — pick any target-kind node and check.
    if let Some(attr_kind) = graph.kinds.id_of(attribute_name) {
        // Find one example node of target_kind, walk ancestors, see
        // if attr_kind appears.
        if let Some(example) = graph
            .nodes
            .iter()
            .enumerate()
            .find(|(_, n)| n.kind == target_kind)
            .map(|(i, _)| NodeId(i as u32))
        {
            if is_ancestor_kind(graph, example, attr_kind) {
                return AttributeStrategy::Ancestor(attr_kind);
            }
        }
        // 3. Cross-edge target.
        for (i, meta) in graph.cross_edges.metas.iter().enumerate() {
            let eid = CrossEdgeId(i as u32);
            if meta.kind_a == target_kind && meta.kind_b == attr_kind {
                return AttributeStrategy::CrossEdge(eid, attr_kind, Direction::Forward);
            }
            if meta.kind_b == target_kind && meta.kind_a == attr_kind {
                return AttributeStrategy::CrossEdge(eid, attr_kind, Direction::Reverse);
            }
        }
    }
    AttributeStrategy::Unknown
}

fn is_ancestor_kind(graph: &Graph, from: NodeId, kind: KindId) -> bool {
    let mut cur = graph.node(from).parent;
    while !cur.is_none() {
        let n = graph.node(cur);
        if n.kind == kind {
            return true;
        }
        if cur == graph.root {
            break;
        }
        cur = n.parent;
    }
    false
}

fn ancestor_name<'a>(graph: &'a Graph, id: NodeId, kind: KindId) -> Option<&'a str> {
    let mut cur = graph.node(id).parent;
    while !cur.is_none() {
        let n = graph.node(cur);
        if n.kind == kind {
            return Some(graph.get_str(n.name));
        }
        if cur == graph.root {
            break;
        }
        cur = n.parent;
    }
    None
}

fn cross_edge_matches(
    graph: &Graph,
    id: NodeId,
    eid: CrossEdgeId,
    direction: Direction,
    needles: &HashSet<&str>,
) -> bool {
    let idx = graph.cross_edges.get(eid);
    let neighbors = match direction {
        Direction::Forward => idx.forward.get(&id),
        Direction::Reverse => idx.reverse.get(&id),
    };
    let Some(ns) = neighbors else { return false };
    ns.iter().any(|nid| needles.contains(graph.get_str(graph.node(*nid).name)))
}

// ──────────────────────────────────────────────────────────────────────────
// Wire adapters — v1 cross_filter model → v2 FilterCriterion
// ──────────────────────────────────────────────────────────────────────────

impl From<&crate::cross_filter::model::Operator> for FilterOperator {
    /// Map the v1 wire operator to the v2 internal enum. `Eq`/`In`/
    /// `InEq` collapse to membership; `Ne` (and v1's redundant `IsNot`)
    /// preserves its inequality intent; everything else lands in
    /// `Unsupported` so the resolver logs and skips rather than
    /// fabricating a partial match.
    fn from(op: &crate::cross_filter::model::Operator) -> Self {
        use crate::cross_filter::model::Operator as V1;
        match op {
            V1::In | V1::InEq | V1::Eq | V1::IsEq => FilterOperator::In,
            V1::Ne | V1::IsNot | V1::NotIn => FilterOperator::Ne,
            // Range / pattern ops have no graph-traversal analogue
            // today — fall through to Unsupported. Decision: add them
            // when a caller actually needs them rather than burning
            // cycles synthesizing semantics nobody asked for.
            V1::Gt | V1::Lt | V1::Gte | V1::Lte | V1::Like | V1::ILike | V1::Between => {
                FilterOperator::Unsupported
            }
        }
    }
}

impl From<&crate::cross_filter::model::Filter> for FilterCriterion {
    fn from(f: &crate::cross_filter::model::Filter) -> Self {
        FilterCriterion {
            attribute_name: f.attribute_name.clone(),
            values: f.values.as_strings(),
            operator: (&f.operator).into(),
        }
    }
}

/// Helper for handler code: convert a v1 `FilterPayload` to the
/// `(filters, attribute_names)` pair `apply_filters` + `project_distinct`
/// expect. UAM (`is_urm_filter` / `user_code` / `acl_code`) is the
/// caller's responsibility — they look up the EntitledSet separately
/// since the v2 resolver doesn't have an opinion on how UAM resolves.
pub fn filters_from_payload(
    payload: &crate::cross_filter::model::FilterPayload,
) -> (Vec<FilterCriterion>, Vec<String>) {
    let filters: Vec<FilterCriterion> = payload.filters.iter().map(FilterCriterion::from).collect();
    let attributes: Vec<String> = payload
        .attributes
        .iter()
        .map(|a| a.attribute_name.clone())
        .collect();
    (filters, attributes)
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build::build_graph;
    use crate::graph::source::{CellValue, Row, SourceReader};
    use crate::graph::spec::from_toml;
    use anyhow::Result;

    struct MockReader { tables: HashMap<String, MockTable> }
    struct MockTable { columns: Vec<String>, rows: Vec<Vec<CellValue>> }
    impl SourceReader for MockReader {
        fn read(&self, table: &str, columns: &[String], _filter: Option<&str>) -> Result<Vec<Row>> {
            let t = match self.tables.get(table) { Some(t) => t, None => return Err(anyhow::anyhow!("mock: no table `{}`", table)) };
            let col_idx: Vec<Option<usize>> =
                columns.iter().map(|c| t.columns.iter().position(|x| x == c)).collect();
            Ok(t.rows.iter().map(|raw| Row {
                cells: col_idx.iter().map(|idx| match idx {
                    Some(i) => raw[*i].clone(),
                    None => CellValue::Null,
                }).collect(),
            }).collect())
        }
    }
    fn ts(s: &str) -> CellValue { CellValue::Text(s.to_string()) }

    fn fixture() -> Graph {
        let toml = r#"
id = "g"
display_name = "G"

[[sources]]
alias = "products"
table = "products_tbl"
attaches_at = "article"

[[sources]]
alias = "brands"
table = "brands_tbl"
attaches_at = "brand"

[[sources]]
alias = "pb_link"
table = "pb_link_tbl"

[[relation]]
from = { alias = "pb_link",  columns = ["article_id"], cardinality = "*" }
to   = { alias = "products", columns = ["article"],    cardinality = "1" }

[[relation]]
from = { alias = "pb_link", columns = ["brand_id"],   cardinality = "*" }
to   = { alias = "brands",  columns = ["brand_name"], cardinality = "1" }

[hierarchy.product]
source = "products"

[hierarchy.product.l0]
column = "l0"

[hierarchy.product.article]
column = "article"

[hierarchy.brand]
source = "brands"

[hierarchy.brand.brand]
column = "brand_name"
"#;
        let spec = from_toml(toml).unwrap();
        let mut tables = HashMap::new();
        tables.insert("products_tbl".to_string(), MockTable {
            columns: vec!["l0".into(), "article".into()],
            rows: vec![
                vec![ts("L0_A"), ts("A1")],
                vec![ts("L0_A"), ts("A2")],
                vec![ts("L0_B"), ts("A3")],
                vec![ts("L0_B"), ts("A4")],
            ],
        });
        tables.insert("brands_tbl".to_string(), MockTable {
            columns: vec!["brand_name".into()],
            rows: vec![vec![ts("B1")], vec![ts("B2")]],
        });
        tables.insert("pb_link_tbl".to_string(), MockTable {
            columns: vec!["article_id".into(), "brand_id".into()],
            rows: vec![
                vec![ts("A1"), ts("B1")],
                vec![ts("A2"), ts("B1")],
                vec![ts("A3"), ts("B2")],
                vec![ts("A4"), ts("B2")],
            ],
        });
        let reader = MockReader { tables };
        let (g, _) = build_graph(&spec, &reader, 1).expect("build");
        g
    }

    fn art(g: &Graph) -> KindId { g.kinds.id_of("article").unwrap() }

    fn membership(attr: &str, vals: &[&str]) -> FilterCriterion {
        FilterCriterion {
            attribute_name: attr.to_string(),
            values: vals.iter().map(|s| s.to_string()).collect(),
            operator: FilterOperator::In,
        }
    }

    #[test]
    fn no_filters_returns_all_target_nodes() {
        let g = fixture();
        let set = apply_filters(&g, art(&g), &[], None);
        // 4 articles seeded.
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn filter_by_ancestor_kind() {
        let g = fixture();
        let set = apply_filters(&g, art(&g), &[membership("l0", &["L0_A"])], None);
        // L0_A contains A1, A2.
        let names: BTreeSet<&str> = set
            .iter()
            .map(|id| g.get_str(g.node(*id).name))
            .collect();
        assert_eq!(names, BTreeSet::from(["A1", "A2"]));
    }

    #[test]
    fn filter_by_self_name() {
        let g = fixture();
        let set = apply_filters(&g, art(&g), &[membership("article", &["A2", "A3"])], None);
        let names: BTreeSet<&str> = set
            .iter()
            .map(|id| g.get_str(g.node(*id).name))
            .collect();
        assert_eq!(names, BTreeSet::from(["A2", "A3"]));
    }

    #[test]
    fn filter_by_cross_edge_target() {
        let g = fixture();
        // Filter articles whose linked brand is B1 (= A1, A2).
        let set = apply_filters(&g, art(&g), &[membership("brand", &["B1"])], None);
        let names: BTreeSet<&str> = set
            .iter()
            .map(|id| g.get_str(g.node(*id).name))
            .collect();
        assert_eq!(names, BTreeSet::from(["A1", "A2"]));
    }

    #[test]
    fn multiple_filters_and_compose() {
        let g = fixture();
        // L0_B and brand B2 → A3 + A4.
        let set = apply_filters(
            &g,
            art(&g),
            &[membership("l0", &["L0_B"]), membership("brand", &["B2"])],
            None,
        );
        let names: BTreeSet<&str> = set
            .iter()
            .map(|id| g.get_str(g.node(*id).name))
            .collect();
        assert_eq!(names, BTreeSet::from(["A3", "A4"]));
    }

    #[test]
    fn entitled_set_restricts_results() {
        let g = fixture();
        let art_kind = art(&g);
        let a1 = g.find_by_name(art_kind, "A1").unwrap();
        let a3 = g.find_by_name(art_kind, "A3").unwrap();
        let mut allowed = HashSet::new();
        allowed.insert(a1);
        allowed.insert(a3);
        let entitled = EntitledSet { allowed: Some(allowed) };
        let set = apply_filters(&g, art_kind, &[], Some(&entitled));
        let names: BTreeSet<&str> = set
            .iter()
            .map(|id| g.get_str(g.node(*id).name))
            .collect();
        assert_eq!(names, BTreeSet::from(["A1", "A3"]));
    }

    #[test]
    fn unsupported_operator_is_skipped() {
        let g = fixture();
        let crit = FilterCriterion {
            attribute_name: "article".into(),
            values: vec!["A1".into()],
            operator: FilterOperator::Unsupported,
        };
        let set = apply_filters(&g, art(&g), &[crit], None);
        // Skipped → no narrowing → all 4 articles.
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn project_distinct_sorts_and_dedupes() {
        let g = fixture();
        let set = apply_filters(&g, art(&g), &[membership("l0", &["L0_A"])], None);
        let projected = project_distinct(&g, art(&g), &set, &["article", "l0", "brand"]);
        assert_eq!(projected.get("article").unwrap(), &vec!["A1".to_string(), "A2".to_string()]);
        // L0_A is the only L0 in the narrowed set.
        assert_eq!(projected.get("l0").unwrap(), &vec!["L0_A".to_string()]);
        // Both A1 and A2 link to B1.
        assert_eq!(projected.get("brand").unwrap(), &vec!["B1".to_string()]);
    }

    #[test]
    fn unknown_attribute_yields_empty_distinct() {
        let g = fixture();
        let set = apply_filters(&g, art(&g), &[], None);
        let projected = project_distinct(&g, art(&g), &set, &["store_code"]);
        assert_eq!(projected.get("store_code").unwrap(), &Vec::<String>::new());
    }
}
