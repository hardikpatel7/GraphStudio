//! Project the in-memory graph into row-shaped output for the
//! DataView read path.
//!
//! Each [`crate::graph::legacy::NodeKind`] gets a different column
//! set: ARTICLE rows surface the full hierarchy + brand/channel +
//! rolled-up metrics; L0..L5 surface just that level's name + the
//! roll-up. Products and stores get their own minimal shapes.
//!
//! Used by `handlers::dataview_source` when a DataView is bound to a
//! source with `kind = "graph"`.

use rayon::prelude::*;
use serde_json::{Value, json};

use crate::graph::legacy::{ArticleGraph, MetricKind, NodeId, NodeKind};

/// Column metadata returned alongside `project_rows` so the introspect
/// path (`POST /api/dataviews/{id}/introspect-source`) and the data
/// path (`POST /api/dataviews/{id}/data`) can describe the schema
/// uniformly.
pub struct ColumnDef {
    pub name: &'static str,
    /// Loose DuckDB-style type label. Not authoritative — the graph
    /// is not a typed engine — used only for the schema panel.
    pub r#type: &'static str,
}

/// Resolve a `node_kind` config string into the concrete kind. Accepts
/// the same values the proto uses (uppercase: "L0".."L5", "ARTICLE",
/// "PRODUCT_CODE", "CHANNEL", "STORE_CODE"). Returns `None` if the
/// string doesn't map to a known kind.
pub fn parse_node_kind(s: &str) -> Option<NodeKind> {
    match s.to_ascii_uppercase().as_str() {
        "L0" => Some(NodeKind::L0),
        "L1" => Some(NodeKind::L1),
        "L2" => Some(NodeKind::L2),
        "L3" => Some(NodeKind::L3),
        "L4" => Some(NodeKind::L4),
        "L5" => Some(NodeKind::L5),
        "ARTICLE" => Some(NodeKind::Article),
        "PRODUCT_CODE" | "PRODUCTCODE" => Some(NodeKind::ProductCode),
        "CHANNEL" => Some(NodeKind::Channel),
        "STORE_CODE" | "STORECODE" => Some(NodeKind::StoreCode),
        _ => None,
    }
}

/// Columns emitted for each node_kind. Schema-only — `project_rows`
/// returns the actual values.
pub fn columns_for(kind: NodeKind) -> Vec<ColumnDef> {
    let metric_cols = || {
        vec![
            ColumnDef { name: "oh", r#type: "BIGINT" },
            ColumnDef { name: "oo", r#type: "BIGINT" },
            ColumnDef { name: "it", r#type: "BIGINT" },
            ColumnDef { name: "reserve_quantity", r#type: "BIGINT" },
            ColumnDef { name: "allocated_units", r#type: "BIGINT" },
            ColumnDef { name: "lw_units", r#type: "BIGINT" },
            ColumnDef { name: "lw_revenue", r#type: "BIGINT" },
            ColumnDef { name: "lw_margin", r#type: "BIGINT" },
        ]
    };

    let mut cols: Vec<ColumnDef> = Vec::new();
    match kind {
        NodeKind::Article => {
            cols.push(ColumnDef { name: "article", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l0_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l1_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l2_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l3_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l4_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l5_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "brand", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "channel", r#type: "VARCHAR" });
            // RCL-resolved columns (require a RuleSet; absent when read
            // through traversal/introspect paths). End-user data — codes
            // remain available via the per-row `rcl` link in the UI.
            cols.push(ColumnDef { name: "store_groups", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "dc_rule",      r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "min_stock",    r#type: "NUMERIC" });
            cols.push(ColumnDef { name: "max_stock",    r#type: "NUMERIC" });
            cols.push(ColumnDef { name: "wos",          r#type: "NUMERIC" });
            cols.push(ColumnDef { name: "aps",          r#type: "NUMERIC" });
            cols.extend(metric_cols());
        }
        NodeKind::L0 | NodeKind::L1 | NodeKind::L2 | NodeKind::L3 | NodeKind::L4 | NodeKind::L5 => {
            cols.push(ColumnDef { name: "name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "level", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "child_count", r#type: "BIGINT" });
            cols.extend(metric_cols());
        }
        NodeKind::ProductCode => {
            cols.push(ColumnDef { name: "product_code", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "article", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l0_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l1_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l2_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l3_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l4_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "l5_name", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "brand", r#type: "VARCHAR" });
        }
        NodeKind::StoreCode => {
            cols.push(ColumnDef { name: "store_code", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "channel", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "store_groups", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "dcs", r#type: "VARCHAR" });
        }
        NodeKind::Channel => {
            cols.push(ColumnDef { name: "channel", r#type: "VARCHAR" });
            cols.push(ColumnDef { name: "store_count", r#type: "BIGINT" });
        }
        NodeKind::Root => {
            cols.push(ColumnDef { name: "(root)", r#type: "VARCHAR" });
            cols.extend(metric_cols());
        }
    }
    cols
}

/// Project graph rows for the requested kind. Returns one row per
/// node of that kind (excluding the empty-name placeholder nodes the
/// build pass inserts when `ph_master` rows have empty l3..l5).
///
/// Parallelized via rayon: for the 48 K-article Bealls workload, the
/// projection is embarrassingly parallel (each node is independent;
/// the graph itself is `Sync`). Sequential projection ran ~600 ms;
/// par_iter brings it under ~150 ms on an 8-core machine.
pub fn project_rows(
    graph: &ArticleGraph,
    kind: NodeKind,
    ruleset: Option<&rcl::RuleSet>,
) -> Vec<Value> {
    let ids: Vec<NodeId> = graph.by_kind[kind.idx()].values().copied().collect();
    ids.par_iter()
        .filter_map(|&id| project_single(graph, kind, id, ruleset))
        .collect()
}

/// Per-node projection. Pure function — no shared state mutation.
/// Public so the traversal module can reuse it (single source of
/// truth for "what does a row of this kind look like").
///
/// `ruleset` is optional. When `Some` and `kind == Article`, the
/// projection populates the rcl-resolved end-user columns
/// (store_groups, dc_rule, min_stock, max_stock, wos, aps) by walking
/// the resolver. When `None`, those columns are emitted as null/empty
/// — that's the case for the traversal/introspect read paths where
/// the RuleSet isn't snapshotted.
pub fn project_single(
    graph: &ArticleGraph,
    kind: NodeKind,
    id: NodeId,
    ruleset: Option<&rcl::RuleSet>,
) -> Option<Value> {
    {
        let node = graph.node(id);
        let name = graph.get_str(node.name);
        // Skip the empty-string sentinel nodes that show up at the
        // root and for hierarchy levels missing on a given product.
        if name.is_empty() && !matches!(kind, NodeKind::Root) {
            return None;
        }
        let m = &node.metrics;
        let metric_obj = || {
            json!({
                "oh": m[MetricKind::Oh.idx()] as i64,
                "oo": m[MetricKind::Oo.idx()] as i64,
                "it": m[MetricKind::It.idx()] as i64,
                "reserve_quantity": m[MetricKind::ReserveQuantity.idx()] as i64,
                "allocated_units": m[MetricKind::AllocatedUnits.idx()] as i64,
                "lw_units": m[MetricKind::LwUnits.idx()] as i64,
                "lw_revenue": m[MetricKind::LwRevenue.idx()] as i64,
                "lw_margin": m[MetricKind::LwMargin.idx()] as i64,
            })
        };

        match kind {
            NodeKind::Article => {
                // Walk parents to collect l5..l0 names. The graph
                // build threaded these in the right order.
                let mut levels: [&str; 6] = ["", "", "", "", "", ""];
                let mut cur = node.parent;
                while !cur.is_none() {
                    let p = graph.node(cur);
                    let pname = graph.get_str(p.name);
                    match p.kind {
                        NodeKind::L0 => levels[0] = pname,
                        NodeKind::L1 => levels[1] = pname,
                        NodeKind::L2 => levels[2] = pname,
                        NodeKind::L3 => levels[3] = pname,
                        NodeKind::L4 => levels[4] = pname,
                        NodeKind::L5 => levels[5] = pname,
                        NodeKind::Root => break,
                        _ => {}
                    }
                    cur = p.parent;
                }
                // Brand: O(1) via the inverse index. (Without it,
                // 48 K × 3 K-brand scan is the dominant cost.)
                let brand = graph
                    .cross_indices
                    .article_to_brand
                    .get(&id)
                    .map(|b| graph.get_str(*b).to_string())
                    .unwrap_or_default();
                let channel = graph
                    .cross_indices
                    .article_to_channel
                    .get(&id)
                    .map(|c| graph.get_str(*c).to_string())
                    .unwrap_or_default();

                let mut obj = serde_json::Map::new();
                obj.insert("article".into(), json!(name));
                obj.insert("l0_name".into(), json!(levels[0]));
                obj.insert("l1_name".into(), json!(levels[1]));
                obj.insert("l2_name".into(), json!(levels[2]));
                obj.insert("l3_name".into(), json!(levels[3]));
                obj.insert("l4_name".into(), json!(levels[4]));
                obj.insert("l5_name".into(), json!(levels[5]));
                obj.insert("brand".into(), json!(brand));
                obj.insert("channel".into(), json!(channel));

                // RCL-resolved columns. When the ruleset isn't supplied
                // (traversal/introspect paths) emit nulls so the schema
                // stays uniform with `columns_for(NodeKind::Article)`.
                let mut store_groups: Value = Value::Null;
                let mut dc_rule: Value = Value::Null;
                let mut min_stock: Value = Value::Null;
                let mut max_stock: Value = Value::Null;
                let mut wos: Value = Value::Null;
                let mut aps: Value = Value::Null;
                if let Some(rules) = ruleset {
                    // Pre-bound pointers (set at build time by
                    // `bind_rule_pointers`) let us do O(1) lookups
                    // against rules.policies / rules.constraints
                    // instead of re-walking priority + specificity per
                    // article. Falls back to explain_* when pointers
                    // are missing (e.g. graph built before RCL service
                    // was up, or article didn't match any rule).
                    use crate::graph::legacy::graph::RuleKind;
                    let mut dc_resolved = false;
                    let mut cn_resolved = false;
                    for ptr in &node.rule_pointers {
                        let rcl_str = graph.get_str(ptr.rcl_code).to_string();
                        let rule_str = graph.get_str(ptr.rule_code).to_string();
                        match ptr.kind {
                            RuleKind::DcPolicy => {
                                if let Some(p) = rules.policies.get(&(rcl_str, rule_str)) {
                                    store_groups = json!(p.default_store_groups.join(", "));
                                    dc_rule = json!(p.dc_store_rule.clone());
                                    dc_resolved = true;
                                }
                            }
                            RuleKind::Constraints => {
                                if let Some(rows) = rules.constraints.get(&(rcl_str, rule_str)) {
                                    if let Some(row) = rows.first() {
                                        min_stock = json!(row.min_stock);
                                        max_stock = json!(row.max_stock);
                                        wos = json!(row.wos);
                                        aps = json!(row.aps);
                                        cn_resolved = true;
                                    }
                                }
                            }
                            RuleKind::Psm => {} // PSM served by psm_resolver, not used here.
                        }
                    }
                    // Fall back to per-row explain when pointers weren't
                    // bound (graph version older than the RuleSet, or
                    // pointer-less node — happens for articles that
                    // didn't match any rule at build time).
                    if !dc_resolved || !cn_resolved {
                        let pc = node
                            .children
                            .first()
                            .map(|&c| graph.get_str(graph.node(c).name))
                            .unwrap_or("");
                        let p = rcl::ProductHierarchy {
                            product_code: pc,
                            l0_name: levels[0],
                            l1_name: levels[1],
                            l2_name: levels[2],
                            l3_name: levels[3],
                            l4_name: levels[4],
                            l5_name: levels[5],
                            brand: &brand,
                        };
                        if !dc_resolved {
                            if let Some(dc) = crate::graph::legacy::explain_dc_policy(rules, &p) {
                                store_groups = json!(dc.policy.default_store_groups.join(", "));
                                dc_rule = json!(dc.policy.dc_store_rule.clone());
                            }
                        }
                        if !cn_resolved {
                            if let Some(c) = crate::graph::legacy::explain_constraints(rules, &p) {
                                if let Some(row) = c.rows.first() {
                                    min_stock = json!(row.min_stock);
                                    max_stock = json!(row.max_stock);
                                    wos = json!(row.wos);
                                    aps = json!(row.aps);
                                }
                            }
                        }
                    }
                }
                obj.insert("store_groups".into(), store_groups);
                obj.insert("dc_rule".into(), dc_rule);
                obj.insert("min_stock".into(), min_stock);
                obj.insert("max_stock".into(), max_stock);
                obj.insert("wos".into(), wos);
                obj.insert("aps".into(), aps);

                if let Some(metrics) = metric_obj().as_object() {
                    for (k, v) in metrics {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                return Some(Value::Object(obj));
            }
            NodeKind::L0
            | NodeKind::L1
            | NodeKind::L2
            | NodeKind::L3
            | NodeKind::L4
            | NodeKind::L5 => {
                let level_label = match kind {
                    NodeKind::L0 => "L0",
                    NodeKind::L1 => "L1",
                    NodeKind::L2 => "L2",
                    NodeKind::L3 => "L3",
                    NodeKind::L4 => "L4",
                    NodeKind::L5 => "L5",
                    _ => "",
                };
                let mut obj = serde_json::Map::new();
                obj.insert("name".into(), json!(name));
                obj.insert("level".into(), json!(level_label));
                obj.insert("child_count".into(), json!(node.children.len() as i64));
                if let Some(metrics) = metric_obj().as_object() {
                    for (k, v) in metrics {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                return Some(Value::Object(obj));
            }
            NodeKind::ProductCode => {
                // Parent is the article node.
                let article_id = node.parent;
                let article_name = graph.get_str(graph.node(article_id).name);
                let mut levels: [&str; 6] = ["", "", "", "", "", ""];
                let mut cur = graph.node(article_id).parent;
                while !cur.is_none() {
                    let p = graph.node(cur);
                    let pname = graph.get_str(p.name);
                    match p.kind {
                        NodeKind::L0 => levels[0] = pname,
                        NodeKind::L1 => levels[1] = pname,
                        NodeKind::L2 => levels[2] = pname,
                        NodeKind::L3 => levels[3] = pname,
                        NodeKind::L4 => levels[4] = pname,
                        NodeKind::L5 => levels[5] = pname,
                        NodeKind::Root => break,
                        _ => {}
                    }
                    cur = p.parent;
                }
                let brand = graph
                    .cross_indices
                    .article_to_brand
                    .get(&article_id)
                    .map(|b| graph.get_str(*b).to_string())
                    .unwrap_or_default();
                let mut obj = serde_json::Map::new();
                obj.insert("product_code".into(), json!(name));
                obj.insert("article".into(), json!(article_name));
                obj.insert("l0_name".into(), json!(levels[0]));
                obj.insert("l1_name".into(), json!(levels[1]));
                obj.insert("l2_name".into(), json!(levels[2]));
                obj.insert("l3_name".into(), json!(levels[3]));
                obj.insert("l4_name".into(), json!(levels[4]));
                obj.insert("l5_name".into(), json!(levels[5]));
                obj.insert("brand".into(), json!(brand));
                return Some(Value::Object(obj));
            }
            NodeKind::StoreCode => {
                let channel = node
                    .parent
                    .0
                    .checked_sub(0)
                    .and_then(|_| Some(graph.node(node.parent)))
                    .map(|p| graph.get_str(p.name).to_string())
                    .unwrap_or_default();
                let sgs: String = graph
                    .cross_indices
                    .store_code_to_sgs
                    .get(&node.name)
                    .map(|v| {
                        v.iter()
                            .map(|s| graph.get_str(*s).to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                let dcs: String = graph
                    .cross_indices
                    .store_code_to_dcs
                    .get(&node.name)
                    .map(|v| {
                        v.iter()
                            .map(|s| graph.get_str(*s).to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                let mut obj = serde_json::Map::new();
                obj.insert("store_code".into(), json!(name));
                obj.insert("channel".into(), json!(channel));
                obj.insert("store_groups".into(), json!(sgs));
                obj.insert("dcs".into(), json!(dcs));
                return Some(Value::Object(obj));
            }
            NodeKind::Channel => {
                let mut obj = serde_json::Map::new();
                obj.insert("channel".into(), json!(name));
                obj.insert("store_count".into(), json!(node.children.len() as i64));
                return Some(Value::Object(obj));
            }
            NodeKind::Root => {
                let mut obj = serde_json::Map::new();
                obj.insert("(root)".into(), json!(""));
                if let Some(metrics) = metric_obj().as_object() {
                    for (k, v) in metrics {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                return Some(Value::Object(obj));
            }
        }
    }
    #[allow(unreachable_code)]
    None
}

/// Apply LIMIT / OFFSET / ORDER BY in-process. The graph is in-memory
/// so SQL semantics aren't available; we implement the subset the
/// DataView read path uses.
pub fn paginate(
    rows: Vec<Value>,
    sort_col: Option<&str>,
    sort_dir: Option<&str>,
    limit: i64,
    offset: i64,
) -> Vec<Value> {
    let mut out = rows;
    if let Some(col) = sort_col.filter(|s| !s.is_empty()) {
        let desc = sort_dir.map(|s| s.eq_ignore_ascii_case("DESC")).unwrap_or(false);
        out.sort_by(|a, b| {
            let av = a.get(col);
            let bv = b.get(col);
            let ord = compare_values(av, bv);
            if desc { ord.reverse() } else { ord }
        });
    }
    let off = offset.max(0) as usize;
    let limit = if limit > 0 { limit as usize } else { out.len() };
    out.into_iter().skip(off).take(limit).collect()
}

/// Map a metric column name to its [`MetricKind`]. Returns `None` for
/// non-metric columns (those still go through the full project + sort
/// path).
fn metric_kind_for(col: &str) -> Option<MetricKind> {
    match col {
        "oh" => Some(MetricKind::Oh),
        "oo" => Some(MetricKind::Oo),
        "it" => Some(MetricKind::It),
        "reserve_quantity" => Some(MetricKind::ReserveQuantity),
        "allocated_units" => Some(MetricKind::AllocatedUnits),
        "lw_units" => Some(MetricKind::LwUnits),
        "lw_revenue" => Some(MetricKind::LwRevenue),
        "lw_margin" => Some(MetricKind::LwMargin),
        _ => None,
    }
}

/// Fast-path projection for paginated reads. Sorts node IDs by their
/// metric/name on the graph itself (no JSON allocations), paginates,
/// then projects only the page. Falls back to the full
/// [`project_rows`] path when:
///   - `sort_col` is empty AND `limit` is non-positive (return all)
///   - `sort_col` references a column not handled by this fast path
///     (string columns other than the node's primary name)
///
/// Total cost for sorted paginated reads:
///   - Sort node ids: O(N log N) on cheap primitives. ~5 ms for 48 K.
///   - Project page only: O(limit). ~1 ms for 100 rows.
///
/// Returns `(rows, total)` where `total` is the count BEFORE pagination
/// (so the UI can render "n of N").
pub fn project_page(
    graph: &ArticleGraph,
    kind: NodeKind,
    sort_col: Option<&str>,
    sort_dir: Option<&str>,
    limit: i64,
    offset: i64,
    // Optional candidate set from `cross_filter::resolver::apply_filters`.
    // When `Some`, restricts `ids` to that intersection before sort/page.
    // Only meaningful when `kind == NodeKind::Article` — for any other
    // kind the set is ignored (the cross-filter resolver only produces
    // article NodeIds).
    candidate_articles: Option<&std::collections::BTreeSet<NodeId>>,
    // Optional rcl ruleset used by `project_single` to populate the
    // article-level rcl-resolved columns (store_groups / dc_rule /
    // constraint values). `None` leaves those columns null.
    ruleset: Option<&rcl::RuleSet>,
) -> (Vec<Value>, i64) {
    let mut ids: Vec<NodeId> = graph.by_kind[kind.idx()].values().copied().collect();
    // Apply the cross-filter candidate set up front so both `total` and
    // the page reflect the filtered cardinality.
    //
    // For ARTICLE: direct membership check.
    // For L0..L5: a hierarchy node is "alive" if its subtree contains
    //   at least one candidate. Walk parents from each candidate article
    //   and mark them alive — same alive-ancestor pattern the tree view
    //   uses for traversal.
    // For other kinds (PRODUCT_CODE / CHANNEL / STORE_CODE): no filter
    //   applied (the cross-filter resolver is article-side only).
    if let Some(set) = candidate_articles {
        match kind {
            NodeKind::Article => ids.retain(|id| set.contains(id)),
            NodeKind::L0
            | NodeKind::L1
            | NodeKind::L2
            | NodeKind::L3
            | NodeKind::L4
            | NodeKind::L5 => {
                let mut alive: std::collections::HashSet<NodeId> =
                    std::collections::HashSet::new();
                for &article_id in set {
                    let mut cur = graph.node(article_id).parent;
                    while !cur.is_none() && cur != graph.root {
                        if !alive.insert(cur) { break; }
                        cur = graph.node(cur).parent;
                    }
                }
                ids.retain(|id| alive.contains(id));
            }
            _ => {} // CHANNEL / STORE_CODE / ProductCode — leave unfiltered
        }
    }
    // Filter empty-name placeholder nodes (same predicate as
    // project_single's early-return).
    ids.retain(|&id| {
        let n = graph.node(id);
        let name = graph.get_str(n.name);
        !(name.is_empty() && !matches!(kind, NodeKind::Root))
    });
    let total = ids.len() as i64;
    let desc = sort_dir
        .map(|s| s.eq_ignore_ascii_case("DESC"))
        .unwrap_or(false);

    // ── Sort node IDs in place using only graph fields (no JSON).
    let t_sort = std::time::Instant::now();
    let sort_branch: &'static str;
    if let Some(col) = sort_col.filter(|s| !s.is_empty()) {
        if let Some(metric) = metric_kind_for(col) {
            sort_branch = "metric";
            let idx = metric.idx();
            ids.sort_by(|a, b| {
                let av = graph.node(*a).metrics[idx];
                let bv = graph.node(*b).metrics[idx];
                let ord = av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal);
                if desc { ord.reverse() } else { ord }
            });
        } else if col == "name" || matches_node_primary_name(kind, col) {
            sort_branch = "primary_name";
            ids.sort_by(|a, b| {
                let av = graph.get_str(graph.node(*a).name);
                let bv = graph.get_str(graph.node(*b).name);
                let ord = av.cmp(bv);
                if desc { ord.reverse() } else { ord }
            });
        } else if matches!(kind, NodeKind::Article) && article_string_sort_col(col).is_some() {
            sort_branch = "article_string_fast";
            // Fast path for article-side string columns the user can
            // sort by without forcing a full projection. Materialize
            // (sort_key, NodeId) once (O(N) graph lookups) then sort
            // (O(N log N) cheap str comparisons). For 48 K Bealls
            // articles this is ~50 ms total — vs. the previous slow
            // path's 200-400 ms full-projection-then-sort.
            //
            // Covers `brand` / `channel` (O(1) cross_indices lookup)
            // and `l0_name`..`l5_name` (parent walk up the spine).
            let kind_marker = article_string_sort_col(col).unwrap();
            let mut keyed: Vec<(&str, NodeId)> = ids
                .iter()
                .map(|&id| (article_sort_key(graph, id, kind_marker), id))
                .collect();
            keyed.sort_by(|a, b| {
                let ord = a.0.cmp(b.0);
                if desc { ord.reverse() } else { ord }
            });
            ids = keyed.into_iter().map(|(_, id)| id).collect();
        } else if matches!(kind, NodeKind::Article)
            && ruleset.is_some()
            && rcl_string_sort_col(col).is_some()
        {
            sort_branch = "rcl_string_fast";
            // RCL-resolved string columns (store_groups, dc_rule).
            // Reads each article's pre-bound `rule_pointers`, looks up
            // the policy in the ruleset, projects the column value to
            // a string. Per-article cost ≈ one HashMap probe; ~50 ms
            // for 48 K vs. ~300 ms for the explain-per-row slow path.
            let rules = ruleset.unwrap();
            let field = rcl_string_sort_col(col).unwrap();
            let mut keyed: Vec<(String, NodeId)> = ids
                .iter()
                .map(|&id| (rcl_string_sort_key(graph, rules, id, field), id))
                .collect();
            keyed.sort_by(|a, b| {
                let ord = a.0.cmp(&b.0);
                if desc { ord.reverse() } else { ord }
            });
            ids = keyed.into_iter().map(|(_, id)| id).collect();
        } else if matches!(kind, NodeKind::Article)
            && ruleset.is_some()
            && rcl_numeric_sort_col(col).is_some()
        {
            sort_branch = "rcl_numeric_fast";
            // RCL-resolved numeric columns (min_stock, max_stock, wos, aps).
            // Same pre-bound-pointer pattern, projecting to f64.
            let rules = ruleset.unwrap();
            let field = rcl_numeric_sort_col(col).unwrap();
            let mut keyed: Vec<(f64, NodeId)> = ids
                .iter()
                .map(|&id| (rcl_numeric_sort_key(graph, rules, id, field), id))
                .collect();
            keyed.sort_by(|a, b| {
                let ord = a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal);
                if desc { ord.reverse() } else { ord }
            });
            ids = keyed.into_iter().map(|(_, id)| id).collect();
        } else {
            // Fall back: project everything, then sort/paginate by JSON
            // value. With Fix B in place this should be rare — only
            // fires for columns we don't recognize (typo / future
            // schema additions / non-Article kinds with non-metric
            // string columns).
            let all = project_rows(graph, kind, ruleset);
            let paged = paginate(all, sort_col, sort_dir, limit, offset);
            tracing::warn!(
                target: "live_view_timing",
                kind = "article_graph_slow_sort",
                sort_col = col,
                rows_projected = total,
                elapsed_ms = t_sort.elapsed().as_millis() as u64,
                "fell into project_rows slow path",
            );
            return (paged, total);
        }
    } else {
        sort_branch = "no_sort";
    }

    let sort_ms = t_sort.elapsed().as_millis() as u64;

    // ── Page slice + project only the visible rows.
    let t_proj = std::time::Instant::now();
    let off = offset.max(0) as usize;
    let lim = if limit > 0 { limit as usize } else { ids.len() };
    let page_ids: Vec<NodeId> = ids.into_iter().skip(off).take(lim).collect();
    let rows: Vec<Value> = page_ids
        .par_iter()
        .filter_map(|&id| project_single(graph, kind, id, ruleset))
        .collect();
    let project_page_ms = t_proj.elapsed().as_millis() as u64;
    tracing::debug!(
        target: "live_view_timing",
        sort_branch,
        sort_ms,
        project_page_ms,
        page_size = rows.len(),
        total,
        "project_page breakdown",
    );
    (rows, total)
}

/// Article-side string columns the fast sort path supports without
/// materializing full row JSON. Returns the marker the resolver uses
/// to look up the value on the node (`brand` / `channel` via
/// cross_indices; `l0`..`l5` via parent walk). Returns `None` for
/// columns the fast path doesn't cover (RCL-resolved et al.).
fn article_string_sort_col(col: &str) -> Option<ArticleSortField> {
    match col {
        "brand" => Some(ArticleSortField::Brand),
        "channel" => Some(ArticleSortField::Channel),
        "l0_name" => Some(ArticleSortField::Level(0)),
        "l1_name" => Some(ArticleSortField::Level(1)),
        "l2_name" => Some(ArticleSortField::Level(2)),
        "l3_name" => Some(ArticleSortField::Level(3)),
        "l4_name" => Some(ArticleSortField::Level(4)),
        "l5_name" => Some(ArticleSortField::Level(5)),
        _ => None,
    }
}

#[derive(Copy, Clone)]
enum ArticleSortField {
    Brand,
    Channel,
    Level(u8),
}

/// Resolve the sort key for a single Article node and field. Returns
/// an empty string when the field is missing on this node — the
/// natural-sort behavior the slow path produces too.
fn article_sort_key(graph: &ArticleGraph, id: NodeId, field: ArticleSortField) -> &str {
    match field {
        ArticleSortField::Brand => graph
            .cross_indices
            .article_to_brand
            .get(&id)
            .map(|s| graph.get_str(*s))
            .unwrap_or(""),
        ArticleSortField::Channel => graph
            .cross_indices
            .article_to_channel
            .get(&id)
            .map(|s| graph.get_str(*s))
            .unwrap_or(""),
        ArticleSortField::Level(level) => {
            let want = match level {
                0 => NodeKind::L0,
                1 => NodeKind::L1,
                2 => NodeKind::L2,
                3 => NodeKind::L3,
                4 => NodeKind::L4,
                5 => NodeKind::L5,
                _ => return "",
            };
            let mut cur = graph.node(id).parent;
            while !cur.is_none() {
                let n = graph.node(cur);
                if n.kind == want {
                    return graph.get_str(n.name);
                }
                if matches!(n.kind, NodeKind::Root) {
                    break;
                }
                cur = n.parent;
            }
            ""
        }
    }
}

/// RCL-resolved string columns the fast sort path can cover via
/// pre-bound `rule_pointers`. Returns the field tag the resolver
/// reads from the ruleset.
fn rcl_string_sort_col(col: &str) -> Option<RclStringField> {
    match col {
        "store_groups" => Some(RclStringField::StoreGroups),
        "dc_rule" => Some(RclStringField::DcRule),
        _ => None,
    }
}

/// RCL-resolved numeric columns covered by the fast sort path.
fn rcl_numeric_sort_col(col: &str) -> Option<RclNumericField> {
    match col {
        "min_stock" => Some(RclNumericField::MinStock),
        "max_stock" => Some(RclNumericField::MaxStock),
        "wos" => Some(RclNumericField::Wos),
        "aps" => Some(RclNumericField::Aps),
        _ => None,
    }
}

#[derive(Copy, Clone)]
enum RclStringField {
    StoreGroups,
    DcRule,
}

#[derive(Copy, Clone)]
enum RclNumericField {
    MinStock,
    MaxStock,
    Wos,
    Aps,
}

fn rcl_string_sort_key(
    graph: &ArticleGraph,
    rules: &rcl::RuleSet,
    id: NodeId,
    field: RclStringField,
) -> String {
    use crate::graph::legacy::graph::RuleKind;
    for ptr in &graph.node(id).rule_pointers {
        if !matches!(ptr.kind, RuleKind::DcPolicy) {
            continue;
        }
        let key = (
            graph.get_str(ptr.rcl_code).to_string(),
            graph.get_str(ptr.rule_code).to_string(),
        );
        if let Some(policy) = rules.policies.get(&key) {
            return match field {
                RclStringField::StoreGroups => policy.default_store_groups.join(", "),
                RclStringField::DcRule => policy.dc_store_rule.clone(),
            };
        }
    }
    String::new()
}

fn rcl_numeric_sort_key(
    graph: &ArticleGraph,
    rules: &rcl::RuleSet,
    id: NodeId,
    field: RclNumericField,
) -> f64 {
    use crate::graph::legacy::graph::RuleKind;
    for ptr in &graph.node(id).rule_pointers {
        if !matches!(ptr.kind, RuleKind::Constraints) {
            continue;
        }
        let key = (
            graph.get_str(ptr.rcl_code).to_string(),
            graph.get_str(ptr.rule_code).to_string(),
        );
        if let Some(rows) = rules.constraints.get(&key) {
            if let Some(row) = rows.first() {
                return match field {
                    RclNumericField::MinStock => row.min_stock as f64,
                    RclNumericField::MaxStock => row.max_stock as f64,
                    RclNumericField::Wos => row.wos as f64,
                    RclNumericField::Aps => row.aps as f64,
                };
            }
        }
    }
    f64::NAN
}

/// Whether `col` matches the "primary name" column for `kind`
/// (e.g. "article" for Article nodes, "product_code" for ProductCode,
/// etc.). Used by `project_page` to know when sorting on the column
/// can short-circuit to the node's interned name.
fn matches_node_primary_name(kind: NodeKind, col: &str) -> bool {
    match kind {
        NodeKind::Article => col == "article",
        NodeKind::ProductCode => col == "product_code",
        NodeKind::StoreCode => col == "store_code",
        NodeKind::Channel => col == "channel",
        _ => false,
    }
}

fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, _) => Ordering::Less,
        (_, None) => Ordering::Greater,
        (Some(av), Some(bv)) => match (av, bv) {
            (Value::Number(an), Value::Number(bn)) => an
                .as_f64()
                .partial_cmp(&bn.as_f64())
                .unwrap_or(Ordering::Equal),
            _ => av.to_string().cmp(&bv.to_string()),
        },
    }
}
