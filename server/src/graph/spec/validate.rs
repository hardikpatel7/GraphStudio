//! Metadata-only validation for `GraphSpec`.
//!
//! Implements the mechanical checks called out in the locked decisions
//! (23, 26, 28, 31, 33, 36). Each check has a stable `code` so the UI
//! can group/explain issues without parsing the message string.
//!
//! Out of scope here (handled later by a DuckDB-aware
//! `validate_with_catalog()`):
//!   - column existence in the actual source table
//!   - column type compatibility with the rollup operator
//!   - `*-1` uniqueness on the `to` side (Decision 26's strict-mode check)
//!   - `dimension = "<name>"` matching a row in SmartStudio's
//!     `dimensions` table (Decision 34)
//!
//! Those need either the DuckDB catalog or SQLite, both of which the
//! pure `validate()` signature avoids by design — the same function
//! runs in unit tests, in the API handler, and at build-time pre-flight
//! without dragging in those dependencies.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use super::{AttachesAt, Cardinality, GraphSpec, Rollup};

/// One validation finding. `code` is the machine-stable identifier;
/// `location` is a dot-path into the spec (e.g. `hierarchy.product.l0`,
/// `sources[2]`, `relation[0].from`) so the UI can scroll to it.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

impl ValidationIssue {
    fn err(code: &'static str, message: impl Into<String>, location: Option<String>) -> Self {
        Self { severity: Severity::Error, code, message: message.into(), location }
    }
    fn warn(code: &'static str, message: impl Into<String>, location: Option<String>) -> Self {
        Self { severity: Severity::Warning, code, message: message.into(), location }
    }
}

/// Run every check against `spec`. Returns all issues — `validate` is
/// non-fatal; the caller decides whether errors block (UI save / build
/// pre-flight do) or are merely reported (preview, lint).
pub fn validate(spec: &GraphSpec) -> Vec<ValidationIssue> {
    let mut out = Vec::new();

    check_top_level(spec, &mut out);
    let source_aliases = collect_source_aliases(spec, &mut out);
    let level_owner = collect_level_owner(spec, &mut out);
    check_hierarchies(spec, &source_aliases, &mut out);
    check_sources(spec, &level_owner, &mut out);
    check_relations(spec, &source_aliases, &mut out);
    check_metrics(spec, &source_aliases, &mut out);
    check_source_roles(spec, &mut out);
    check_reachability(spec, &mut out);

    out
}

// ──────────────────────────────────────────────────────────────────────────
// Top-level
// ──────────────────────────────────────────────────────────────────────────

fn check_top_level(spec: &GraphSpec, out: &mut Vec<ValidationIssue>) {
    if spec.id.trim().is_empty() {
        out.push(ValidationIssue::err("GRAPH_ID_EMPTY", "graph `id` must be non-empty", None));
    }
    if spec.display_name.trim().is_empty() {
        out.push(ValidationIssue::err(
            "GRAPH_DISPLAY_NAME_EMPTY",
            "graph `display_name` must be non-empty",
            None,
        ));
    }
    if spec.hierarchy.is_empty() {
        out.push(ValidationIssue::err(
            "GRAPH_NO_HIERARCHY",
            "graph must declare at least one `[hierarchy.<name>]`",
            None,
        ));
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Sources
// ──────────────────────────────────────────────────────────────────────────

/// Build the alias → table-name lookup and surface duplicate aliases.
/// Returned set is used by every later check that resolves an alias —
/// keeps the "alias unknown" error consistent rather than each check
/// rolling its own.
fn collect_source_aliases(
    spec: &GraphSpec,
    out: &mut Vec<ValidationIssue>,
) -> HashSet<String> {
    let mut seen: HashSet<String> = HashSet::new();
    for (i, s) in spec.sources.iter().enumerate() {
        if !seen.insert(s.alias.clone()) {
            out.push(ValidationIssue::err(
                "SRC_DUPLICATE_ALIAS",
                format!("duplicate source alias `{}`", s.alias),
                Some(format!("sources[{i}]")),
            ));
        }
        if s.alias.trim().is_empty() {
            out.push(ValidationIssue::err(
                "SRC_EMPTY_ALIAS",
                "source `alias` must be non-empty",
                Some(format!("sources[{i}]")),
            ));
        }
        if s.table.trim().is_empty() {
            out.push(ValidationIssue::err(
                "SRC_EMPTY_TABLE",
                format!("source `{}` is missing `table`", s.alias),
                Some(format!("sources[{i}]")),
            ));
        }
    }
    seen
}

// ──────────────────────────────────────────────────────────────────────────
// Hierarchies & levels
// ──────────────────────────────────────────────────────────────────────────

/// Build the level-id → hierarchy-name lookup. Decision 28 requires that
/// `attaches_at` references resolve to a unique kind, which only holds
/// when level ids are globally unique across hierarchies. We flag
/// collisions here once; downstream `attaches_at` checks then use the
/// resulting map without re-handling the ambiguity.
fn collect_level_owner(
    spec: &GraphSpec,
    out: &mut Vec<ValidationIssue>,
) -> HashMap<String, String> {
    let mut owner: HashMap<String, String> = HashMap::new();
    for (hname, h) in &spec.hierarchy {
        for level_id in h.levels.keys() {
            if let Some(prev) = owner.insert(level_id.clone(), hname.clone()) {
                out.push(ValidationIssue::err(
                    "HIER_LEVEL_NAME_COLLISION",
                    format!(
                        "level id `{level_id}` declared in both `[hierarchy.{prev}]` and `[hierarchy.{hname}]`; level ids must be globally unique so `attaches_at` references are unambiguous",
                    ),
                    Some(format!("hierarchy.{hname}.{level_id}")),
                ));
            }
        }
    }
    owner
}

fn check_hierarchies(
    spec: &GraphSpec,
    source_aliases: &HashSet<String>,
    out: &mut Vec<ValidationIssue>,
) {
    for (hname, h) in &spec.hierarchy {
        if h.source.trim().is_empty() {
            out.push(ValidationIssue::err(
                "HIER_NO_SOURCE",
                format!("`[hierarchy.{hname}]` is missing `source`"),
                Some(format!("hierarchy.{hname}")),
            ));
        } else if !source_aliases.contains(&h.source) {
            out.push(ValidationIssue::err(
                "HIER_SOURCE_UNKNOWN",
                format!(
                    "`[hierarchy.{hname}].source` references unknown source alias `{}`",
                    h.source
                ),
                Some(format!("hierarchy.{hname}")),
            ));
        }

        if h.levels.is_empty() {
            out.push(ValidationIssue::err(
                "HIER_NO_LEVELS",
                format!("`[hierarchy.{hname}]` declares no levels"),
                Some(format!("hierarchy.{hname}")),
            ));
        }

        for (level_id, lvl) in &h.levels {
            let loc = format!("hierarchy.{hname}.{level_id}");
            if lvl.column.trim().is_empty() {
                out.push(ValidationIssue::err(
                    "LEVEL_NO_COLUMN",
                    format!("level `{level_id}` is missing `column`"),
                    Some(loc.clone()),
                ));
            }
            if lvl.split.is_some() && lvl.unnest == Some(true) {
                out.push(ValidationIssue::err(
                    "LEVEL_SPLIT_AND_UNNEST",
                    format!(
                        "level `{level_id}` declares both `split` and `unnest`; choose one based on the column's actual DuckDB type"
                    ),
                    Some(loc),
                ));
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Source attach declarations
// ──────────────────────────────────────────────────────────────────────────

fn check_sources(
    spec: &GraphSpec,
    level_owner: &HashMap<String, String>,
    out: &mut Vec<ValidationIssue>,
) {
    // Primary hierarchy = first declared (Decision 31).
    let primary_hierarchy = spec.hierarchy.keys().next().cloned();

    for (i, s) in spec.sources.iter().enumerate() {
        let Some(attach) = &s.attaches_at else { continue };
        let loc = format!("sources[{i}].attaches_at");

        let kinds: Vec<&str> = attach.kinds().collect();
        let mut primary_count = 0usize;
        for k in &kinds {
            match level_owner.get(*k) {
                None => out.push(ValidationIssue::err(
                    "SRC_ATTACH_UNKNOWN_KIND",
                    format!(
                        "source `{}` declares `attaches_at` kind `{k}` which is not a declared hierarchy level",
                        s.alias
                    ),
                    Some(loc.clone()),
                )),
                Some(owner) => {
                    if Some(owner.clone()) == primary_hierarchy {
                        primary_count += 1;
                    }
                }
            }
        }

        if matches!(attach, AttachesAt::Composite(_)) && primary_count != 1 {
            // Decision 28: composite attach is "exactly one primary + zero-
            // or-more auxiliary". Two primary kinds, or zero primary kinds
            // (dim × dim), are both rejected here.
            out.push(ValidationIssue::err(
                "SRC_ATTACH_COMPOSITE_PRIMARY_COUNT",
                format!(
                    "source `{}` composite attach must contain exactly one kind from the primary hierarchy (`{}`); got {primary_count}",
                    s.alias,
                    primary_hierarchy.as_deref().unwrap_or("?")
                ),
                Some(loc),
            ));
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Relations
// ──────────────────────────────────────────────────────────────────────────

fn check_relations(
    spec: &GraphSpec,
    source_aliases: &HashSet<String>,
    out: &mut Vec<ValidationIssue>,
) {
    for (i, r) in spec.relations.iter().enumerate() {
        let loc_from = format!("relation[{i}].from");
        let loc_to = format!("relation[{i}].to");

        if !source_aliases.contains(&r.from.alias) {
            out.push(ValidationIssue::err(
                "REL_UNKNOWN_ALIAS",
                format!("relation `from.alias = \"{}\"` is not a declared source", r.from.alias),
                Some(loc_from.clone()),
            ));
        }
        if !source_aliases.contains(&r.to.alias) {
            out.push(ValidationIssue::err(
                "REL_UNKNOWN_ALIAS",
                format!("relation `to.alias = \"{}\"` is not a declared source", r.to.alias),
                Some(loc_to.clone()),
            ));
        }

        if r.from.columns.is_empty() && r.to.columns.is_empty() {
            out.push(ValidationIssue::err(
                "REL_NO_COLUMNS",
                "relation has no `columns` on either side",
                Some(format!("relation[{i}]")),
            ));
        } else if r.from.columns.len() != r.to.columns.len() {
            out.push(ValidationIssue::err(
                "REL_COL_COUNT_MISMATCH",
                format!(
                    "relation has {} columns on `from` and {} on `to`; counts must match (positional pairing, Decision 25)",
                    r.from.columns.len(),
                    r.to.columns.len()
                ),
                Some(format!("relation[{i}]")),
            ));
        } else {
            // Same-length columns. Decision 25: when names differ, warn.
            // Doesn't catch reordering mistakes, but does prompt the
            // author to re-check positional pairing.
            for (a, b) in r.from.columns.iter().zip(r.to.columns.iter()) {
                if a != b {
                    out.push(ValidationIssue::warn(
                        "REL_COL_NAME_DIFFERS",
                        format!(
                            "relation pairs columns `{a}` (from) ↔ `{b}` (to); names differ — verify positional order is correct",
                        ),
                        Some(format!("relation[{i}]")),
                    ));
                    break; // one warning per relation is enough
                }
            }
        }

        if r.from.cardinality == Cardinality::Many && r.to.cardinality == Cardinality::Many {
            out.push(ValidationIssue::err(
                "REL_BOTH_MANY",
                "relation has `*` on both sides; n:m primitives are rejected — use a bridge source with two n:1 relations (Decision 24)",
                Some(format!("relation[{i}]")),
            ));
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Metrics
// ──────────────────────────────────────────────────────────────────────────

fn check_metrics(
    spec: &GraphSpec,
    source_aliases: &HashSet<String>,
    out: &mut Vec<ValidationIssue>,
) {
    for (src_alias, metrics) in &spec.metrics {
        if !source_aliases.contains(src_alias) {
            out.push(ValidationIssue::err(
                "METRIC_SOURCE_UNKNOWN",
                format!("`[metrics.{src_alias}]` references unknown source alias"),
                Some(format!("metrics.{src_alias}")),
            ));
        }
        // A metrics block on a source with no `attaches_at` is meaningless
        // — without an attach point the engine has no node to write the
        // value onto.
        if let Some(s) = spec.sources.iter().find(|s| &s.alias == src_alias) {
            if s.attaches_at.is_none() {
                out.push(ValidationIssue::err(
                    "METRIC_SOURCE_NO_ATTACH",
                    format!(
                        "`[metrics.{src_alias}]` is declared but source `{src_alias}` has no `attaches_at`; metrics need an attach point",
                    ),
                    Some(format!("metrics.{src_alias}")),
                ));
            }
        }
        for (mid, m) in metrics {
            if m.column.is_some() && m.expr.is_some() {
                out.push(ValidationIssue::warn(
                    "METRIC_BOTH_COLUMN_AND_EXPR",
                    format!(
                        "metric `{src_alias}.{mid}` sets both `column` and `expr`; `expr` will take precedence — drop one for clarity",
                    ),
                    Some(format!("metrics.{src_alias}.{mid}")),
                ));
            }
            // Collection rollups (set/list) over an `expr` are fine, but
            // count/count_distinct expect a column reference, not a
            // derived expression with aggregation semantics. Flag both
            // patterns the user is likely to get wrong.
            if matches!(m.rollup, Rollup::Count | Rollup::CountDistinct) && m.expr.is_some() {
                out.push(ValidationIssue::warn(
                    "METRIC_COUNT_OVER_EXPR",
                    format!(
                        "metric `{src_alias}.{mid}` uses `{:?}` rollup over an `expr`; counts are usually applied to columns",
                        m.rollup
                    ),
                    Some(format!("metrics.{src_alias}.{mid}")),
                ));
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Source role inference (Decision 36)
// ──────────────────────────────────────────────────────────────────────────

/// Decision 36 says a source is either a metric source (has `attaches_at`)
/// or a bridge source (no `attaches_at`, ≥2 relations to different
/// hierarchies). Anything else is malformed.
fn check_source_roles(spec: &GraphSpec, out: &mut Vec<ValidationIssue>) {
    // For each source, count relations and which hierarchies' sources
    // they touch. A source's "hierarchy reach" comes from relations whose
    // *other side* is a hierarchy spine source.
    let hierarchy_sources: HashMap<&str, &str> = spec
        .hierarchy
        .iter()
        .map(|(name, h)| (h.source.as_str(), name.as_str()))
        .collect();

    for (i, s) in spec.sources.iter().enumerate() {
        if s.attaches_at.is_some() {
            continue; // metric source — role unambiguous
        }
        // Bridge candidate: collect hierarchies this source reaches
        // through one-hop relations.
        let mut reached: HashSet<&str> = HashSet::new();
        for r in &spec.relations {
            let other = if r.from.alias == s.alias {
                Some(r.to.alias.as_str())
            } else if r.to.alias == s.alias {
                Some(r.from.alias.as_str())
            } else {
                None
            };
            if let Some(o) = other {
                if let Some(hname) = hierarchy_sources.get(o) {
                    reached.insert(hname);
                }
            }
        }
        if reached.len() < 2 {
            out.push(ValidationIssue::err(
                "SRC_ROLE_AMBIGUOUS",
                format!(
                    "source `{}` has no `attaches_at` and isn't a bridge — it reaches {} hierarchy/ies. Declare `attaches_at = \"<kind>\"` (metric source) or add a second relation to a different hierarchy (bridge source)",
                    s.alias,
                    reached.len()
                ),
                Some(format!("sources[{i}]")),
            ));
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Reachability (Decision 23)
// ──────────────────────────────────────────────────────────────────────────

/// Every source actually used by the graph must be reachable through
/// `[[relation]]` edges from the primary hierarchy's source — *unless*
/// it carries `attaches_at` matching a registered kind, in which case
/// Decision 33's "column names align → implicit identity join" rule
/// applies and the source is auto-reachable. Walking the relation
/// graph undirected matches the join semantics: relations are
/// bidirectional at the read layer even when cardinality is asymmetric.
fn check_reachability(spec: &GraphSpec, out: &mut Vec<ValidationIssue>) {
    let Some(primary) = spec.hierarchy.values().next() else { return };
    let root = primary.source.as_str();

    // Adjacency from declared relations.
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for r in &spec.relations {
        adj.entry(r.from.alias.as_str()).or_default().push(r.to.alias.as_str());
        adj.entry(r.to.alias.as_str()).or_default().push(r.from.alias.as_str());
    }

    // Seed reachability with the primary source AND every
    // self-attaching source (Decision 33 — implicit identity join). A
    // self-attaching source declares a single attach kind that exists
    // in some hierarchy, so the engine resolves the join transparently
    // off the kind's identifying column without an explicit relation.
    let kinds: HashSet<&str> = spec
        .hierarchy
        .values()
        .flat_map(|h| h.levels.keys().map(String::as_str))
        .collect();
    let mut reached: HashSet<&str> = HashSet::from([root]);
    let mut queue: VecDeque<&str> = VecDeque::from([root]);
    for s in &spec.sources {
        if let Some(AttachesAt::Single(k)) = &s.attaches_at {
            if kinds.contains(k.as_str()) && reached.insert(s.alias.as_str()) {
                queue.push_back(s.alias.as_str());
            }
        }
    }

    while let Some(n) = queue.pop_front() {
        if let Some(neighbors) = adj.get(n) {
            for &m in neighbors {
                if reached.insert(m) {
                    queue.push_back(m);
                }
            }
        }
    }

    // Every hierarchy's source must be reachable.
    for (hname, h) in &spec.hierarchy {
        if !reached.contains(h.source.as_str()) {
            out.push(ValidationIssue::err(
                "REACHABILITY_HIERARCHY",
                format!(
                    "hierarchy `{hname}` source `{}` is not reachable from primary source `{root}` via declared `[[relation]]`s",
                    h.source
                ),
                Some(format!("hierarchy.{hname}")),
            ));
        }
    }
    // Every metric source must be reachable.
    for src_alias in spec.metrics.keys() {
        if !reached.contains(src_alias.as_str()) {
            out.push(ValidationIssue::err(
                "REACHABILITY_METRIC",
                format!(
                    "metric source `{src_alias}` is not reachable from primary source `{root}` via declared `[[relation]]`s and does not have an `attaches_at` matching a hierarchy kind",
                ),
                Some(format!("metrics.{src_alias}")),
            ));
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::spec::parse::from_toml;

    /// Minimal valid graph — one hierarchy, one source, one metric. All
    /// other tests start from this baseline and break one thing.
    const MINIMAL: &str = r#"
id = "g"
display_name = "G"

[[sources]]
alias = "src"
table = "tbl"
attaches_at = "leaf"

[hierarchy.product]
source = "src"

[hierarchy.product.leaf]
column = "leaf_id"

[metrics.src]
v = { rollup = "sum" }
"#;

    fn issues(toml: &str) -> Vec<ValidationIssue> {
        validate(&from_toml(toml).expect("parse"))
    }

    // `code` is `&'static str`, so the returned strings don't borrow
    // from the input slice — annotating the lifetime explicitly lets the
    // caller compose `codes(&issues(&t)).contains(…)` without keeping
    // the temporary `Vec<ValidationIssue>` alive.
    fn codes(issues: &[ValidationIssue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    #[test]
    fn minimal_passes() {
        let v = issues(MINIMAL);
        assert!(v.iter().all(|i| i.severity == Severity::Warning), "unexpected errors: {v:#?}");
    }

    #[test]
    fn relation_n_to_n_rejected() {
        // Add a second source + a *-* relation between them.
        let t = MINIMAL.to_string()
            + r#"
[[sources]]
alias = "other"
table = "other_tbl"
attaches_at = "leaf"

[[relation]]
from = { alias = "src",   columns = ["a"], cardinality = "*" }
to   = { alias = "other", columns = ["a"], cardinality = "*" }
"#;
        assert!(codes(&issues(&t)).contains(&"REL_BOTH_MANY"));
    }

    #[test]
    fn unknown_attach_kind() {
        let t = MINIMAL.replace(r#"attaches_at = "leaf""#, r#"attaches_at = "ghost""#);
        assert!(codes(&issues(&t)).contains(&"SRC_ATTACH_UNKNOWN_KIND"));
    }

    #[test]
    fn metric_source_with_valid_attach_is_auto_reachable() {
        // Decision 33: a metric source whose `attaches_at` matches a
        // declared hierarchy level is implicitly joined via that
        // level's identifying column. No `[[relation]]` needed, no
        // REACHABILITY_METRIC error.
        let t = MINIMAL.to_string()
            + r#"
[[sources]]
alias = "lonely"
table = "lonely_tbl"
attaches_at = "leaf"

[metrics.lonely]
x = { rollup = "sum" }
"#;
        let cs = codes(&issues(&t));
        assert!(!cs.contains(&"REACHABILITY_METRIC"), "auto-reachable; got: {cs:?}");
    }

    #[test]
    fn split_and_unnest_mutually_exclusive() {
        let t = MINIMAL.replace(
            r#"column = "leaf_id""#,
            r#"column = "leaf_id"
split  = "|"
unnest = true"#,
        );
        assert!(codes(&issues(&t)).contains(&"LEVEL_SPLIT_AND_UNNEST"));
    }

    /// The inventorysmart graph template lives at
    /// `templates/inventorysmart/graphs/default.toml`. This test
    /// exercises the parser + validator against that file so a
    /// regression surfaces immediately rather than waiting for a
    /// runtime POST `/api/graphs/:id/validate` against a real tenant.
    #[test]
    fn inventorysmart_template_parses_and_validates() {
        // CARGO_MANIFEST_DIR points at `server/`; template TOML lives at
        // `../templates/inventorysmart/graphs/...` from there.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../templates/inventorysmart/graphs/default.toml"
        );
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        let spec = crate::graph::spec::from_toml(&text)
            .unwrap_or_else(|e| panic!("parse inventorysmart template: {e:#}"));
        let issues = validate(&spec);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| matches!(i.severity, Severity::Error))
            .collect();
        assert!(
            errors.is_empty(),
            "bealls TOML failed validation:\n{}",
            errors
                .iter()
                .map(|i| format!("  [{}] {}", i.code, i.message))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
