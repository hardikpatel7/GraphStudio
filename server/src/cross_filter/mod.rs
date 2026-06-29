//! Graph-backed cross-filter.
//!
//! Mirrors the contract of `inventory-smart-rust`'s
//! `POST /cross-filter-v2` route (file
//! `impact_core/src/core/filters/router.rs`). Same `FilterPayload`
//! input, same `FilterResponse` output shape — so callers can switch
//! between SQL and graph backends behind an env flag.
//!
//! The work splits into two pure functions:
//!
//! 1. [`apply_filters`] — narrow the candidate node set. Each
//!    `Filter` operates on a dimension/attribute and intersects with
//!    the running candidate set. Hierarchy filters (`l0_name`..`l5_name`)
//!    walk the graph spine; brand uses
//!    `cross_indices.brand_to_articles`; channel uses
//!    `article_to_channel`. Optional UAM `entitled_set` is intersected
//!    last so unauthorized rows are never visible.
//! 2. [`project_distinct`] — for each requested attribute, collect the
//!    set of distinct values from the candidate articles. Returned as
//!    `HashMap<attr_name, Vec<String>>` (sorted) to match the upstream
//!    response shape.
//!
//! Both functions are read-only over the graph snapshot — safe to call
//! against the live `Arc<ArticleGraph>` without locks.

pub mod model;
pub mod resolver;

pub use model::{
    Attribute, Dimension, Filter, FilterPayload, FilterResponse, FilterType, Operator, Values,
};
pub use resolver::{apply_filters, project_distinct, EntitledSet};
