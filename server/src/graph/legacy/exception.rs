//! Exception view rules — Phase 1.
//!
//! Pure predicates over an article node + its rcl-resolved data. Each
//! rule answers "is this article in trouble?" using only data that's
//! already cheap to read on V8: the graph's per-node metrics, plus
//! `explain_dc_policy` / `explain_constraints` / `psm.explain` for
//! anything sourced from the rcl ruleset.
//!
//! The functions here are read-only and do not mutate the graph.
//! `count_exceptions` runs an O(N) parallel pass over articles and
//! returns one count per rule; `list_exceptions` returns the article
//! NodeId list (paginated) for any selected rule(s).

use std::collections::{BTreeSet, HashMap};

use rayon::prelude::*;
use smallvec::SmallVec;

use crate::graph::legacy::{ArticleGraph, MetricKind, NodeId, NodeKind};

/// All rules supported in Phase 1. Names are wire-stable — clients send
/// lowercase strings (`"stockout"`, `"overstock"`, …) and we round-trip
/// through `Rule::from_wire` / `Rule::as_wire`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    Stockout,
    Overstock,
    BelowMin,
    ReserveGap,
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
}

/// Threshold over `max_stock` that triggers the Overstock rule.
/// Hard-coded for v1; parameterizable later if operators want a knob.
const OVERSTOCK_FACTOR: f64 = 1.5;

/// Evaluate every rule on a single article and return which fired.
/// Uses a SmallVec because most rows fire 0 or 1 rules — no heap alloc
/// in the common case. The `min_stock` / `max_stock` come from
/// `explain_constraints` (first row) when present; absent constraints
/// just disable the constraint-dependent rules for that article.
pub fn flag_article(
    graph: &ArticleGraph,
    ruleset: Option<&rcl::RuleSet>,
    article_id: NodeId,
) -> SmallVec<[Rule; 4]> {
    let node = graph.node(article_id);
    if !matches!(node.kind, NodeKind::Article) {
        return SmallVec::new();
    }
    let oh = node.metrics[MetricKind::Oh.idx()];
    let oo = node.metrics[MetricKind::Oo.idx()];
    let it = node.metrics[MetricKind::It.idx()];
    let allocated = node.metrics[MetricKind::AllocatedUnits.idx()];
    let lw_units = node.metrics[MetricKind::LwUnits.idx()];

    let mut flags: SmallVec<[Rule; 4]> = SmallVec::new();

    // Cheap rules first — they don't need the ruleset.
    if oh <= 0.0 && lw_units > 0.0 {
        flags.push(Rule::Stockout);
    }
    if allocated > oh + oo + it {
        flags.push(Rule::ReserveGap);
    }

    // Build the article's hierarchy once. Used by both the constraint
    // resolver (when ruleset is present) and the PSM resolver — both
    // need the same field set.
    let hier = if ruleset.is_some() || graph.psm.is_ready() {
        Some(article_hierarchy(graph, article_id))
    } else {
        None
    };
    if let (Some(rules), Some(h)) = (ruleset, hier.as_ref()) {
        if let Some(c) = crate::graph::legacy::explain_constraints(rules, &h.borrow()) {
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

    // PSM eligibility — on-the-fly resolver, takes a field accessor.
    if let Some(h) = hier.as_ref() {
        if graph.psm.is_ready() && graph.psm.explain(|f| field_for(h, f)).is_none() {
            flags.push(Rule::NoEligibleStores);
        }
    }

    flags
}

/// O(N) parallel pass: returns count per rule across the candidate set.
/// `candidates = None` runs over every article. The candidate set
/// usually comes from `cross_filter::resolver::apply_filters` so the
/// chip counts respect the active filter dropdowns.
pub fn count_exceptions(
    graph: &ArticleGraph,
    ruleset: Option<&rcl::RuleSet>,
    candidates: Option<&BTreeSet<NodeId>>,
) -> ExceptionCounts {
    let ids: Vec<NodeId> = match candidates {
        Some(set) => set.iter().copied().collect(),
        None => graph.by_kind[NodeKind::Article.idx()]
            .values()
            .copied()
            .collect(),
    };
    let total = ids.len();
    // Per-thread tally → reduce. SmallVec→Rule mapping keeps it cache-cheap.
    let counts = ids
        .par_iter()
        .fold(
            || HashMap::<Rule, usize>::new(),
            |mut acc, &id| {
                for r in flag_article(graph, ruleset, id) {
                    *acc.entry(r).or_insert(0) += 1;
                }
                acc
            },
        )
        .reduce(
            || HashMap::<Rule, usize>::new(),
            |mut a, b| {
                for (k, v) in b {
                    *a.entry(k).or_insert(0) += v;
                }
                a
            },
        );
    ExceptionCounts {
        total_articles: total,
        by_rule: counts,
    }
}

#[derive(Debug, Clone)]
pub struct ExceptionCounts {
    pub total_articles: usize,
    pub by_rule: HashMap<Rule, usize>,
}

/// Articles that fire any rule in `selected`. Returns the candidate
/// NodeIds — caller projects rows. Pagination/sort live on the caller
/// side so this stays a pure predicate sweep.
pub fn list_exception_ids(
    graph: &ArticleGraph,
    ruleset: Option<&rcl::RuleSet>,
    selected: &[Rule],
    candidates: Option<&BTreeSet<NodeId>>,
) -> Vec<(NodeId, SmallVec<[Rule; 4]>)> {
    let ids: Vec<NodeId> = match candidates {
        Some(set) => set.iter().copied().collect(),
        None => graph.by_kind[NodeKind::Article.idx()]
            .values()
            .copied()
            .collect(),
    };
    ids.par_iter()
        .filter_map(|&id| {
            let flags = flag_article(graph, ruleset, id);
            // OR-within: include if any selected rule fires.
            let hit = selected.iter().any(|r| flags.contains(r));
            if hit { Some((id, flags)) } else { None }
        })
        .collect()
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Owned mirror of `rcl::ProductHierarchy` (which holds borrows). We
/// build this once per article rather than re-walking parents on every
/// rule check — the per-row cost dominates if the resolver fires hot.
struct OwnedHierarchy {
    product_code: String,
    l0_name: String,
    l1_name: String,
    l2_name: String,
    l3_name: String,
    l4_name: String,
    l5_name: String,
    brand: String,
}

/// Field accessor used by `PsmResolver::explain`. Maps a dimension key
/// from a rule_dim JSON ("l0_name", "brand", ...) to the matching
/// value on the product (as String — explain() allocates per call).
fn field_for(h: &OwnedHierarchy, field: &str) -> String {
    match field {
        "l0_name" => h.l0_name.clone(),
        "l1_name" => h.l1_name.clone(),
        "l2_name" => h.l2_name.clone(),
        "l3_name" => h.l3_name.clone(),
        "l4_name" => h.l4_name.clone(),
        "l5_name" => h.l5_name.clone(),
        "brand" => h.brand.clone(),
        "product_code" => h.product_code.clone(),
        _ => String::new(),
    }
}

impl OwnedHierarchy {
    fn borrow(&self) -> rcl::ProductHierarchy<'_> {
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

fn article_hierarchy(graph: &ArticleGraph, article_id: NodeId) -> OwnedHierarchy {
    let node = graph.node(article_id);
    let mut levels: [&str; 6] = ["", "", "", "", "", ""];
    let mut cur = node.parent;
    while !cur.is_none() && cur != graph.root {
        let p = graph.node(cur);
        match p.kind {
            NodeKind::L0 => levels[0] = graph.get_str(p.name),
            NodeKind::L1 => levels[1] = graph.get_str(p.name),
            NodeKind::L2 => levels[2] = graph.get_str(p.name),
            NodeKind::L3 => levels[3] = graph.get_str(p.name),
            NodeKind::L4 => levels[4] = graph.get_str(p.name),
            NodeKind::L5 => levels[5] = graph.get_str(p.name),
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
    let product_code = node
        .children
        .first()
        .map(|&c| graph.get_str(graph.node(c).name).to_string())
        .unwrap_or_default();
    OwnedHierarchy {
        product_code,
        l0_name: levels[0].to_string(),
        l1_name: levels[1].to_string(),
        l2_name: levels[2].to_string(),
        l3_name: levels[3].to_string(),
        l4_name: levels[4].to_string(),
        l5_name: levels[5].to_string(),
        brand,
    }
}
