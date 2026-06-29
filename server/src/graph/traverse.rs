//! Generic graph traversal: `traverse(graph, from, edge) → [NodeId]`.
//!
//! Mirrors the surface of `graph::legacy::traverse` but returns raw
//! `NodeId`s instead of projected `serde_json::Value` rows. The v2
//! projection layer (kind → column-set rendering) lives separately and
//! consumes traversal output downstream — keeping traversal pure makes
//! it cheap to test and reuse from non-HTTP call sites (CDC delta
//! refresh, internal aggregate lookups).
//!
//! Five built-in edges:
//! - `Children` / `Parent` / `Ancestors` — spine, comes off the
//!   `Node.parent` / `Node.children` fields directly.
//! - `DescendantsOfKind(name)` — DFS down the spine, collecting nodes
//!   of the named kind. Once a target-kind node is found, descent
//!   stops at it (don't recurse into article → product_code when
//!   you asked for articles).
//! - `CrossEdge(bridge_source)` — looks up the cross-edge registered
//!   for the named bridge source and walks forward or reverse
//!   depending on which side `from`'s kind matches.
//!
//! Unsupported `(from_kind, edge)` pairs return an empty vec rather
//! than erroring, matching the "graceful degradation" pattern most
//! callers want — a UI affordance for "no rows" is easier than one
//! for "this combination is invalid".

use super::graph::{CrossEdgeId, Graph, KindId, NodeId};

/// Edge label. Names are borrowed for `DescendantsOfKind` and
/// `CrossEdge` so callers can pass `&'static str` literals or
/// freshly-parsed strings without churning ownership.
#[derive(Debug, Clone, Copy)]
pub enum Edge<'a> {
    Children,
    Parent,
    Ancestors,
    /// All descendants of `from` whose `KindId` resolves to the named
    /// level id (e.g. `"article"`). DFS stops at each target-kind
    /// node — doesn't recurse further, matching v1's
    /// `project_subtree_articles` semantics.
    DescendantsOfKind(&'a str),
    /// Walk a cross-edge by the bridge source's alias. The direction
    /// is inferred from `from.kind` — if it matches `kind_a` in the
    /// registered `CrossEdgeMeta`, the forward map applies; if
    /// `kind_b`, the reverse map applies; otherwise the result is
    /// empty.
    CrossEdge(&'a str),
}

/// Walk `edge` from `from` and return the resolved `NodeId`s. Order
/// is deterministic (children declaration order for spine edges,
/// row order for cross-edges), so snapshot-style tests don't have to
/// sort.
pub fn traverse(graph: &Graph, from: NodeId, edge: Edge<'_>) -> Vec<NodeId> {
    if from.is_none() {
        return Vec::new();
    }
    match edge {
        Edge::Children => graph.node(from).children.iter().copied().collect(),
        Edge::Parent => {
            let p = graph.node(from).parent;
            if p.is_none() || p == graph.root {
                Vec::new()
            } else {
                vec![p]
            }
        }
        Edge::Ancestors => {
            // `Graph::ancestors` yields the starting node too;
            // traversal semantics expect ancestors only, so skip the
            // first element. Drop root automatically (ancestors() does
            // that already).
            graph.ancestors(from).skip(1).collect()
        }
        Edge::DescendantsOfKind(kind_name) => match graph.kinds.id_of(kind_name) {
            None => Vec::new(),
            Some(target) => descendants_of_kind(graph, from, target),
        },
        Edge::CrossEdge(bridge_alias) => walk_cross_edge(graph, from, bridge_alias),
    }
}

/// DFS from `root` collecting nodes whose `KindId == target`. Stops
/// recursion at each match — for the bealls case this prevents an
/// L0 → articles walk from also pulling in every product_code.
fn descendants_of_kind(graph: &Graph, root: NodeId, target: KindId) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = Vec::new();
    let mut stack: Vec<NodeId> = vec![root];
    while let Some(id) = stack.pop() {
        let n = graph.node(id);
        if n.kind == target && id != root {
            out.push(id);
            continue;
        }
        for &c in &n.children {
            stack.push(c);
        }
    }
    out
}

/// Look up the cross-edge registered for `bridge_alias` and resolve
/// `from`'s neighbors on whichever side matches its kind. Returns
/// empty when the bridge isn't found or `from`'s kind isn't an
/// endpoint of any registered cross-edge with this alias.
fn walk_cross_edge(graph: &Graph, from: NodeId, bridge_alias: &str) -> Vec<NodeId> {
    let from_kind = graph.node(from).kind;
    // Multiple bridges can share the same source alias only if a tenant
    // registered the same source twice — which validation doesn't
    // allow. Still, iterate to find the first matching one rather than
    // requiring a uniqueness invariant from the registry layer.
    let cross_edge_id: Option<CrossEdgeId> = graph
        .cross_edges
        .metas
        .iter()
        .enumerate()
        .find(|(_, m)| m.bridge_source == bridge_alias)
        .map(|(i, _)| CrossEdgeId(i as u32));
    let Some(eid) = cross_edge_id else {
        return Vec::new();
    };
    let meta = &graph.cross_edges.metas[eid.0 as usize];
    let idx = graph.cross_edges.get(eid);
    if from_kind == meta.kind_a {
        idx.forward.get(&from).map(|v| v.to_vec()).unwrap_or_default()
    } else if from_kind == meta.kind_b {
        idx.reverse.get(&from).map(|v| v.to_vec()).unwrap_or_default()
    } else {
        Vec::new()
    }
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
    use std::cell::RefCell;
    use std::collections::HashMap;

    // Minimal MockSourceReader (duplicated from build::tests so this
    // module's tests don't depend on build's `#[cfg(test)]` privates).
    struct MockReader {
        tables: HashMap<String, MockTable>,
    }
    struct MockTable {
        columns: Vec<String>,
        rows: Vec<Vec<CellValue>>,
    }
    impl SourceReader for MockReader {
        fn read(
            &self,
            table: &str,
            columns: &[String],
            _filter: Option<&str>,
        ) -> Result<Vec<Row>> {
            let t = match self.tables.get(table) { Some(t) => t, None => return Err(anyhow::anyhow!("mock: no table `{}`", table)) };
            let col_idx: Vec<Option<usize>> =
                columns.iter().map(|c| t.columns.iter().position(|x| x == c)).collect();
            Ok(t.rows
                .iter()
                .map(|raw| {
                    let cells = col_idx
                        .iter()
                        .map(|idx| match idx {
                            Some(i) => raw[*i].clone(),
                            None => CellValue::Null,
                        })
                        .collect();
                    Row { cells }
                })
                .collect())
        }
    }
    fn ts(s: &str) -> CellValue { CellValue::Text(s.to_string()) }

    /// Build the same 2-hierarchy + bridge fixture used in the
    /// `bridge_source_produces_cross_edges` test, so traversal can
    /// exercise every Edge variant against a non-trivial shape.
    fn fixture() -> Graph {
        let _ = RefCell::new(0); // silence unused-import nag in some cfgs
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
        tables.insert(
            "products_tbl".to_string(),
            MockTable {
                columns: vec!["l0".to_string(), "article".to_string()],
                rows: vec![
                    vec![ts("L0_A"), ts("A1")],
                    vec![ts("L0_A"), ts("A2")],
                    vec![ts("L0_B"), ts("A3")],
                ],
            },
        );
        tables.insert(
            "brands_tbl".to_string(),
            MockTable {
                columns: vec!["brand_name".to_string()],
                rows: vec![vec![ts("B1")], vec![ts("B2")]],
            },
        );
        tables.insert(
            "pb_link_tbl".to_string(),
            MockTable {
                columns: vec!["article_id".to_string(), "brand_id".to_string()],
                rows: vec![
                    vec![ts("A1"), ts("B1")],
                    vec![ts("A2"), ts("B1")],
                    vec![ts("A3"), ts("B2")],
                ],
            },
        );
        let reader = MockReader { tables };
        let (g, _) = build_graph(&spec, &reader, 1).expect("build");
        g
    }

    #[test]
    fn children_returns_direct_descendants() {
        let g = fixture();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let art_kind = g.kinds.id_of("article").unwrap();
        let l0_a = g.find_by_name(l0_kind, "L0_A").unwrap();
        let kids = traverse(&g, l0_a, Edge::Children);
        // L0_A has two article children (A1, A2).
        assert_eq!(kids.len(), 2);
        for k in &kids {
            assert_eq!(g.node(*k).kind, art_kind);
        }
    }

    #[test]
    fn parent_returns_immediate_ancestor() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let a1 = g.find_by_name(art_kind, "A1").unwrap();
        let parents = traverse(&g, a1, Edge::Parent);
        assert_eq!(parents.len(), 1);
        assert_eq!(g.node(parents[0]).kind, l0_kind);
    }

    #[test]
    fn ancestors_walks_to_but_excludes_root() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let a1 = g.find_by_name(art_kind, "A1").unwrap();
        let chain = traverse(&g, a1, Edge::Ancestors);
        // Only L0_A is above A1 (besides root).
        assert_eq!(chain.len(), 1);
        assert_eq!(g.node(chain[0]).kind, l0_kind);
    }

    #[test]
    fn descendants_of_kind_pulls_subtree_articles() {
        let g = fixture();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let art_kind = g.kinds.id_of("article").unwrap();
        let l0_a = g.find_by_name(l0_kind, "L0_A").unwrap();
        let articles = traverse(&g, l0_a, Edge::DescendantsOfKind("article"));
        assert_eq!(articles.len(), 2);
        for a in &articles {
            assert_eq!(g.node(*a).kind, art_kind);
        }
    }

    #[test]
    fn cross_edge_walks_forward_and_reverse() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        let brand_kind = g.kinds.id_of("brand").unwrap();

        // Forward: article → brand.
        let a1 = g.find_by_name(art_kind, "A1").unwrap();
        let a1_brands = traverse(&g, a1, Edge::CrossEdge("pb_link"));
        assert_eq!(a1_brands.len(), 1);
        assert_eq!(g.node(a1_brands[0]).kind, brand_kind);
        // Sanity: the brand resolved is B1.
        let b1 = g.find_by_name(brand_kind, "B1").unwrap();
        assert_eq!(a1_brands[0], b1);

        // Reverse: brand → [articles]. B1 has A1 + A2.
        let b1_articles = traverse(&g, b1, Edge::CrossEdge("pb_link"));
        assert_eq!(b1_articles.len(), 2);
        let a2 = g.find_by_name(art_kind, "A2").unwrap();
        assert!(b1_articles.contains(&a1));
        assert!(b1_articles.contains(&a2));
    }

    #[test]
    fn cross_edge_with_wrong_kind_returns_empty() {
        let g = fixture();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let l0_a = g.find_by_name(l0_kind, "L0_A").unwrap();
        // L0 isn't an endpoint of the article↔brand bridge.
        assert!(traverse(&g, l0_a, Edge::CrossEdge("pb_link")).is_empty());
    }
}
