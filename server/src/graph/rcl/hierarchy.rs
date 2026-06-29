//! Build a borrow-ready `rcl::ProductHierarchy` from a v2 Graph node.
//!
//! v1 read these fields directly off `PhMasterRow` during build (it
//! had the source row in hand). v2 doesn't — the hierarchy data is
//! distributed across the graph spine (ancestors give l0..l5 + article),
//! the article's children give product_code, and brand lives on the
//! `ph_master_brand_bridge` cross-edge.
//!
//! ## Field mapping (bealls)
//!
//! Decisions 14 + 35 keep RCL out of the metadata schema, so this
//! module is bealls-specific. The v2 kind names map to RCL field
//! names as:
//!
//! | v2 kind  | RCL field    |
//! |----------|--------------|
//! | l0       | l0_name      |
//! | l1       | l1_name      |
//! | l2       | l2_name      |
//! | l3       | l3_name      |
//! | l4       | l4_name      |
//! | l5       | l5_name      |
//! | article  | (not used)   |
//! | brand    | brand        |
//! | product_code | product_code |
//!
//! If a tenant's TOML uses different kind names (e.g. `dept`/`class`
//! instead of `l0`/`l1`), an explicit mapping table belongs here.

use crate::graph::graph::{CrossEdgeId, Graph, NodeId};

/// Owned mirror of `rcl::ProductHierarchy` (which holds borrows).
/// Build per article, then call [`OwnedHierarchy::borrow`] to feed the
/// rcl resolver. The intermediate owned form decouples the resolver
/// borrow lifetime from the graph borrow lifetime, simplifying
/// callers that want to retain the resolver result past the graph
/// access.
#[derive(Debug, Clone, Default)]
pub struct OwnedHierarchy {
    pub product_code: String,
    pub l0_name: String,
    pub l1_name: String,
    pub l2_name: String,
    pub l3_name: String,
    pub l4_name: String,
    pub l5_name: String,
    pub brand: String,
}

impl OwnedHierarchy {
    /// Borrow the owned strings as a `rcl::ProductHierarchy`. Lifetime
    /// is tied to `self`, so callers must keep the `OwnedHierarchy`
    /// alive across the resolver call (typical pattern: build the
    /// owned form in a let-binding, borrow once, use the result).
    pub fn borrow(&self) -> rcl::ProductHierarchy<'_> {
        rcl::ProductHierarchy {
            product_code: &self.product_code,
            l0_name: &self.l0_name,
            l1_name: &self.l1_name,
            l2_name: &self.l2_name,
            l3_name: &self.l3_name,
            l4_name: &self.l4_name,
            l5_name: &self.l5_name,
            brand: &self.brand,
        }
    }
}

/// Build the owned hierarchy view for one article node. Walks the
/// spine to fill in l0..l5, picks the article's first product_code
/// child as the leaf identifier, and consults the article↔brand
/// cross-edge for brand.
///
/// When the kind isn't an `article` node, returns a hierarchy
/// populated as best we can — most callers should pre-filter by
/// kind, but defensive behavior here keeps the resolver from
/// crashing on a misrouted call.
pub fn owned_hierarchy_for(graph: &Graph, node_id: NodeId) -> OwnedHierarchy {
    let mut h = OwnedHierarchy::default();
    if node_id.is_none() {
        return h;
    }
    // Walk ancestors and copy named-level fields. The match arms are
    // hardcoded to the bealls naming convention; see module docs for
    // the field-mapping table.
    let mut cur = graph.node(node_id).parent;
    while !cur.is_none() && cur != graph.root {
        let n = graph.node(cur);
        let kind_name = &graph.kinds.get(n.kind).name;
        let value = graph.get_str(n.name).to_string();
        match kind_name.as_str() {
            "l0" => h.l0_name = value,
            "l1" => h.l1_name = value,
            "l2" => h.l2_name = value,
            "l3" => h.l3_name = value,
            "l4" => h.l4_name = value,
            "l5" => h.l5_name = value,
            _ => {}
        }
        cur = n.parent;
    }

    // product_code = the article's first child (per v1 convention).
    // bealls splits the pipe-delimited product_codes column into
    // multiple children; we take the first as the canonical id, same
    // as v1's `node.children.first()` path.
    if let Some(&pc_id) = graph.node(node_id).children.first() {
        h.product_code = graph.get_str(graph.node(pc_id).name).to_string();
    }

    // brand: walk the article↔brand cross-edge. Try every registered
    // edge whose endpoints include the article and a "brand"-named
    // kind on the other side. First match wins.
    let article_kind = graph.node(node_id).kind;
    let brand_kind = graph.kinds.id_of("brand");
    if let Some(brand_kind) = brand_kind {
        for (i, meta) in graph.cross_edges.metas.iter().enumerate() {
            let eid = CrossEdgeId(i as u32);
            let idx = graph.cross_edges.get(eid);
            let neighbors = if meta.kind_a == article_kind && meta.kind_b == brand_kind {
                idx.forward.get(&node_id)
            } else if meta.kind_b == article_kind && meta.kind_a == brand_kind {
                idx.reverse.get(&node_id)
            } else {
                continue;
            };
            if let Some(brands) = neighbors {
                if let Some(&first) = brands.first() {
                    h.brand = graph.get_str(graph.node(first).name).to_string();
                    break;
                }
            }
        }
    }

    h
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
    use std::collections::HashMap;

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

    /// Bealls-shaped fixture: l0..l1 + article + product_code (with
    /// split) + brand hierarchy + bridge. Enough to test every field
    /// in OwnedHierarchy.
    #[test]
    fn owned_hierarchy_walks_spine_and_cross_edge() {
        let toml = r#"
id = "g"
display_name = "G"

[[sources]]
alias = "ph_master"
table = "ph_master_tbl"
attaches_at = "article"

[[sources]]
alias = "ph_master_as_brand_source"
table = "ph_master_tbl"
attaches_at = "brand"

[[sources]]
alias = "ph_master_brand_bridge"
table = "ph_master_tbl"

[[relation]]
from = { alias = "ph_master_brand_bridge", columns = ["article"], cardinality = "*" }
to   = { alias = "ph_master",              columns = ["article"], cardinality = "1" }

[[relation]]
from = { alias = "ph_master_brand_bridge",    columns = ["brand"], cardinality = "*" }
to   = { alias = "ph_master_as_brand_source", columns = ["brand"], cardinality = "1" }

[hierarchy.product]
source = "ph_master"

[hierarchy.product.l0]
column = "l0"

[hierarchy.product.l1]
column = "l1"

[hierarchy.product.article]
column = "article"

[hierarchy.product.product_code]
column = "product_codes"
split  = "|"

[hierarchy.brand]
source = "ph_master_as_brand_source"

[hierarchy.brand.brand]
column = "brand"
"#;
        let spec = from_toml(toml).unwrap();
        let mut tables = HashMap::new();
        tables.insert(
            "ph_master_tbl".to_string(),
            MockTable {
                columns: vec![
                    "l0".into(),
                    "l1".into(),
                    "article".into(),
                    "product_codes".into(),
                    "brand".into(),
                ],
                rows: vec![vec![ts("30-bls"), ts("3510"), ts("A1"), ts("P1|P2"), ts("VENUS")]],
            },
        );
        let reader = MockReader { tables };
        let (g, _) = build_graph(&spec, &reader, 1).expect("build");

        let art_kind = g.kinds.id_of("article").unwrap();
        let a1 = g.find_by_name(art_kind, "A1").unwrap();
        let h = owned_hierarchy_for(&g, a1);

        assert_eq!(h.l0_name, "30-bls");
        assert_eq!(h.l1_name, "3510");
        // product_code = first child of A1 (= P1 since it was first in
        // the split, and split inserts in order).
        assert_eq!(h.product_code, "P1");
        // brand via cross-edge.
        assert_eq!(h.brand, "VENUS");

        // Borrow round-trips through ProductHierarchy.
        let p = h.borrow();
        assert_eq!(p.l0_name, "30-bls");
        assert_eq!(p.brand, "VENUS");
    }
}
