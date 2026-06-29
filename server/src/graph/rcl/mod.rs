//! RCL integration for graph.
//!
//! Decisions 14 + 35 keep RCL out of the metadata schema; this module
//! is the bealls-specific runtime layer that bridges v2 graph state
//! to the shared `rcl::RuleSet` and the bealls-shaped PSM tables.
//!
//! Layout:
//! - `psm_resolver` — pure PSM matcher (priorities + dim schemas → rule_code)
//! - `explain` — DC-policy + constraints resolution against `rcl::RuleSet`
//! - `hierarchy` — build `rcl::ProductHierarchy` from a v2 article NodeId
//! - `build` — read `raw_rcl_psm_*` via `SourceReader`, build PsmResolver

pub mod psm_resolver;
pub mod explain;
pub mod hierarchy;
pub mod build;

pub use build::build_psm_resolver;
pub use explain::{ConstraintsExplain, DcPolicyExplain, explain_constraints, explain_dc_policy};
pub use hierarchy::{OwnedHierarchy, owned_hierarchy_for};
pub use psm_resolver::{PsmExplain, PsmResolver};

use crate::graph::graph::StrId;

/// Which RCL flavor a `RulePtr` resolves. Same vocabulary as v1's
/// `graph::legacy::RuleKind`; kept here so a future v2-only consumer
/// doesn't have to reach into the article_graph module just for the
/// enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    DcPolicy,
    Constraints,
    Psm,
}

/// Pre-bound rule pointer attached to an article node. The actual
/// rule payload lives in `rcl::RuleSet` keyed by `(rcl_code,
/// rule_code)`; this is just the pointer, resolved once per
/// (graph version, RuleSet version).
#[derive(Debug, Clone)]
pub struct RulePtr {
    pub kind: RuleKind,
    pub rcl_code: StrId,
    pub rule_code: StrId,
}
