//! Write `Vec<ArticleSelectionRow>` into the tenant's `tenant_data.duckdb`
//! as table `article_selection`.
//!
//! Strategy: drop + create the table, then bulk-insert via DuckDB's Appender
//! (no SQL parsing per row, batched flush). The dataview's
//! `source = duckdb_table` reads from this table on every `/data` call.

use std::time::Instant;

use anyhow::{Context, Result};
use duckdb::Connection;

use super::types::ArticleSelectionRow;

pub struct MaterializeResult {
    pub rows_written: usize,
    pub duckdb_ms: u128,
}

/// DuckDB DDL for the `article_selection` table. Schema matches
/// [`ArticleSelectionRow`] field-for-field.
pub const CREATE_SQL: &str = r#"
CREATE OR REPLACE TABLE article_selection (
    ph_code                                BIGINT,
    article                                VARCHAR,
    l0_name                                VARCHAR,
    l1_name                                VARCHAR,
    l2_name                                VARCHAR,
    l3_name                                VARCHAR,
    l4_name                                VARCHAR,
    l5_name                                VARCHAR,
    style_color_description                VARCHAR,
    product_description                    VARCHAR,
    sizes                                  VARCHAR,
    upc                                    VARCHAR,
    product_life_cycle                     VARCHAR,
    article_status_tag                     VARCHAR,
    brand                                  VARCHAR,
    channel                                VARCHAR,
    oh                                     BIGINT,
    oo                                     BIGINT,
    it                                     BIGINT,
    reserve_quantity                       BIGINT,
    allocated_units                        BIGINT,
    net_available_inventory                BIGINT,
    oh_map                                 VARCHAR,
    rq_map                                 VARCHAR,
    au_map                                 VARCHAR,
    last_allocated                         VARCHAR,
    pack_type_id                           BIGINT,
    lw_units                               BIGINT,
    lw_margin                              BIGINT,
    lw_revenue                             BIGINT,
    price                                  DOUBLE,
    discount                               DOUBLE,
    in_stock_perc                          DOUBLE,
    aps                                    DOUBLE,
    min_stock                              BIGINT,
    max_stock                              BIGINT,
    min_stock_validator                    BIGINT,
    max_stock_validator                    BIGINT,
    mapped_stores_count                    BIGINT,
    wos                                    BIGINT,
    avg_max_mod                            BIGINT,
    min_woc                                BIGINT,
    max_woc                                BIGINT,
    dcs                                    VARCHAR,
    store_groups                           VARCHAR,
    beginning_available_to_allocate_eaches BIGINT,
    beginning_available_to_allocate_packs  BIGINT,
    allocation_rules                       VARCHAR,
    mapped_stores                          VARCHAR,
    min_type                               VARCHAR,
    product_profiles                       VARCHAR,
    size_names                             VARCHAR
);
"#;

/// Surgically replace the rows for `ph_codes` in
/// `<duckdb_path>::article_selection` with the supplied rows. Used by the
/// Phase 3 partial_recompute path so a small CDC change doesn't rewrite the
/// whole table. The table is created (with full schema) if it doesn't exist
/// yet — first scoped run on a fresh tenant works without a prior full run.
pub fn materialize_partial_to_duckdb(
    duckdb_path: &str,
    ph_codes: &[String],
    rows: &[ArticleSelectionRow],
) -> Result<MaterializeResult> {
    use std::time::Instant;
    let start = Instant::now();
    let conn = Connection::open(duckdb_path)
        .with_context(|| format!("DuckDB open: {}", duckdb_path))?;

    // Ensure the table exists (no-op if already there). Matches the full path.
    conn.execute_batch(CREATE_SQL_IF_MISSING).context("ensure article_selection")?;

    if ph_codes.is_empty() {
        return Ok(MaterializeResult { rows_written: 0, duckdb_ms: start.elapsed().as_millis() });
    }
    // DELETE WHERE ph_code IN (?, ?, …). Parameterized to avoid SQL injection;
    // ph_codes were validated upstream but we re-isolate here.
    // ph_code is now BIGINT in the DDL — parse the caller's String IDs so
    // the DELETE binds match the column type.
    let placeholders: Vec<&str> = (0..ph_codes.len()).map(|_| "?").collect();
    let delete_sql = format!(
        "DELETE FROM article_selection WHERE ph_code IN ({})",
        placeholders.join(",")
    );
    let ph_ints: Vec<i64> = ph_codes.iter()
        .map(|s| s.parse::<i64>().unwrap_or(0))
        .collect();
    let params: Vec<&dyn duckdb::ToSql> = ph_ints
        .iter()
        .map(|n| n as &dyn duckdb::ToSql)
        .collect();
    let deleted = conn.execute(&delete_sql, params.as_slice())
        .context("delete partial article_selection")?;

    {
        let mut app = conn.appender("article_selection").context("open appender")?;
        for r in rows {
            app.append_row(duckdb::params![
                r.ph_code, r.article, r.l0_name, r.l1_name, r.l2_name, r.l3_name,
                r.l4_name, r.l5_name, r.style_color_description, r.product_description,
                r.sizes, r.upc, r.product_life_cycle, r.article_status_tag, r.brand,
                r.channel, r.oh, r.oo, r.it, r.reserve_quantity, r.allocated_units,
                r.net_available_inventory, r.oh_map, r.rq_map, r.au_map, r.last_allocated,
                r.pack_type_id,
                r.lw_units, r.lw_margin, r.lw_revenue, r.price, r.discount, r.in_stock_perc,
                r.aps, r.min_stock, r.max_stock, r.min_stock_validator, r.max_stock_validator,
                r.mapped_stores_count, r.wos, r.avg_max_mod, r.min_woc, r.max_woc,
                r.dcs, r.store_groups,
                r.beginning_available_to_allocate_eaches,
                r.beginning_available_to_allocate_packs,
                r.allocation_rules,
                r.mapped_stores, r.min_type, r.product_profiles, r.size_names,
            ]).with_context(|| format!("append_row ph_code={}", r.ph_code))?;
        }
        app.flush().context("appender flush")?;
    }
    conn.execute_batch("CHECKPOINT").ok();

    tracing::info!(
        "[article_selection] partial materialize: deleted {}, inserted {} for {} keys ({}ms)",
        deleted, rows.len(), ph_codes.len(), start.elapsed().as_millis()
    );
    Ok(MaterializeResult { rows_written: rows.len(), duckdb_ms: start.elapsed().as_millis() })
}

/// Idempotent CREATE used by the partial-materialize path. Same column
/// shape as `CREATE_SQL` but `IF NOT EXISTS` so it's safe to run repeatedly.
const CREATE_SQL_IF_MISSING: &str = r#"
CREATE TABLE IF NOT EXISTS article_selection (
    ph_code                                BIGINT,
    article                                VARCHAR,
    l0_name                                VARCHAR,
    l1_name                                VARCHAR,
    l2_name                                VARCHAR,
    l3_name                                VARCHAR,
    l4_name                                VARCHAR,
    l5_name                                VARCHAR,
    style_color_description                VARCHAR,
    product_description                    VARCHAR,
    sizes                                  VARCHAR,
    upc                                    VARCHAR,
    product_life_cycle                     VARCHAR,
    article_status_tag                     VARCHAR,
    brand                                  VARCHAR,
    channel                                VARCHAR,
    oh                                     BIGINT,
    oo                                     BIGINT,
    it                                     BIGINT,
    reserve_quantity                       BIGINT,
    allocated_units                        BIGINT,
    net_available_inventory                BIGINT,
    oh_map                                 VARCHAR,
    rq_map                                 VARCHAR,
    au_map                                 VARCHAR,
    last_allocated                         VARCHAR,
    pack_type_id                           BIGINT,
    lw_units                               BIGINT,
    lw_margin                              BIGINT,
    lw_revenue                             BIGINT,
    price                                  DOUBLE,
    discount                               DOUBLE,
    in_stock_perc                          DOUBLE,
    aps                                    DOUBLE,
    min_stock                              BIGINT,
    max_stock                              BIGINT,
    min_stock_validator                    BIGINT,
    max_stock_validator                    BIGINT,
    mapped_stores_count                    BIGINT,
    wos                                    BIGINT,
    avg_max_mod                            BIGINT,
    min_woc                                BIGINT,
    max_woc                                BIGINT,
    dcs                                    VARCHAR,
    store_groups                           VARCHAR,
    beginning_available_to_allocate_eaches BIGINT,
    beginning_available_to_allocate_packs  BIGINT,
    allocation_rules                       VARCHAR,
    mapped_stores                          VARCHAR,
    min_type                               VARCHAR,
    product_profiles                       VARCHAR,
    size_names                             VARCHAR
);
"#;

/// Materialize `rows` into `<duckdb_path>::article_selection`. Drops + recreates
/// the table on every call (snapshot semantics — V4 doesn't do incremental
/// CDC, neither do we).
pub fn materialize_to_duckdb(
    duckdb_path: &str,
    rows: &[ArticleSelectionRow],
) -> Result<MaterializeResult> {
    let start = Instant::now();
    let conn = Connection::open(duckdb_path)
        .with_context(|| format!("DuckDB open: {}", duckdb_path))?;

    conn.execute_batch(CREATE_SQL).context("create article_selection")?;

    {
        let mut app = conn.appender("article_selection").context("open appender")?;
        for r in rows {
            // Order MUST match CREATE_SQL column order exactly — 47 columns
            // (46 original + pack_type_id slotted in after last_allocated).
            app.append_row(duckdb::params![
                r.ph_code, r.article, r.l0_name, r.l1_name, r.l2_name, r.l3_name,
                r.l4_name, r.l5_name, r.style_color_description, r.product_description,
                r.sizes, r.upc, r.product_life_cycle, r.article_status_tag, r.brand,
                r.channel, r.oh, r.oo, r.it, r.reserve_quantity, r.allocated_units,
                r.net_available_inventory, r.oh_map, r.rq_map, r.au_map, r.last_allocated,
                r.pack_type_id,
                r.lw_units, r.lw_margin, r.lw_revenue, r.price, r.discount, r.in_stock_perc,
                r.aps, r.min_stock, r.max_stock, r.min_stock_validator, r.max_stock_validator,
                r.mapped_stores_count, r.wos, r.avg_max_mod, r.min_woc, r.max_woc,
                r.dcs, r.store_groups,
                r.beginning_available_to_allocate_eaches,
                r.beginning_available_to_allocate_packs,
                r.allocation_rules,
                r.mapped_stores, r.min_type, r.product_profiles, r.size_names,
            ]).with_context(|| format!("append_row ph_code={}", r.ph_code))?;
        }
        app.flush().context("appender flush")?;
    }

    conn.execute_batch("CHECKPOINT").ok();

    Ok(MaterializeResult {
        rows_written: rows.len(),
        duckdb_ms: start.elapsed().as_millis(),
    })
}
