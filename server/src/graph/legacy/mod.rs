//! V8 — graph-backed `article_selection`.
//!
//! In-memory hierarchy graph (l0 → l1 → … → l5 → article → product_code,
//! plus an analogous store spine) with bottom-up pre-aggregated metrics
//! and pre-bound RCL rule pointers per article. Backs:
//!   - per-product RCL lookup ("which rule matched, what's its payload")
//!   - hierarchy-level aggregate reads ("OH at l1 = 3510")
//!   - subtree filters that yield the leaf entities (articles, product_codes)
//!
//! V7 (`article_selection`) stays untouched. V8 lives alongside it; the
//! medium-term goal is byte-for-byte parity with V7's flat output, derived
//! from the graph instead of a per-PH rayon assembly.
//!
//! See `docs/article-selection-v7.md` (V7 reference + appendix) for the
//! source-of-truth semantics this module reproduces.

pub mod build;
pub mod exception;
pub mod graph;
pub mod projection;
pub mod psm_resolver;
pub mod resolver;
pub mod rollup;
pub mod rows;
pub mod source;
pub mod traverse;

pub use build::{build_graph, BuildStats};
pub use graph::{
    ArticleGraph, METRIC_COUNT, MetricKind, Node, NodeId, NodeKind, RuleKind, RulePtr, StrId,
};
pub use psm_resolver::{PsmExplain, PsmResolver};
pub use resolver::{explain_constraints, explain_dc_policy};
pub use rows::GraphSourceReader;
