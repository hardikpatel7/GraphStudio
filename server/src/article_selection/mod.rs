//! Article Selection materializer.
//!
//! Lifted from `inventory_backend_rust::remote_cache::article_selection_v4`,
//! with the inline RCL resolution swapped out for the in-process
//! [`rcl::RuleStore`].
//!
//! Pipeline:
//! 1. Parallel PG `COPY` of 8 `mv_asv2_*` materialized views (port 5433) +
//!    raw config tables (psaf, store_dc, store_groups, sg_mapping,
//!    distribution_centres, dc_store_policy_user_rule, product_profile_master).
//! 2. Parse CSVs into typed `HashMap`s with rayon.
//! 3. Resolve RCL DcPolicy + Constraints against [`rcl::RuleStore::snapshot`]
//!    (no PG round-trip — rules already live in memory).
//! 4. Rayon assemble 43K rows of [`types::ArticleSelectionRow`].
//! 5. Materialize into the tenant `tenant_data.duckdb` as table
//!    `article_selection` via DuckDB Appender. The dataview's
//!    `source = duckdb_table` reads from there.
//!
//! See `inventory_backend_rust/remote-cache/src/article_selection_v4/ARCHITECTURE.md`
//! for the V4 design doc this module is derived from.
//!
//! ## Prerequisites on the PG instance at `[rcl].port_override`
//!
//! 1. The `asv2_*` materialized views must exist. DDL is shipped alongside
//!    this module at `migrations/0001_asv2_materialized_views.sql` (lifted
//!    from V4). Apply once per tenant.
//! 2. The connecting PG user must have `SELECT` on each MV.
//! 3. The MVs must be refreshed periodically (`REFRESH MATERIALIZED VIEW
//!    CONCURRENTLY mv_asv2_*`) for the data to stay current. Out of scope
//!    for this module.

pub mod extractor;
pub mod materialize;
pub mod store;
pub mod types;

pub use extractor::{
    ExtractionResult, extract_and_assemble, extract_and_assemble_from_duckdb,
    extract_and_assemble_scoped,
};
pub use materialize::{MaterializeResult, materialize_partial_to_duckdb, materialize_to_duckdb};
pub use store::{ArticleSelectionStore, load_from_duckdb};
pub use types::ArticleSelectionRow;
