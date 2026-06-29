//! Generic graph traversal: `traverse(from, edge) → rows`.
//!
//! Every clickable cell in the SmartStudio UI is a traversal:
//!   - article + "product_codes" → product_code rows
//!   - l1_name + "children" → l2 rows
//!   - l1_name + "articles" → all articles under that l1 (subtree)
//!   - brand + "articles" → all articles tagged with that brand
//!   - channel + "stores" → all stores under that channel
//!   - product_code + "article" → the parent article
//!
//! All output rows go through `projection::project_single` so the
//! inspector renders them with the same shape DataViewPreview uses.
//!
//! `from` is `(kind, name)`. `kind` is mostly a `NodeKind`, with
//! `BRAND` as a virtual sentinel for the cross-index entry point
//! (brand isn't a node in the spine; it's a tag).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

use crate::graph::legacy::projection::project_single;
use crate::graph::legacy::{ArticleGraph, NodeId, NodeKind, StrId};

/// Caller-facing kind label. Mostly a `NodeKind`, plus `Brand` for
/// the cross-index virtual entry point. `Channel` is in `NodeKind`
/// already (the store-spine root).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FromKind {
    L0,
    L1,
    L2,
    L3,
    L4,
    L5,
    Article,
    ProductCode,
    Channel,
    StoreCode,
    Brand,
}

impl FromKind {
    /// Map to the corresponding `NodeKind` if there is one. `Brand`
    /// has no node form — returns None.
    fn to_node_kind(self) -> Option<NodeKind> {
        match self {
            FromKind::L0 => Some(NodeKind::L0),
            FromKind::L1 => Some(NodeKind::L1),
            FromKind::L2 => Some(NodeKind::L2),
            FromKind::L3 => Some(NodeKind::L3),
            FromKind::L4 => Some(NodeKind::L4),
            FromKind::L5 => Some(NodeKind::L5),
            FromKind::Article => Some(NodeKind::Article),
            FromKind::ProductCode => Some(NodeKind::ProductCode),
            FromKind::Channel => Some(NodeKind::Channel),
            FromKind::StoreCode => Some(NodeKind::StoreCode),
            FromKind::Brand => None,
        }
    }
}

/// Edge labels. Some are valid only for certain `FromKind`s; invalid
/// pairs return an error. Lowercase wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    /// Walk parent→children (next level down). Valid for hierarchy
    /// nodes (l0..l5) → next level; article → product_codes; channel
    /// → store_codes.
    Children,
    /// Walk to immediate parent. Valid for everything except root.
    Parent,
    /// Walk all ancestors (root excluded). Valid for any spine node.
    Ancestors,
    /// Cross-edge to all articles tagged with the source. Valid for
    /// `Brand`, `Channel`, and any hierarchy level (subtree articles).
    Articles,
    /// Cross-edge to all stores under the source. Valid for
    /// `Channel`.
    Stores,
    /// Cross-edge to the article's brand (1 row).
    Brand,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TraverseRequest {
    pub from: FromRef,
    pub edge: Edge,
    /// Optional cross-filter selections. When present, applied to children
    /// traversals so the tree only shows nodes whose subtree contains at
    /// least one matching article. Other edges (parent/ancestors/brand)
    /// ignore the filter — they're scalar walks where pruning makes no
    /// sense.
    #[serde(default)]
    pub filters: Vec<crate::cross_filter::model::Filter>,
    /// Optional exception-rule filter. When non-empty, narrows the alive
    /// article set to those firing any of the named rules (Phase 1 set:
    /// stockout, overstock, below_min, reserve_gap, no_eligible_stores).
    /// Composes AND with `filters`.
    #[serde(default)]
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FromRef {
    pub kind: FromKind,
    pub name: String,
}

/// Walk the requested edge from the given (kind, name) and return
/// projected rows. Falls back to an error message rather than empty
/// rows when the (kind, edge) pair is unsupported, so the UI can
/// surface a clear hint.
pub fn traverse(
    graph: &ArticleGraph,
    req: &TraverseRequest,
    ruleset: Option<&rcl::RuleSet>,
) -> Result<Vec<Value>, String> {
    let TraverseRequest { from, edge, filters, rules } = req;
    // Resolve the candidate article set:
    //   filters narrow via cross_filter (article-side dimensions).
    //   rules narrow via exception predicates (stockout, overstock, ...).
    //   When both are present they AND-compose (intersection).
    // Then the alive-ancestor set is computed once for the children-prune
    // path. None = no narrowing, every child passes through.
    let alive: Option<HashSet<NodeId>> = build_alive_set(graph, filters, rules, ruleset);

    match (from.kind, edge) {
        // ── Children traversals ──────────────────────────────────
        (FromKind::L0, Edge::Children)
        | (FromKind::L1, Edge::Children)
        | (FromKind::L2, Edge::Children)
        | (FromKind::L3, Edge::Children)
        | (FromKind::L4, Edge::Children)
        | (FromKind::L5, Edge::Children)
        | (FromKind::Article, Edge::Children)
        | (FromKind::Channel, Edge::Children) => {
            let kind = from.kind.to_node_kind().expect("non-Brand has node_kind");
            let id = find_node(graph, kind, &from.name)?;
            Ok(project_children_filtered(graph, id, alive.as_ref()))
        }
        // article + product_codes is an alias for article + children.
        (FromKind::Article, Edge::Articles) => Err(
            "edge=articles is not valid for FROM=article; use edge=children for product_codes".into(),
        ),

        // ── Parent / ancestors ───────────────────────────────────
        (kind, Edge::Parent) if kind != FromKind::Brand => {
            let nk = kind.to_node_kind().expect("non-Brand has node_kind");
            let id = find_node(graph, nk, &from.name)?;
            let parent = graph.node(id).parent;
            if parent.is_none() || parent == graph.root {
                return Ok(Vec::new());
            }
            Ok(vec![project_single(graph, graph.node(parent).kind, parent, None)
                .unwrap_or(serde_json::Value::Null)])
        }
        (kind, Edge::Ancestors) if kind != FromKind::Brand => {
            let nk = kind.to_node_kind().expect("non-Brand has node_kind");
            let id = find_node(graph, nk, &from.name)?;
            let mut out = Vec::new();
            let mut cur = graph.node(id).parent;
            while !cur.is_none() && cur != graph.root {
                if let Some(row) = project_single(graph, graph.node(cur).kind, cur, None) {
                    out.push(row);
                }
                cur = graph.node(cur).parent;
            }
            Ok(out)
        }

        // ── Subtree articles for hierarchy nodes ─────────────────
        (FromKind::L0, Edge::Articles)
        | (FromKind::L1, Edge::Articles)
        | (FromKind::L2, Edge::Articles)
        | (FromKind::L3, Edge::Articles)
        | (FromKind::L4, Edge::Articles)
        | (FromKind::L5, Edge::Articles) => {
            let kind = from.kind.to_node_kind().expect("non-Brand has node_kind");
            let id = find_node(graph, kind, &from.name)?;
            Ok(project_subtree_articles(graph, id))
        }

        // ── Cross-index entries ──────────────────────────────────
        (FromKind::Brand, Edge::Articles) => Ok(project_brand_articles(graph, &from.name)),
        (FromKind::Channel, Edge::Articles) => Ok(project_channel_articles(graph, &from.name)),
        (FromKind::Article, Edge::Brand) => {
            let id = find_node(graph, NodeKind::Article, &from.name)?;
            let brand = graph.cross_indices.article_to_brand.get(&id);
            match brand {
                Some(b) => Ok(vec![serde_json::json!({
                    "brand": graph.get_str(*b),
                })]),
                None => Ok(Vec::new()),
            }
        }
        (FromKind::Channel, Edge::Stores) => {
            // Channel node's children are store_codes. Same as
            // (Channel, Children) but kept as a distinct edge label
            // for caller clarity.
            let id = find_node(graph, NodeKind::Channel, &from.name)?;
            Ok(project_children(graph, id))
        }

        // ── Unsupported pair ─────────────────────────────────────
        (k, e) => Err(format!(
            "unsupported traversal: from kind={:?} edge={:?}",
            k, e
        )),
    }
}

/// Look up a node by its interned name. The graph drops its
/// `string_index` after build, so we walk `string_pool` for the value.
/// One-shot per traversal; fast enough for an ad-hoc click.
fn find_node(graph: &ArticleGraph, kind: NodeKind, name: &str) -> Result<NodeId, String> {
    let str_id = graph
        .string_pool
        .iter()
        .position(|s| s.as_ref() == name)
        .map(|i| StrId(i as u32))
        .ok_or_else(|| format!("name '{}' not interned in graph", name))?;
    graph
        .find(kind, str_id)
        .ok_or_else(|| format!("node ({:?}, '{}') not found in graph", kind, name))
}

fn project_children(graph: &ArticleGraph, id: NodeId) -> Vec<Value> {
    project_children_filtered(graph, id, None)
}

/// Resolve the alive-ancestor set from cross-filter selections + exception
/// rules. Returned set contains every candidate article AND every hierarchy
/// ancestor up to (but excluding) the root. `None` = no narrowing applied.
///
/// Used by both the traversal and the data path, keeping the rule logic
/// in one place.
pub fn build_alive_set(
    graph: &ArticleGraph,
    filters: &[crate::cross_filter::model::Filter],
    rules: &[String],
    ruleset: Option<&rcl::RuleSet>,
) -> Option<HashSet<NodeId>> {
    if filters.is_empty() && rules.is_empty() {
        return None;
    }
    // Step 1: cross-filter narrowing (article-side dimensions).
    let cross: Option<std::collections::BTreeSet<NodeId>> = if filters.is_empty() {
        None
    } else {
        Some(crate::cross_filter::resolver::apply_filters(graph, filters, None))
    };
    // Step 2: rule narrowing (exception predicates).
    let rule_set: Option<Vec<crate::graph::legacy::exception::Rule>> = if rules.is_empty() {
        None
    } else {
        let parsed: Vec<_> = rules
            .iter()
            .filter_map(|s| crate::graph::legacy::exception::Rule::from_wire(s.as_str()))
            .collect();
        if parsed.is_empty() { None } else { Some(parsed) }
    };
    let candidate_articles: Vec<NodeId> = match (cross.as_ref(), rule_set.as_ref()) {
        (None, None) => return None,
        (Some(set), None) => set.iter().copied().collect(),
        (None, Some(rs)) => {
            crate::graph::legacy::exception::list_exception_ids(graph, ruleset, rs, None)
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        }
        (Some(set), Some(rs)) => {
            crate::graph::legacy::exception::list_exception_ids(graph, ruleset, rs, Some(set))
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        }
    };
    let mut alive: HashSet<NodeId> = HashSet::new();
    for article_id in candidate_articles {
        alive.insert(article_id);
        let mut cur = graph.node(article_id).parent;
        while !cur.is_none() && cur != graph.root {
            if !alive.insert(cur) { break; }
            cur = graph.node(cur).parent;
        }
    }
    Some(alive)
}

/// Same as [`project_children`] but, when `alive` is `Some`, includes
/// only children whose subtree contains at least one matching article
/// (i.e. the child is itself in `alive` — recall that `alive` already
/// contains every ancestor of every candidate article). Used by the
/// tree view's filter-aware drill-down.
fn project_children_filtered(
    graph: &ArticleGraph,
    id: NodeId,
    alive: Option<&HashSet<NodeId>>,
) -> Vec<Value> {
    let children = graph.node(id).children.clone();
    children
        .into_iter()
        .filter(|c| match alive {
            // No filter — pass through.
            None => true,
            // Filter present: only show children whose subtree intersects.
            // For node kinds that aren't on the article-side (CHANNEL → STORE_CODE),
            // alive will not contain them — pass through to keep the store
            // dimension usable independently of article filters.
            Some(set) => {
                let kind = graph.node(*c).kind;
                let article_side = matches!(
                    kind,
                    NodeKind::L0 | NodeKind::L1 | NodeKind::L2 | NodeKind::L3
                    | NodeKind::L4 | NodeKind::L5 | NodeKind::Article | NodeKind::ProductCode
                );
                if !article_side { true } else { set.contains(c) }
            }
        })
        .filter_map(|c| project_single(graph, graph.node(c).kind, c, None))
        .collect()
}

fn project_subtree_articles(graph: &ArticleGraph, root: NodeId) -> Vec<Value> {
    // DFS from root, collecting Article-kind descendants. Bealls
    // hierarchy is shallow (~6 levels), so the stack stays small even
    // for L0 subtrees of ~48 K articles.
    let mut articles: HashSet<NodeId> = HashSet::new();
    let mut stack: Vec<NodeId> = vec![root];
    while let Some(id) = stack.pop() {
        let n = graph.node(id);
        if matches!(n.kind, NodeKind::Article) {
            articles.insert(id);
            continue; // don't descend into product_code children
        }
        for &c in &n.children {
            stack.push(c);
        }
    }
    articles
        .into_iter()
        .filter_map(|id| project_single(graph, NodeKind::Article, id, None))
        .collect()
}

fn project_brand_articles(graph: &ArticleGraph, brand_name: &str) -> Vec<Value> {
    let str_id = match graph
        .string_pool
        .iter()
        .position(|s| s.as_ref() == brand_name)
        .map(|i| StrId(i as u32))
    {
        Some(id) => id,
        None => return Vec::new(),
    };
    let Some(article_ids) = graph.cross_indices.brand_to_articles.get(&str_id) else {
        return Vec::new();
    };
    article_ids
        .iter()
        .filter_map(|&id| project_single(graph, NodeKind::Article, id, None))
        .collect()
}

fn project_channel_articles(graph: &ArticleGraph, channel_name: &str) -> Vec<Value> {
    let str_id = match graph
        .string_pool
        .iter()
        .position(|s| s.as_ref() == channel_name)
        .map(|i| StrId(i as u32))
    {
        Some(id) => id,
        None => return Vec::new(),
    };
    // Inverse of cross_indices.article_to_channel — scan articles for
    // channel match. (~48 K articles, fast in practice; if we want
    // O(1), build a `channel_to_articles` cross-index at build time.)
    graph
        .cross_indices
        .article_to_channel
        .iter()
        .filter(|(_, c)| **c == str_id)
        .filter_map(|(article_id, _)| project_single(graph, NodeKind::Article, *article_id, None))
        .collect()
}
