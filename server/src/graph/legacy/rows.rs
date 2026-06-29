//! Backend-agnostic row types + the [`GraphSourceReader`] trait.
//!
//! `build::build_graph` reads everything it needs through this trait, so
//! the same builder works against DuckDB (Phase 1, today) or parquet /
//! Postgres / BigQuery (Phase 6, future) without any changes to the
//! graph code itself.
//!
//! The structs here intentionally mirror the V7 row shapes in
//! `article_selection/types.rs` (PhMasterRow, PafRow, InventoryAgg, …).
//! Kept defined locally so the graph crate has no dependency on the V7
//! module — V7 and V8 evolve independently.

use anyhow::Result;
use std::collections::{HashMap, HashSet};

/// PH master row. One per article in `inventory_smart.ph_master`.
/// `product_codes` is the `|`-joined string from V7's extract step
/// (`array_to_string(product_codes, '|')`).
#[derive(Debug, Clone)]
pub struct PhMasterRow {
    pub ph_code: String,
    pub article: String,
    pub l0_name: String,
    pub l1_name: String,
    pub l2_name: String,
    pub l3_name: String,
    pub l4_name: String,
    pub l5_name: String,
    pub brand: String,
    pub channel: String,
    pub product_codes: String,
}

/// Per-product attributes from `global.product_attributes_filter`. The
/// hierarchy fields here are authoritative for product-level RCL match
/// (V7 uses these in `paf_by_pc` keyed by product_code).
#[derive(Debug, Clone)]
pub struct PafRow {
    pub product_code: String,
    pub article: String,
    pub l0_name: String,
    pub l1_name: String,
    pub l2_name: String,
    pub l3_name: String,
    pub l4_name: String,
    pub l5_name: String,
    pub brand: String,
}

/// Pre-aggregated inventory per ph_code (asv2_inventory MV).
#[derive(Debug, Clone, Copy, Default)]
pub struct InventoryAgg {
    pub oh: i64,
    pub oo: i64,
    pub it: i64,
    pub reserve_quantity: i64,
    pub allocated_units: i64,
}

/// Pre-aggregated transaction metrics per ph_code (asv2_txs_metrics MV).
#[derive(Debug, Clone, Copy, Default)]
pub struct TxsMetrics {
    pub lw_units: i64,
    pub lw_margin: i64,
    pub lw_revenue: i64,
}

/// Backend-agnostic source reader. Each fn returns the data the graph
/// builder needs in a fully-materialized Rust collection — no streaming,
/// no SQL handles. The reader is responsible for whatever I/O its
/// backend requires.
///
/// Bound is `Send` only — `duckdb::Connection` is `Send` but not `Sync`,
/// and Phase 1 calls `build_graph` from a single `spawn_blocking` task,
/// so `Sync` isn't required. If a future reader needs concurrent access
/// across multiple builder threads (unlikely — the rollup is itself
/// single-threaded), wrap that backend's handle in a `Mutex`.
///
/// Phase 1 ships only [`source::duckdb::DuckDbReader`]; later phases add
/// `ParquetReader`, `PgReader`, `BqReader` behind this same trait.
pub trait GraphSourceReader: Send {
    /// All ph_master rows that have at least one row in
    /// `article_inventory_dashboard` (i.e. the active set V7 builds for).
    fn read_ph_master(&self) -> Result<Vec<PhMasterRow>>;

    /// Product-attribute filter rows for the active products. Keyed by
    /// `product_code`.
    fn read_paf(&self) -> Result<HashMap<String, PafRow>>;

    /// Per-PH inventory aggregates, keyed by `ph_code`.
    fn read_inventory(&self) -> Result<HashMap<String, InventoryAgg>>;

    /// Per-PH transaction metrics (lw_units / lw_margin / lw_revenue),
    /// keyed by `ph_code`.
    fn read_txs_metrics(&self) -> Result<HashMap<String, TxsMetrics>>;

    /// `product_code → [dc_code]` from active
    /// `product_mapping_product_dc` rows.
    fn read_product_dc(&self) -> Result<HashMap<String, Vec<String>>>;

    /// `store_code → [dc_code]` from active
    /// `product_mapping_store_dc` rows.
    fn read_store_dc(&self) -> Result<HashMap<String, Vec<String>>>;

    /// `dc_code → human name` from active
    /// `distribution_centres` rows.
    fn read_distribution_centres(&self) -> Result<HashMap<String, String>>;

    /// `store_code → channel` from `store_attributes_filter` (active=true).
    fn read_store_channels(&self) -> Result<HashMap<String, String>>;

    /// `store_code → [sg_code]` from `store_groups_mapping` joined with
    /// `store_groups` where not deleted. Returns the inverse of V7's
    /// `sg_mapping` (sg → stores), shaped store-first since the graph
    /// keys store nodes by store_code.
    fn read_store_to_sgs(&self) -> Result<HashMap<String, Vec<String>>>;

    /// Active store_codes from `store_master` (active=true, is_deleted=false).
    /// Used to filter the store-side spine.
    fn read_active_store_codes(&self) -> Result<HashSet<String>>;

    /// PSM priority chain for module 101 from `global.rcl_master` (rows
    /// where module_code = 101 and validity covers today). Returned
    /// pre-sorted by priority ASC. Each entry is (rcl_code, priority).
    fn read_psm_priorities(&self) -> Result<Vec<(String, i32)>>;

    /// PSM rule dimensions from
    /// `global.rcl_product_mapping_product_store_rule`. Returns one
    /// row per rule as `(rcl_code, rule_code, rcl_dimension::text)`.
    /// The dim_json is parsed once at build time into a per-rcl-code
    /// index; the resolver matches the product's hierarchy fields
    /// against each rule's dim map at lookup time. No md5 round-trip.
    ///
    /// On `develop/dev` this returned md5-keyed pairs; the experiment
    /// branch keeps the raw JSON so resolution is on-the-fly and the
    /// per-product `rcl_hash` precompute can be dropped (-700 MB on
    /// the Bealls dataset).
    fn read_psm_rule_dim(&self) -> Result<Vec<(String, String, String)>>;
}
