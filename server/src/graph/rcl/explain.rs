//! Per-product RCL explain functions — ported verbatim from
//! `crate::graph::legacy::resolver`.
//!
//! Walks the priority + specificity chain on a live `rcl::RuleSet`
//! and surfaces the matched `(rcl_code, rule_code)` plus the resolved
//! payload. Used by the gRPC `ResolveRcl` RPC backing the SmartStudio
//! "RCL Explorer" UI, and by the RCL-dependent exception rules
//! (Overstock / BelowMin) which need `min_stock` / `max_stock` off the
//! resolved constraint row.
//!
//! Cost: O(n_rules × n_selector_fields) per product, ≤ 16 rules × 7
//! fields = sub-microsecond. Per-call (not pre-bound) since the gRPC
//! viewer is a low-volume surface; exception rules iterate every
//! article, but with a small rule corpus this stays well inside the
//! rollup's overall time budget.

use std::collections::{HashMap, HashSet};

use rcl::{ConstraintRow, DcPolicy, ProductHierarchy, RclRule, RuleSet};

/// Result of explaining a single resolution. `rcl_code` is the
/// `rcl_master` row that won the priority match; `rule_code` is the
/// PolicyRuleDim / ConstraintRuleDim entry that won the dimension
/// match (empty string `""` for the wildcard projection).
#[derive(Debug, Clone)]
pub struct DcPolicyExplain<'r> {
    pub rcl_code: String,
    pub rule_code: String,
    pub policy: &'r DcPolicy,
}

#[derive(Debug, Clone)]
pub struct ConstraintsExplain<'r> {
    pub rcl_code: String,
    pub rule_code: String,
    pub rows: &'r [ConstraintRow],
}

/// Explain DC-policy resolution for a single product.
///
/// Resolution chain (matches `rcl::resolve_dc_policy`):
///   1. Pre-filter `rules.rules` to those that have at least one entry
///      in `rules.policies` (otherwise we'd happily match an rcl_code
///      with no DcPolicy at all).
///   2. Walk those candidates in priority order; first match wins.
///   3. Walk `rules.policy_rules[rcl_code]` (already specificity-sorted
///      DESC by `parse_policy_rules`) and pick the first whose
///      dimensions are all satisfied by the product's hierarchy. Fall
///      back to the wildcard `("", rcl_code)` entry if none match.
///   4. Look up the resolved DcPolicy in `rules.policies`.
pub fn explain_dc_policy<'r>(
    rules: &'r RuleSet,
    p: &ProductHierarchy<'_>,
) -> Option<DcPolicyExplain<'r>> {
    let candidate_rcl_codes: HashSet<&str> =
        rules.policies.keys().map(|(rcl, _)| rcl.as_str()).collect();
    let rule = first_matching_rule(rules, &candidate_rcl_codes, p)?;
    let rcl_code = rule.rcl_code.clone();
    let rule_code = pick_rule_code_for_policy(rules, &rcl_code, p)?;
    let policy = rules.policies.get(&(rcl_code.clone(), rule_code.clone()))?;
    Some(DcPolicyExplain { rcl_code, rule_code, policy })
}

/// Explain constraints resolution. Same shape as [`explain_dc_policy`]
/// but against `constraints` / `constraint_rules`.
pub fn explain_constraints<'r>(
    rules: &'r RuleSet,
    p: &ProductHierarchy<'_>,
) -> Option<ConstraintsExplain<'r>> {
    let candidate_rcl_codes: HashSet<&str> =
        rules.constraints.keys().map(|(rcl, _)| rcl.as_str()).collect();
    let rule = first_matching_rule(rules, &candidate_rcl_codes, p)?;
    let rcl_code = rule.rcl_code.clone();
    let rule_code = pick_rule_code_for_constraints(rules, &rcl_code, p)?;
    let rows = rules.constraints.get(&(rcl_code.clone(), rule_code.clone()))?;
    Some(ConstraintsExplain { rcl_code, rule_code, rows: rows.as_slice() })
}

// ── Internal helpers ────────────────────────────────────────────────

/// Walk `rules.rules` in priority/specificity order (already sorted in
/// `parse_rcl_master`) and return the first whose `rcl_code` is in
/// `candidates` and whose selectors match the product.
fn first_matching_rule<'r>(
    rules: &'r RuleSet,
    candidates: &HashSet<&str>,
    p: &ProductHierarchy<'_>,
) -> Option<&'r RclRule> {
    rules.rules.iter().find(|r| {
        candidates.contains(r.rcl_code.as_str())
            && r.matches(p.l0_name, p.l1_name, p.l2_name, p.l3_name, p.l4_name, p.l5_name, p.brand)
    })
}

/// Pick the most-specific rule_code from `rules.policy_rules[rcl_code]`
/// (already specificity-sorted DESC). Falls back to the wildcard
/// rule_code `""` if the wildcard policy entry exists.
fn pick_rule_code_for_policy(
    rules: &RuleSet,
    rcl_code: &str,
    p: &ProductHierarchy<'_>,
) -> Option<String> {
    if let Some(dims_list) = rules.policy_rules.get(rcl_code) {
        for entry in dims_list {
            if dimensions_satisfied(&entry.dimensions, p) {
                return Some(entry.rule_code.clone());
            }
        }
    }
    if rules.policies.contains_key(&(rcl_code.to_string(), String::new())) {
        return Some(String::new());
    }
    None
}

fn pick_rule_code_for_constraints(
    rules: &RuleSet,
    rcl_code: &str,
    p: &ProductHierarchy<'_>,
) -> Option<String> {
    if let Some(dims_list) = rules.constraint_rules.get(rcl_code) {
        for entry in dims_list {
            if dimensions_satisfied(&entry.dimensions, p) {
                return Some(entry.rule_code.clone());
            }
        }
    }
    if rules.constraints.contains_key(&(rcl_code.to_string(), String::new())) {
        return Some(String::new());
    }
    None
}

/// Returns true iff every key in `dims` matches the corresponding
/// hierarchy field on `p`. Unknown keys are skipped (treated as
/// "any"). Mirrors `rcl::resolver::dimension_satisfied`.
fn dimensions_satisfied(dims: &HashMap<String, String>, p: &ProductHierarchy<'_>) -> bool {
    for (k, v) in dims {
        let actual = match k.as_str() {
            "l0_name" => p.l0_name,
            "l1_name" => p.l1_name,
            "l2_name" => p.l2_name,
            "l3_name" => p.l3_name,
            "l4_name" => p.l4_name,
            "l5_name" => p.l5_name,
            "brand" => p.brand,
            _ => continue,
        };
        if actual != v.as_str() {
            return false;
        }
    }
    true
}
