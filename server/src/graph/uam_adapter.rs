//! Bridge `crate::uam::UamStore` (whose pre-resolved entitlements live
//! in v1 `ArticleGraph` NodeId space) into v2's `EntitledSet`.
//!
//! The v1 store keeps the *raw filters* alongside its pre-resolved
//! v1 NodeIds (see `uam::store::EntitlementEntry::raw_filters`). The
//! adapter here takes those raw filters and re-resolves them against
//! the v2 graph snapshot — sidesteps the v1↔v2 NodeId translation
//! problem entirely, and stays cheap because filter cardinality is
//! always small (a few dimensions × a few values, vs. potentially
//! tens of thousands of pre-resolved NodeIds).
//!
//! Three outcomes per `(user_code, acl_code)` lookup:
//!
//! - Row absent → returns `Lookup::Unknown`. Caller decides whether
//!   to deny (typical for `is_urm_filter = true`) or treat as
//!   unrestricted.
//! - Row with empty filters → `Lookup::Unrestricted`.
//! - Row with filters → `Lookup::Restricted(EntitledSet)`.

use std::collections::HashSet;
use std::sync::Arc;

use super::cross_filter::{EntitledSet, FilterCriterion, apply_filters};
use super::graph::{Graph, KindId};
use crate::cross_filter::model::Filter as V1Filter;
use crate::uam::UamStore;

/// Three-way result so callers don't have to disambiguate "not found"
/// from "found but unrestricted" via tuple flags.
#[derive(Debug, Clone)]
pub enum Lookup {
    /// `(user_code, acl_code)` has no row in `user_access_hierarchy_mapping`.
    /// `is_urm_filter = true` requests should 403; service-to-service
    /// callers usually treat as unrestricted.
    Unknown,
    /// Row present with no filters — full access.
    Unrestricted,
    /// Row present with restrictive filters resolved against the v2 graph.
    /// `EntitledSet.allowed = Some(empty_set)` is preserved (user has
    /// explicit zero access) rather than collapsed to `None`.
    Restricted(EntitledSet),
}

/// Look up the user's entitlements and re-resolve against `graph`.
/// Pure read; no I/O.
pub fn entitled_set_for(
    store: &UamStore,
    user_code: i32,
    acl_code: i32,
    graph: &Graph,
    target_kind: KindId,
) -> Lookup {
    let entry: Arc<crate::uam::store::EntitlementEntry> = match store.lookup(user_code, acl_code) {
        Some(e) => e,
        None => return Lookup::Unknown,
    };

    // Unrestricted: row exists with no filters.
    if entry.raw_filters.is_empty() {
        return Lookup::Unrestricted;
    }

    // Re-resolve raw filters against v2. The v1 store also keeps
    // `entry.entitled` (v1 NodeIds) but we ignore it here — v2's
    // NodeIds differ, and re-resolving is both more correct
    // (handles graph drift between v1 boot and v2 build) and cheaper
    // (filter cardinality, not entitlement size).
    let filters: Vec<FilterCriterion> = entry
        .raw_filters
        .iter()
        .map(|f: &V1Filter| FilterCriterion::from(f))
        .collect();
    let candidates = apply_filters(graph, target_kind, &filters, None);
    let allowed: HashSet<_> = candidates.into_iter().collect();
    Lookup::Restricted(EntitledSet { allowed: Some(allowed) })
}
