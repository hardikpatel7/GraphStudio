//! Exception-rule predicates + the alive-set used by tree-view pruning.
//!
//! Five rules ported from v1's `article_graph::exception`:
//! `stockout` / `reserve_gap` (pure scalar predicates) and
//! `overstock` / `below_min` / `no_eligible_stores` (require the live
//! `rcl::RuleSet` + PSM resolver). The latter three only fire when
//! `flag_node` is called with a `Some(&RuleSet)` and the graph's
//! `psm.is_ready()` is true.
//!
//! ## Metric resolution
//!
//! Rules read metrics by *bare name* (`"oh"`, `"lw_units"`), not by
//! `<source>.<name>` — v1's wire-stable rule semantics expect those
//! exact names. `MetricLookup` scans the registry once per request,
//! building a `name → slot_index` map. First registration wins on
//! collisions; bealls has no name overlaps across sources so this is
//! a non-issue in practice.

use rayon::prelude::*;
use smallvec::SmallVec;
use std::collections::{BTreeSet, HashMap, HashSet};

use super::cross_filter::{EntitledSet, FilterCriterion, apply_filters};
use super::graph::{Graph, KindId, MetricValue, NodeId, StrId};
use super::rcl::{explain_constraints, owned_hierarchy_for};

/// Threshold over `max_stock` that triggers the Overstock rule.
/// Hardcoded for parity with v1's `OVERSTOCK_FACTOR`; if operators
/// want a knob, parameterize per-request later.
const OVERSTOCK_FACTOR: f64 = 1.5;

/// Wire-stable exception rule label. v1's wire strings carry over so
/// existing UI clients work against the v2 endpoint unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// `oh ≤ 0 AND lw_units > 0` — the canonical "we sold last week
    /// but have nothing on hand" stockout.
    Stockout,
    /// `allocated_units > oh + oo + it` — the allocation exceeds
    /// what the article can possibly fulfill, i.e. reservations
    /// outpace supply.
    ReserveGap,
    /// RCL-dependent — not fired in Phase 3-bis. Kept in the enum so
    /// `from_wire` round-trips for clients that already send the
    /// label; effectively a no-op until RCL integration lands.
    Overstock,
    /// RCL-dependent — see `Overstock`.
    BelowMin,
    /// RCL-dependent — see `Overstock`.
    NoEligibleStores,
}

impl Rule {
    pub const ALL: [Rule; 5] = [
        Rule::Stockout,
        Rule::Overstock,
        Rule::BelowMin,
        Rule::ReserveGap,
        Rule::NoEligibleStores,
    ];

    pub fn as_wire(self) -> &'static str {
        match self {
            Rule::Stockout => "stockout",
            Rule::Overstock => "overstock",
            Rule::BelowMin => "below_min",
            Rule::ReserveGap => "reserve_gap",
            Rule::NoEligibleStores => "no_eligible_stores",
        }
    }

    pub fn from_wire(s: &str) -> Option<Rule> {
        match s {
            "stockout" => Some(Rule::Stockout),
            "overstock" => Some(Rule::Overstock),
            "below_min" => Some(Rule::BelowMin),
            "reserve_gap" => Some(Rule::ReserveGap),
            "no_eligible_stores" => Some(Rule::NoEligibleStores),
            _ => None,
        }
    }

    /// Whether this rule's predicate needs the live `rcl::RuleSet`.
    /// Phase 3-bis treats `true` as "never fires"; Phase 4-B+ will
    /// plumb the ruleset in and replace this gate with the actual
    /// predicate.
    pub fn requires_rcl(self) -> bool {
        matches!(self, Rule::Overstock | Rule::BelowMin | Rule::NoEligibleStores)
    }
}

/// `metric_name → slot_index` lookup. Built once per request; the
/// per-rule predicates do constant-time index reads against
/// `Node.metrics`. Names from the registry are stored as-is (lowercase
/// "oh", "lw_units", …) since v1's wire vocabulary matches.
pub struct MetricLookup {
    slots: HashMap<String, usize>,
}

impl MetricLookup {
    /// Walk the primary-metric registry once, recording the first
    /// slot index for each metric name. Collisions across sources
    /// don't happen in bealls; if they ever do, the first
    /// registration wins — matches the "first wins" pattern the
    /// rest of the codebase uses for ambiguous lookups.
    pub fn build(graph: &Graph) -> Self {
        let primary_ids = graph.metrics.primary_metric_ids();
        let mut slots = HashMap::new();
        for (slot, mid) in primary_ids.iter().enumerate() {
            let meta = graph.metrics.get(*mid);
            slots.entry(meta.name.clone()).or_insert(slot);
        }
        Self { slots }
    }

    /// Read the named metric off `node_id`. Returns `None` when the
    /// metric isn't registered or the slot isn't a `Scalar`
    /// (collection rollups can't be coerced to f64 here — the rule
    /// predicates that need them would need a richer accessor).
    pub fn get(&self, graph: &Graph, node_id: NodeId, metric_name: &str) -> Option<f64> {
        let slot = *self.slots.get(metric_name)?;
        match graph.node(node_id).metrics.get(slot) {
            Some(MetricValue::Scalar(f)) => Some(*f),
            _ => None,
        }
    }
}

/// Evaluate every rule on `node_id`. Returns the firing set — a
/// `SmallVec` because most nodes fire 0–1 rules and we don't want a
/// heap allocation per row.
///
/// Pass `Some(&rcl::RuleSet)` to enable Overstock / BelowMin
/// (require constraint resolution) and to scope NoEligibleStores
/// (PSM resolution; still gated on `graph.psm.is_ready()`). Pass
/// `None` to skip those three — useful when the RCL service isn't
/// running yet or when callers explicitly want only the cheap rules.
pub fn flag_node(
    graph: &Graph,
    lookup: &MetricLookup,
    node_id: NodeId,
    ruleset: Option<&rcl::RuleSet>,
) -> SmallVec<[Rule; 4]> {
    let mut flags: SmallVec<[Rule; 4]> = SmallVec::new();

    // Read all five inputs once. Missing → 0.0 (matches v1; absent
    // inventory rows on an article shouldn't crash the rule pass).
    let oh = lookup.get(graph, node_id, "oh").unwrap_or(0.0);
    let oo = lookup.get(graph, node_id, "oo").unwrap_or(0.0);
    let it = lookup.get(graph, node_id, "it").unwrap_or(0.0);
    let lw_units = lookup.get(graph, node_id, "lw_units").unwrap_or(0.0);
    let allocated = lookup.get(graph, node_id, "allocated_units").unwrap_or(0.0);

    if oh <= 0.0 && lw_units > 0.0 {
        flags.push(Rule::Stockout);
    }
    if allocated > oh + oo + it {
        flags.push(Rule::ReserveGap);
    }

    // RCL-dependent rules — only fire when the live RuleSet is
    // present. We build the OwnedHierarchy on demand so the
    // RCL-less path doesn't pay the ancestor walk + brand cross-edge
    // lookup; on the RCL path it's the same per-article cost v1 paid.
    let needs_rcl = ruleset.is_some() || graph.psm.is_ready();
    if !needs_rcl {
        return flags;
    }
    let hierarchy = owned_hierarchy_for(graph, node_id);

    if let Some(rules) = ruleset {
        if let Some(c) = explain_constraints(rules, &hierarchy.borrow()) {
            if let Some(row) = c.rows.first() {
                let min_stock = row.min_stock;
                let max_stock = row.max_stock;
                if max_stock > 0.0 && oh > max_stock * OVERSTOCK_FACTOR {
                    flags.push(Rule::Overstock);
                }
                if min_stock > 0.0 && oh > 0.0 && oh < min_stock {
                    flags.push(Rule::BelowMin);
                }
            }
        }
    }

    if graph.psm.is_ready() {
        let explain = graph.psm.explain(|field| match field {
            "l0_name" => hierarchy.l0_name.clone(),
            "l1_name" => hierarchy.l1_name.clone(),
            "l2_name" => hierarchy.l2_name.clone(),
            "l3_name" => hierarchy.l3_name.clone(),
            "l4_name" => hierarchy.l4_name.clone(),
            "l5_name" => hierarchy.l5_name.clone(),
            "brand" => hierarchy.brand.clone(),
            "product_code" => hierarchy.product_code.clone(),
            _ => String::new(),
        });
        if explain.is_none() {
            flags.push(Rule::NoEligibleStores);
        }
    }

    flags
}

/// Parallel pass: per-rule count over `candidates` (or every node of
/// `target_kind` when `None`). The returned `(total, by_rule)` shape
/// mirrors v1's `ExceptionCounts` so existing UI surfaces work.
///
/// `ruleset = None` skips the three RCL-dependent rules (Overstock,
/// BelowMin, NoEligibleStores). Pass the live `Arc<rcl::RuleSet>`
/// to enable them — same gating contract as `flag_node`.
pub fn count_exceptions(
    graph: &Graph,
    target_kind: KindId,
    candidates: Option<&BTreeSet<NodeId>>,
    ruleset: Option<&rcl::RuleSet>,
) -> ExceptionCounts {
    let lookup = MetricLookup::build(graph);
    let empty = StrId(0);
    let ids: Vec<NodeId> = match candidates {
        Some(set) => set.iter().copied().collect(),
        None => (0..graph.node_count())
            .map(|i| NodeId(i as u32))
            .filter(|id| {
                let n = graph.node(*id);
                n.kind == target_kind && n.name != empty
            })
            .collect(),
    };
    let total = ids.len();
    let by_rule = ids
        .par_iter()
        .fold(
            HashMap::<Rule, usize>::new,
            |mut acc, &id| {
                for r in flag_node(graph, &lookup, id, ruleset) {
                    *acc.entry(r).or_insert(0) += 1;
                }
                acc
            },
        )
        .reduce(
            HashMap::<Rule, usize>::new,
            |mut a, b| {
                for (k, v) in b {
                    *a.entry(k).or_insert(0) += v;
                }
                a
            },
        );
    ExceptionCounts { total, by_rule }
}

#[derive(Debug, Clone)]
pub struct ExceptionCounts {
    pub total: usize,
    pub by_rule: HashMap<Rule, usize>,
}

/// Alive-set: AND-compose cross-filter narrowing with exception-rule
/// narrowing, then expand the result to include every spine ancestor
/// up to (excluding) root. Tree views consume this to prune branches
/// with no alive descendants.
///
/// Returns `None` when neither filters nor rules narrow — the caller
/// treats this as "no narrowing applied, every node passes" rather
/// than building an unused HashSet (matches v1).
pub fn alive_set(
    graph: &Graph,
    target_kind: KindId,
    filters: &[FilterCriterion],
    rules: &[Rule],
    entitled: Option<&EntitledSet>,
    ruleset: Option<&rcl::RuleSet>,
) -> Option<HashSet<NodeId>> {
    if filters.is_empty() && rules.is_empty() {
        return None;
    }

    // Step 1: cross-filter narrowing.
    let candidates: BTreeSet<NodeId> = if filters.is_empty() {
        // No filters → seed with every target-kind node (matching
        // apply_filters' own seed behavior so the empty-name
        // placeholder is excluded uniformly).
        let empty = StrId(0);
        (0..graph.node_count())
            .map(|i| NodeId(i as u32))
            .filter(|id| {
                let n = graph.node(*id);
                n.kind == target_kind && n.name != empty
            })
            .collect()
    } else {
        apply_filters(graph, target_kind, filters, entitled)
    };

    // Step 2: rule narrowing — only fire if the caller asked.
    let candidates: Vec<NodeId> = if rules.is_empty() {
        candidates.into_iter().collect()
    } else {
        let lookup = MetricLookup::build(graph);
        candidates
            .into_iter()
            .filter(|id| {
                let flags = flag_node(graph, &lookup, *id, ruleset);
                rules.iter().any(|r| flags.contains(r))
            })
            .collect()
    };

    // Step 3: ancestor expansion. Each surviving target node
    // contributes itself + every spine ancestor up to (but not
    // including) root, so tree views can prune subtrees whose
    // descendants are all dead.
    let mut alive: HashSet<NodeId> = HashSet::with_capacity(candidates.len() * 2);
    for id in candidates {
        alive.insert(id);
        let mut cur = graph.node(id).parent;
        while !cur.is_none() && cur != graph.root {
            if !alive.insert(cur) {
                break;
            }
            cur = graph.node(cur).parent;
        }
    }
    Some(alive)
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
    fn ti(i: i64) -> CellValue { CellValue::Int(i) }

    /// 2-level hierarchy (l0 → article) + scalar metrics matching the
    /// rule predicates' inputs (oh, oo, it, lw_units, allocated_units).
    fn fixture() -> Graph {
        let toml = r#"
id = "g"
display_name = "G"

[[sources]]
alias = "products"
table = "products_tbl"
attaches_at = "article"

[[sources]]
alias = "inventory"
table = "inv_tbl"
attaches_at = "article"

[[sources]]
alias = "txs"
table = "txs_tbl"
attaches_at = "article"

[hierarchy.product]
source = "products"

[hierarchy.product.l0]
column = "l0"

[hierarchy.product.article]
column = "article"

[metrics.inventory]
oh               = { rollup = "sum" }
oo               = { rollup = "sum" }
it               = { rollup = "sum" }
allocated_units  = { rollup = "sum" }

[metrics.txs]
lw_units = { rollup = "sum" }
"#;
        let spec = from_toml(toml).unwrap();
        let mut tables = HashMap::new();
        tables.insert("products_tbl".to_string(), MockTable {
            columns: vec!["l0".into(), "article".into()],
            rows: vec![
                vec![ts("L0_A"), ts("A1")],
                vec![ts("L0_A"), ts("A2")],
                vec![ts("L0_B"), ts("A3")],
            ],
        });
        // A1: oh=0, oo=0, it=0, allocated=0 → no rules.
        // A2: oh=0, oo=0, it=0, allocated=0, lw_units=5 → Stockout.
        // A3: oh=10, oo=0, it=0, allocated=20 → ReserveGap (allocated > sum).
        tables.insert("inv_tbl".to_string(), MockTable {
            columns: vec!["article".into(), "oh".into(), "oo".into(), "it".into(), "allocated_units".into()],
            rows: vec![
                vec![ts("A1"), ti(5),  ti(0), ti(0), ti(0)],
                vec![ts("A2"), ti(0),  ti(0), ti(0), ti(0)],
                vec![ts("A3"), ti(10), ti(0), ti(0), ti(20)],
            ],
        });
        tables.insert("txs_tbl".to_string(), MockTable {
            columns: vec!["article".into(), "lw_units".into()],
            rows: vec![
                vec![ts("A1"), ti(3)],
                vec![ts("A2"), ti(5)],
                vec![ts("A3"), ti(2)],
            ],
        });
        let reader = MockReader { tables };
        let (g, _) = build_graph(&spec, &reader, 1).expect("build");
        g
    }

    #[test]
    fn flag_node_detects_stockout_and_reserve_gap() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        let a1 = g.find_by_name(art_kind, "A1").unwrap();
        let a2 = g.find_by_name(art_kind, "A2").unwrap();
        let a3 = g.find_by_name(art_kind, "A3").unwrap();
        let lookup = MetricLookup::build(&g);

        // A1: oh=5, lw_units=3 → no stockout (oh>0); allocated=0 ≤ 5 → no reserve_gap.
        assert!(flag_node(&g, &lookup, a1, None).is_empty());
        // A2: oh=0, lw_units=5 → Stockout.
        assert_eq!(flag_node(&g, &lookup, a2, None).as_slice(), &[Rule::Stockout]);
        // A3: allocated=20 > oh+oo+it=10 → ReserveGap.
        assert_eq!(flag_node(&g, &lookup, a3, None).as_slice(), &[Rule::ReserveGap]);
    }

    #[test]
    fn count_exceptions_tallies_per_rule() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        let counts = count_exceptions(&g, art_kind, None, None);
        assert_eq!(counts.total, 3);
        assert_eq!(*counts.by_rule.get(&Rule::Stockout).unwrap_or(&0), 1);
        assert_eq!(*counts.by_rule.get(&Rule::ReserveGap).unwrap_or(&0), 1);
        assert!(!counts.by_rule.contains_key(&Rule::Overstock));
    }

    #[test]
    fn alive_set_returns_none_when_no_narrowing() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        assert!(alive_set(&g, art_kind, &[], &[], None, None).is_none());
    }

    #[test]
    fn alive_set_includes_ancestors() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        // Stockout narrows to A2; alive should include A2 + L0_A.
        let alive = alive_set(&g, art_kind, &[], &[Rule::Stockout], None, None).expect("Some");
        let a2 = g.find_by_name(art_kind, "A2").unwrap();
        let l0_a = g.find_by_name(l0_kind, "L0_A").unwrap();
        assert!(alive.contains(&a2));
        assert!(alive.contains(&l0_a));
        // L0_B has no stockout descendants — must be excluded.
        let l0_b = g.find_by_name(l0_kind, "L0_B").unwrap();
        assert!(!alive.contains(&l0_b));
    }

    #[test]
    fn alive_set_intersects_filters_and_rules() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        // Filter to L0_A (= {A1, A2}) AND rule=ReserveGap.
        // Neither A1 nor A2 fires ReserveGap → empty alive set
        // (besides the empty-target case the function still returns
        // Some, just an empty HashSet — let's verify that).
        let f = FilterCriterion {
            attribute_name: "l0".into(),
            values: vec!["L0_A".into()],
            operator: super::super::cross_filter::FilterOperator::In,
        };
        let alive = alive_set(&g, art_kind, &[f], &[Rule::ReserveGap], None, None).expect("Some");
        // The intersection (l0=L0_A) ∩ (reserve_gap) is empty.
        assert!(alive.is_empty());
    }

    #[test]
    fn rcl_rules_never_fire_in_phase_3_bis() {
        let g = fixture();
        let art_kind = g.kinds.id_of("article").unwrap();
        // Even with every article matching by filter, the
        // RCL-dependent rules return zero count — Decision 35 defers
        // them and `flag_node` doesn't fire them yet.
        let counts = count_exceptions(&g, art_kind, None, None);
        assert_eq!(*counts.by_rule.get(&Rule::Overstock).unwrap_or(&0), 0);
        assert_eq!(*counts.by_rule.get(&Rule::BelowMin).unwrap_or(&0), 0);
        assert_eq!(*counts.by_rule.get(&Rule::NoEligibleStores).unwrap_or(&0), 0);
    }
}
