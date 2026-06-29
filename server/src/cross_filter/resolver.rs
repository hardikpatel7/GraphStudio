//! Pure cross-filter resolution against an `ArticleGraph` snapshot.
//!
//! No I/O, no SQL — read-only over the in-memory graph. Used by the
//! HTTP handler at [`crate::handlers::cross_filter`] and by the UAM
//! cold-load to materialize each user's entitled set.
//!
//! Two phases:
//!   1. `apply_filters` — narrow article candidates by filter set
//!   2. `project_distinct` — for each requested attribute, collect
//!      sorted distinct values across the candidate set

use std::collections::{BTreeSet, HashSet};

use rayon::prelude::*;

use crate::graph::legacy::{ArticleGraph, NodeId, NodeKind};
use crate::cross_filter::model::{Filter, Operator};

/// Concrete entitlement set for a (user, acl) pair. Resolved by the
/// UAM module from each user's `filters` jsonb. Empty / absent means
/// the user is unrestricted for the given dimension.
#[derive(Debug, Clone, Default)]
pub struct EntitledSet {
    /// Article NodeIds the user is allowed to see. None = unrestricted.
    pub articles: Option<HashSet<NodeId>>,
    /// store_code strings the user is allowed to see. None = unrestricted.
    pub store_codes: Option<HashSet<String>>,
}

/// Apply a filter set against the graph and return the candidate
/// article NodeIds. Filters are AND-ed (matches the upstream
/// `query_type` default of `AND`).
///
/// Per-filter semantics:
///   - `attribute_name in {l0_name..l5_name}` (dimension=product or
///     product_store): the article's parent at that level must be in
///     the filter's value set. Walks the spine.
///   - `attribute_name = "brand"`: uses
///     `cross_indices.article_to_brand`.
///   - `attribute_name = "channel"`: uses `article_to_channel`.
///   - `attribute_name = "article"`: direct membership check on the
///     article's own name.
///   - dimension=store filters (store_code, channel, etc.) are
///     applied after the article candidates are computed (they
///     constrain the store-side entitled set; for the cross-filter
///     attribute response they're a no-op on articles unless the
///     caller asked for store-side attributes — handled in
///     project_distinct).
///
/// Operator support: `In` / `Eq` / `InEq` are membership; everything
/// else is logged-and-ignored for now (the SQL backend supports more,
/// but the V8 graph has no need yet for ranges or LIKE patterns —
/// extend when a caller needs it).
///
/// `entitled` is intersected last so unauthorized rows are never
/// visible regardless of how the filters resolve.
pub fn apply_filters(
    graph: &ArticleGraph,
    filters: &[Filter],
    entitled: Option<&EntitledSet>,
) -> BTreeSet<NodeId> {
    // Start with all article NodeIds (excluding empty-name placeholders).
    let mut candidates: BTreeSet<NodeId> = graph.by_kind[NodeKind::Article.idx()]
        .iter()
        .filter_map(|(name, id)| {
            if graph.get_str(*name).is_empty() {
                None
            } else {
                Some(*id)
            }
        })
        .collect();

    // Each filter narrows the set.
    for f in filters {
        if !is_membership(f.operator) {
            tracing::warn!(
                "[cross_filter] unsupported operator {:?} on attribute '{}' — skipping",
                f.operator,
                f.attribute_name
            );
            continue;
        }
        let needles: HashSet<String> = f.values.as_strings().into_iter().collect();
        if needles.is_empty() {
            continue;
        }
        candidates = filter_candidates(graph, &candidates, &f.attribute_name, &needles);
        if candidates.is_empty() {
            return candidates;
        }
    }

    // UAM intersection — applied last and unconditionally.
    if let Some(ent) = entitled {
        if let Some(allowed) = &ent.articles {
            candidates.retain(|id| allowed.contains(id));
        }
    }

    candidates
}

/// Apply one filter to the running candidate set. Read-only over the
/// graph; returns a fresh set.
fn filter_candidates(
    graph: &ArticleGraph,
    candidates: &BTreeSet<NodeId>,
    attribute_name: &str,
    needles: &HashSet<String>,
) -> BTreeSet<NodeId> {
    // Hierarchy attributes match against ancestor names.
    let level = match attribute_name {
        "l0_name" => Some(NodeKind::L0),
        "l1_name" => Some(NodeKind::L1),
        "l2_name" => Some(NodeKind::L2),
        "l3_name" => Some(NodeKind::L3),
        "l4_name" => Some(NodeKind::L4),
        "l5_name" => Some(NodeKind::L5),
        _ => None,
    };
    if let Some(kind) = level {
        return candidates
            .par_iter()
            .copied()
            .filter(|id| {
                ancestor_name(graph, *id, kind)
                    .map(|s| needles.contains(s))
                    .unwrap_or(false)
            })
            .collect();
    }

    match attribute_name {
        "brand" => candidates
            .par_iter()
            .copied()
            .filter(|id| {
                graph
                    .cross_indices
                    .article_to_brand
                    .get(id)
                    .map(|b| needles.contains(graph.get_str(*b)))
                    .unwrap_or(false)
            })
            .collect(),
        "channel" => candidates
            .par_iter()
            .copied()
            .filter(|id| {
                graph
                    .cross_indices
                    .article_to_channel
                    .get(id)
                    .map(|c| needles.contains(graph.get_str(*c)))
                    .unwrap_or(false)
            })
            .collect(),
        "article" => candidates
            .par_iter()
            .copied()
            .filter(|id| needles.contains(graph.get_str(graph.node(*id).name)))
            .collect(),
        // store_* / climate / s0_name etc. — these filter the
        // store-side spine, not articles. For now, keep all articles
        // and let project_distinct handle store-attribute responses
        // separately (matches V2 semantics where store filters narrow
        // the store result while article filters narrow the article
        // result).
        _ => candidates.clone(),
    }
}

fn is_membership(op: Operator) -> bool {
    matches!(op, Operator::In | Operator::InEq | Operator::Eq)
}

/// Walk parents from `id` until we hit a node of the given kind.
/// Returns the interned name as a `&str` for cheap membership checks.
fn ancestor_name<'a>(
    graph: &'a ArticleGraph,
    id: NodeId,
    kind: NodeKind,
) -> Option<&'a str> {
    let mut cur = graph.node(id).parent;
    while !cur.is_none() {
        let n = graph.node(cur);
        if n.kind == kind {
            return Some(graph.get_str(n.name));
        }
        if matches!(n.kind, NodeKind::Root) {
            break;
        }
        cur = n.parent;
    }
    None
}

/// For each requested attribute, walk the candidate articles and
/// collect distinct values. Returned sorted ASC for stable response.
///
/// Supported attribute_names: `article`, `l0_name`..`l5_name`,
/// `brand`, `channel`. Anything else returns an empty list (matches
/// V2 behaviour for unknown columns).
pub fn project_distinct(
    graph: &ArticleGraph,
    candidates: &BTreeSet<NodeId>,
    attribute_names: &[&str],
) -> std::collections::HashMap<String, Vec<String>> {
    let mut out: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::with_capacity(attribute_names.len());
    for attr in attribute_names {
        let values = distinct_for_attribute(graph, candidates, attr);
        out.insert((*attr).to_string(), values);
    }
    out
}

fn distinct_for_attribute(
    graph: &ArticleGraph,
    candidates: &BTreeSet<NodeId>,
    attribute_name: &str,
) -> Vec<String> {
    let level = match attribute_name {
        "l0_name" => Some(NodeKind::L0),
        "l1_name" => Some(NodeKind::L1),
        "l2_name" => Some(NodeKind::L2),
        "l3_name" => Some(NodeKind::L3),
        "l4_name" => Some(NodeKind::L4),
        "l5_name" => Some(NodeKind::L5),
        _ => None,
    };

    let mut set: HashSet<String> = HashSet::new();
    if let Some(kind) = level {
        for id in candidates {
            if let Some(name) = ancestor_name(graph, *id, kind) {
                set.insert(name.to_string());
            }
        }
    } else {
        match attribute_name {
            "article" => {
                for id in candidates {
                    set.insert(graph.get_str(graph.node(*id).name).to_string());
                }
            }
            "brand" => {
                for id in candidates {
                    if let Some(b) = graph.cross_indices.article_to_brand.get(id) {
                        set.insert(graph.get_str(*b).to_string());
                    }
                }
            }
            "channel" => {
                for id in candidates {
                    if let Some(c) = graph.cross_indices.article_to_channel.get(id) {
                        set.insert(graph.get_str(*c).to_string());
                    }
                }
            }
            // Unknown attribute or store-side: empty list.
            _ => {}
        }
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}
