//! Project a `NodeId` to a `serde_json::Value` row.
//!
//! v1's `graph::legacy::projection::columns_for` hard-codes a 12-arm
//! `match NodeKind` returning per-kind column lists. v2 derives the
//! same shape from the registry: id + kind + name come for free,
//! `include_ancestors` walks the spine yielding `{<kind>: <name>}`
//! per ancestor, `include_metrics` reads `Node.metrics` paired with
//! `MetricRegistry`, and `include_cross_edges` walks the registered
//! `CrossEdgeRegistry` for whichever bridges have `from` on either
//! endpoint.
//!
//! All three opt-ins default to false so callers (and the traverse
//! handler) get a minimal row unless they ask for more — keeps wire
//! payloads small for large traversals (an L0 → articles call that
//! returns 48 K rows should not also serialize 48 K × 8 metrics
//! unless the caller wants that).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::graph::{CrossEdgeId, Graph, MetricValue, NodeId};

/// What to include in each projected row. `id`/`kind`/`name` are
/// always emitted; the three fields here gate the optional sections.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub struct ProjectionOptions {
    /// Emit `"ancestors": { "<kind>": "<name>", … }` walking from the
    /// node's parent up to (excluding) root.
    #[serde(default)]
    pub include_ancestors: bool,
    /// Emit `"metrics": { "<metric_name>": <value>, … }` reading
    /// every primary-metric slot on the node. Composite metrics are
    /// elided here — they don't live on `Node.metrics` and need a
    /// separate cube-lookup API.
    #[serde(default)]
    pub include_metrics: bool,
    /// Emit `"cross_edges": { "<bridge_alias>": [<name>, …], … }` for
    /// every registered cross-edge where the node is an endpoint.
    /// Walks the forward map when the node's kind matches `kind_a`
    /// and the reverse map when it matches `kind_b`.
    #[serde(default)]
    pub include_cross_edges: bool,
}

/// Render one node. Returns `Value::Null` only when `node` is
/// `NodeId::NONE` — for the synthetic root and out-of-range cases
/// callers should generally pre-filter rather than relying on this.
pub fn project(graph: &Graph, node: NodeId, opts: &ProjectionOptions) -> Value {
    if node.is_none() {
        return Value::Null;
    }
    let n = graph.node(node);
    let mut row = Map::new();
    row.insert("id".into(), json!(node.0));
    row.insert("kind".into(), json!(graph.kinds.get(n.kind).name));
    row.insert("name".into(), json!(graph.get_str(n.name)));

    if opts.include_ancestors {
        // `Graph::ancestors` yields the node itself first; skip it so
        // the "ancestors" payload contains only *strict* ancestors.
        let mut ancestors = Map::new();
        for ancestor_id in graph.ancestors(node).skip(1) {
            let a = graph.node(ancestor_id);
            let kind_name = &graph.kinds.get(a.kind).name;
            ancestors.insert(kind_name.clone(), json!(graph.get_str(a.name)));
        }
        row.insert("ancestors".into(), Value::Object(ancestors));
    }

    if opts.include_metrics {
        let primary_ids = graph.metrics.primary_metric_ids();
        let mut metrics = Map::new();
        // Two-pass walk in *slot order* (== spec declaration order)
        // — `serde_json::Map`'s alphabetical iteration would otherwise
        // scramble which source wins a bare name. Pass 1 claims each
        // bare name for the FIRST-declared source; pass 2 emits the
        // losers under their `<source>.<metric>` prefixed form so
        // callers can still disambiguate (e.g. `inventory_per_dc.oh`
        // when a dc-level dataview wants the dc-attached value).
        let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (slot, mid) in primary_ids.iter().enumerate() {
            let meta = graph.metrics.get(*mid);
            let value = metric_value_to_json(graph, &n.metrics[slot]);
            if taken.insert(meta.name.clone()) {
                metrics.insert(meta.name.clone(), value);
            } else {
                metrics.insert(format!("{}.{}", meta.source_alias, meta.name), value);
            }
        }
        row.insert("metrics".into(), Value::Object(metrics));
    }

    if opts.include_cross_edges {
        let mut edges = Map::new();
        for (i, meta) in graph.cross_edges.metas.iter().enumerate() {
            let eid = CrossEdgeId(i as u32);
            let idx = graph.cross_edges.get(eid);
            let neighbors: &smallvec::SmallVec<[NodeId; 4]> = if n.kind == meta.kind_a {
                match idx.forward.get(&node) {
                    Some(v) => v,
                    None => continue,
                }
            } else if n.kind == meta.kind_b {
                match idx.reverse.get(&node) {
                    Some(v) => v,
                    None => continue,
                }
            } else {
                continue;
            };
            let names: Vec<Value> = neighbors
                .iter()
                .map(|nid| json!(graph.get_str(graph.node(*nid).name)))
                .collect();
            edges.insert(meta.bridge_source.clone(), Value::Array(names));
        }
        row.insert("cross_edges".into(), Value::Object(edges));
    }

    Value::Object(row)
}

/// `MetricValue` → JSON. Set/List are serialized as arrays of the
/// underlying string values (resolved through the pool); Scalar is a
/// JSON number; Bool maps directly. This is the read-time
/// interpretation Decision 13's "count_distinct surfaces as set.len()"
/// alludes to — but we leave the count_distinct → length collapse to
/// the caller, since it's trivial on the client side and exposing the
/// full set is strictly more information.
fn metric_value_to_json(graph: &Graph, v: &MetricValue) -> Value {
    match v {
        MetricValue::Scalar(f) => {
            // Drop NaN / ±inf back to null — they're operator-identity
            // artifacts for min/max on empty subtrees, not real values.
            if f.is_finite() { json!(f) } else { Value::Null }
        }
        MetricValue::Set(set) => Value::Array(
            set.iter()
                .map(|sid| json!(graph.get_str(*sid)))
                .collect(),
        ),
        MetricValue::List(list) => Value::Array(
            list.iter()
                .map(|sid| json!(graph.get_str(*sid)))
                .collect(),
        ),
        MetricValue::Bool(b) => json!(b),
    }
}

/// Paginated projection over all nodes of a kind. Mirrors the
/// surface that DataView read paths expect — sort by node name,
/// slice the page, project each row.
///
/// Row shape: ancestors / metrics / cross_edges are **flattened**
/// into top-level columns so the DataView UI gets a normal table
/// (`{l0: "...", l1: "...", oh: 100, brand_to_articles: "...,...", ...}`)
/// rather than nested objects. Cross-edge neighbor lists are joined
/// into comma-separated strings; metric source-alias prefixes (e.g.
/// `inventory.oh`) drop the prefix when the metric name is unique
/// across sources.
pub fn project_page(
    graph: &Graph,
    kind_name: &str,
    sort_dir: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Value>, i64), String> {
    let kind_id = graph
        .kinds
        .id_of(kind_name)
        .ok_or_else(|| format!("unknown kind `{kind_name}` — check graph spec"))?;

    let mut ids: Vec<NodeId> = graph
        .iter_kind(kind_id)
        .filter(|nid| !graph.get_str(graph.node(*nid).name).is_empty())
        .collect();
    let desc = sort_dir == Some("desc");
    ids.sort_by(|a, b| {
        let na = graph.get_str(graph.node(*a).name);
        let nb = graph.get_str(graph.node(*b).name);
        if desc { nb.cmp(na) } else { na.cmp(nb) }
    });

    let total = ids.len() as i64;
    let off = offset.max(0) as usize;
    let lim = if limit > 0 { limit as usize } else { usize::MAX };
    let slice: &[NodeId] = if off >= ids.len() {
        &[]
    } else {
        let end = (off + lim).min(ids.len());
        &ids[off..end]
    };

    // Cross-edges are deliberately OFF for the dataview projection.
    // On the "many" side of a bridge (e.g. a brand node, which
    // connects to hundreds of article nodes via
    // `ph_master_brand_bridge`), the cross-edge cell carries every
    // connected node's name as a comma-joined string — 15K+ bytes
    // for a popular brand, useless for tabular display, and enough
    // to blow the agent's tool-output cap on a 10-row leaderboard.
    // Callers that genuinely want neighbor lists should hit the
    // `node` / `traverse` endpoints with `project.include_cross_edges
    // = true`.
    let opts = ProjectionOptions {
        include_ancestors: true,
        include_metrics: true,
        include_cross_edges: false,
    };
    let mut rows: Vec<Value> = Vec::with_capacity(slice.len());
    for &nid in slice {
        rows.push(flatten_row(project(graph, nid, &opts)));
    }
    Ok((rows, total))
}

/// Public-API alias for `flatten_row`. Callers outside this module
/// (e.g. the dataview-source filter path that needs to project an
/// arbitrary node subset) skip `project_page` and reach for `project`
/// + this — same shape as a `project_page` row.
pub fn flatten_row_public(row: Value) -> Value {
    flatten_row(row)
}

/// Flatten a `project()` row by inlining `ancestors` / `metrics` /
/// `cross_edges` sub-objects into top-level fields. Cross-edge arrays
/// get joined into comma-separated strings for tabular display.
fn flatten_row(row: Value) -> Value {
    let mut top = match row {
        Value::Object(m) => m,
        other => return other,
    };
    if let Some(Value::Object(ancestors)) = top.remove("ancestors") {
        for (k, v) in ancestors { top.insert(k, v); }
    }
    if let Some(Value::Object(metrics)) = top.remove("metrics") {
        // `project()` already collision-resolved bare names in slot
        // order (first declared source wins the bare name; later
        // sources keep their `<source>.<metric>` prefix). Just promote
        // each entry to top-level here.
        for (k, v) in metrics {
            top.insert(k, v);
        }
    }
    if let Some(Value::Object(edges)) = top.remove("cross_edges") {
        for (k, v) in edges {
            let s = match v {
                Value::Array(items) => items
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
                    .join(","),
                Value::String(s) => s,
                other => other.to_string(),
            };
            top.insert(k, Value::String(s));
        }
    }
    Value::Object(top)
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
    fn ti(i: i64) -> CellValue { CellValue::Int(i) }

    /// 2 levels (l0 → article) + scalar metric. Lets each opt-in
    /// section be tested in isolation against a known shape.
    fn fixture() -> Graph {
        let toml = r#"
id = "g"
display_name = "G"

[[sources]]
alias = "products"
table = "products_tbl"
attaches_at = "article"

[[sources]]
alias = "inv"
table = "inv_tbl"
attaches_at = "article"

[hierarchy.product]
source = "products"

[hierarchy.product.l0]
column = "l0"

[hierarchy.product.article]
column = "article"

[metrics.inv]
oh = { rollup = "sum" }
"#;
        let spec = from_toml(toml).unwrap();
        let mut tables = HashMap::new();
        tables.insert("products_tbl".to_string(), MockTable {
            columns: vec!["l0".into(), "article".into()],
            rows: vec![vec![ts("L0_A"), ts("A1")], vec![ts("L0_A"), ts("A2")]],
        });
        tables.insert("inv_tbl".to_string(), MockTable {
            columns: vec!["article".into(), "oh".into()],
            rows: vec![vec![ts("A1"), ti(10)], vec![ts("A2"), ti(25)]],
        });
        let reader = MockReader { tables };
        let (g, _) = build_graph(&spec, &reader, 1).expect("build");
        g
    }

    #[test]
    fn project_minimal_emits_id_kind_name() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        let a1 = g.find_by_name(art_kind, "A1").unwrap();
        let row = project(&g, a1, &ProjectionOptions::default());
        assert_eq!(row["kind"], json!("article"));
        assert_eq!(row["name"], json!("A1"));
        assert!(row.get("ancestors").is_none());
        assert!(row.get("metrics").is_none());
        assert!(row.get("cross_edges").is_none());
    }

    #[test]
    fn project_with_ancestors_yields_kind_indexed_map() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        let a1 = g.find_by_name(art_kind, "A1").unwrap();
        let opts = ProjectionOptions { include_ancestors: true, ..Default::default() };
        let row = project(&g, a1, &opts);
        let ancestors = row["ancestors"].as_object().expect("ancestors object");
        // Only L0_A is a strict ancestor of A1 (root excluded).
        assert_eq!(ancestors.len(), 1);
        assert_eq!(ancestors.get("l0"), Some(&json!("L0_A")));
    }

    #[test]
    fn project_with_metrics_shows_rolled_value() {
        let g = fixture();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        let l0_a = g.find_by_name(l0_kind, "L0_A").unwrap();
        let opts = ProjectionOptions { include_metrics: true, ..Default::default() };
        let row = project(&g, l0_a, &opts);
        // L0_A = A1.oh + A2.oh = 10 + 25 = 35 after rollup.
        let metrics = row["metrics"].as_object().unwrap();
        assert_eq!(metrics["inv.oh"], json!(35.0));
    }
}
