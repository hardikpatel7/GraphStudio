//! In-memory `article_selection` store (Phase 3 of misty-hinton).
//!
//! Concrete-typed mirror of the `tenant_data.duckdb::article_selection` table.
//! Lives behind `Arc<RwLock<Arc<Vec<…>>>>` so reads (RPC `GetList` /
//! `GetFilterValues`) take the inner `Arc` and walk it lock-free, while
//! writes (`swap`) replace the inner `Arc` atomically.
//!
//! The store is rehydrated:
//!   - on boot, by reading the existing DuckDB table (if any).
//!   - after every successful pipeline run whose declared placement is
//!     `DuckDbAndInMemory` and which produced an `article_selection` table.
//!
//! This is concrete (not generic) on purpose — V4 is the worked example and
//! one type is enough to ship Phase 3. Generalization to a registry keyed by
//! output id can come when a second store needs the same shape.

use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use duckdb::Connection;

use super::types::ArticleSelectionRow;

/// Snapshot-on-replace store. Reads clone the `Arc` and walk it without
/// holding the lock; writes acquire the lock briefly to swap the pointer.
pub struct ArticleSelectionStore {
    rows: RwLock<Arc<Vec<ArticleSelectionRow>>>,
}

impl ArticleSelectionStore {
    pub fn new() -> Self {
        Self {
            rows: RwLock::new(Arc::new(Vec::new())),
        }
    }

    /// Replace the store contents wholesale. Used by the pipeline post-run
    /// hook and by boot-time rehydration.
    pub fn swap(&self, rows: Vec<ArticleSelectionRow>) {
        let mut guard = self.rows.write().expect("ArticleSelectionStore poisoned");
        *guard = Arc::new(rows);
    }

    /// Surgically replace rows for the given `ph_codes` with `replacements`.
    /// Phase 3 partial_recompute path. Existing rows whose `ph_code` is in
    /// `ph_codes` are dropped; `replacements` are appended. Rows in
    /// `replacements` whose `ph_code` is not in `ph_codes` are still appended
    /// (callers shouldn't pass any, but it's well-defined).
    pub fn update_rows(&self, ph_codes: &[String], replacements: Vec<ArticleSelectionRow>) {
        // ph_code is i64 in ArticleSelectionRow; callers still pass String IDs
        // (CDC keys arrive as text). Parse once here, then membership-test.
        let drop: std::collections::HashSet<i64> =
            ph_codes.iter().filter_map(|s| s.parse().ok()).collect();
        let mut guard = self.rows.write().expect("ArticleSelectionStore poisoned");
        let mut next: Vec<ArticleSelectionRow> = guard
            .iter()
            .filter(|r| !drop.contains(&r.ph_code))
            .cloned()
            .collect();
        next.extend(replacements);
        *guard = Arc::new(next);
    }

    /// Take a snapshot reference. Cheap (Arc clone). Caller iterates without
    /// holding any lock.
    pub fn snapshot(&self) -> Arc<Vec<ArticleSelectionRow>> {
        self.rows.read().expect("ArticleSelectionStore poisoned").clone()
    }

    pub fn len(&self) -> usize {
        self.snapshot().len()
    }
}

impl Default for ArticleSelectionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Read every row in `<duckdb_path>::article_selection` and return them as
/// owned `ArticleSelectionRow`s. Used by both boot rehydration and the
/// post-pipeline-run rehydration hook. Returns an empty vec if the table
/// doesn't exist yet (fresh tenant).
pub fn load_from_duckdb(duckdb_path: &str) -> Result<Vec<ArticleSelectionRow>> {
    let conn = Connection::open(duckdb_path)
        .with_context(|| format!("DuckDB open: {}", duckdb_path))?;

    // Check existence first so a fresh tenant doesn't error out.
    let exists: i64 = conn
        .query_row(
            "SELECT count(*) FROM duckdb_tables() \
             WHERE database_name = current_database() AND schema_name = 'main' \
               AND table_name = 'article_selection'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if exists == 0 {
        return Ok(Vec::new());
    }

    // Column order MUST match `materialize::CREATE_SQL` (51 columns —
    // pack_type_id slotted in after last_allocated as part of Bucket 1;
    // Bucket 3 adds mapped_stores/min_type/product_profiles/size_names at end).
    let mut stmt = conn.prepare(
        "SELECT \
            ph_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, \
            style_color_description, product_description, sizes, upc, \
            product_life_cycle, article_status_tag, brand, channel, \
            oh, oo, it, reserve_quantity, allocated_units, net_available_inventory, \
            oh_map, rq_map, au_map, last_allocated, pack_type_id, \
            lw_units, lw_margin, lw_revenue, price, discount, in_stock_perc, \
            aps, min_stock, max_stock, min_stock_validator, max_stock_validator, \
            mapped_stores_count, wos, avg_max_mod, min_woc, max_woc, \
            dcs, store_groups, \
            beginning_available_to_allocate_eaches, beginning_available_to_allocate_packs, \
            allocation_rules, \
            mapped_stores, min_type, product_profiles, size_names \
         FROM article_selection",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ArticleSelectionRow {
                ph_code: r.get(0)?,
                article: r.get(1)?,
                l0_name: r.get(2)?,
                l1_name: r.get(3)?,
                l2_name: r.get(4)?,
                l3_name: r.get(5)?,
                l4_name: r.get(6)?,
                l5_name: r.get(7)?,
                style_color_description: r.get(8)?,
                product_description: r.get(9)?,
                sizes: r.get(10)?,
                upc: r.get(11)?,
                product_life_cycle: r.get(12)?,
                article_status_tag: r.get(13)?,
                brand: r.get(14)?,
                channel: r.get(15)?,
                oh: r.get(16)?,
                oo: r.get(17)?,
                it: r.get(18)?,
                reserve_quantity: r.get(19)?,
                allocated_units: r.get(20)?,
                net_available_inventory: r.get(21)?,
                oh_map: r.get(22)?,
                rq_map: r.get(23)?,
                au_map: r.get(24)?,
                last_allocated: r.get(25)?,
                pack_type_id: r.get(26)?,
                lw_units: r.get(27)?,
                lw_margin: r.get(28)?,
                lw_revenue: r.get(29)?,
                price: r.get(30)?,
                discount: r.get(31)?,
                in_stock_perc: r.get(32)?,
                aps: r.get(33)?,
                min_stock: r.get(34)?,
                max_stock: r.get(35)?,
                min_stock_validator: r.get(36)?,
                max_stock_validator: r.get(37)?,
                mapped_stores_count: r.get(38)?,
                wos: r.get(39)?,
                avg_max_mod: r.get(40)?,
                min_woc: r.get(41)?,
                max_woc: r.get(42)?,
                dcs: r.get(43)?,
                store_groups: r.get(44)?,
                beginning_available_to_allocate_eaches: r.get(45)?,
                beginning_available_to_allocate_packs: r.get(46)?,
                allocation_rules: r.get(47)?,
                mapped_stores: r.get(48)?,
                min_type: r.get(49)?,
                product_profiles: r.get(50)?,
                size_names: r.get(51)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()
        .context("collect article_selection rows")?;
    Ok(rows)
}
