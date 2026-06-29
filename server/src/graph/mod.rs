//! graph — the unified graph runtime.
//!
//! Two implementations coexist inside this module:
//!   - The metadata-driven engine (this dir): TOML → `GraphSpec` →
//!     `Graph` via `build::build_graph`. Spec-authored hierarchies,
//!     metrics, cross-edges.
//!   - `legacy::` — the hand-coded `ArticleGraph` implementation that
//!     still serves the article-level read endpoints (match_product,
//!     resolve_rcl, aggregate_at, exceptions, etc.) until the
//!     metadata engine covers them. Slated for deletion once that
//!     migration lands.
//!
//! See `docs/v1-cleanup-todo.md` for migration status.

pub mod spec;
pub mod graph;
pub mod source;
pub mod rollup;
pub mod build;
pub mod traverse;
pub mod project;
pub mod cross_filter;
pub mod exception;
pub mod uam_adapter;
pub mod rcl;
pub mod memory;
pub mod legacy;

#[cfg(test)]
mod parity;

pub use build::{BuildStats, build_graph};
pub use graph::Graph;
pub use spec::{Severity, ValidationIssue, from_toml, validate};

// Additional types are re-exported as `graph::graph::*` when needed
// — kept namespaced so this top-level re-export stays minimal and
// rustc doesn't complain about unused public re-exports.
