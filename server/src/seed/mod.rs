//! Boot-time seed loaders that bring the tenant data dir into a known
//! shape from on-disk source-controlled artifacts.
//!
//! Each loader is non-fatal: it logs warnings on per-file failure but lets
//! boot proceed. The aim is "best-effort sync from disk", not "abort if
//! anything's off" — a missing or malformed file shouldn't take down a
//! tenant.
//!
//! Layout inside the tenant data dir (`state.data_dir`):
//!   duckdb_views/<view>.sql     — CREATE OR REPLACE VIEW statements
//!   sources/<id>.toml           — Source rows
//!   dataviews/<id>.toml         — DataView rows
//!
//! Files arrive in the tenant dir at `is_new = true` bootstrap (see
//! `instance_config::copy_product_templates`); the loaders here reapply
//! them on every boot.
//!
//! Boot order matters: duckdb_views first (sources may name them as
//! target_table), then sources (dataviews bind to them), then dataviews.

pub mod dataviews;
pub mod duckdb_views;
pub mod sources;
