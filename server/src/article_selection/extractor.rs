//! PG `COPY` extraction → CSV parse → rayon assembly. RCL resolution comes
//! from the in-process [`rcl::RuleSet`] snapshot — no PG round-trip for rules.
//!
//! Lifted from V4 (`article_selection_v4::extractor`). Differences from V4:
//! - The 3 RCL `COPY` queries (rcl_master, rcl_dc_store_policy,
//!   rcl_constraint_master) are removed.
//! - Inline `resolve_rcl_dc_store_policy` / `resolve_rcl_constraints` are
//!   replaced by [`rcl::resolve_dc_policy`] / [`rcl::resolve_constraints`].
//! - The dead V3-style raw aggregators (compute_txs, compute_inventory, …)
//!   are dropped — V4 used the `mv_asv2_*` MVs directly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use futures::StreamExt;
use rayon::prelude::*;
use tokio_postgres::NoTls;

use super::types::*;

/// Output of one extraction run.
pub struct ExtractionResult {
    pub rows: Vec<ArticleSelectionRow>,
    pub total_ms: u128,
}

/// Run extraction + assembly. `ruleset` is a snapshot from the in-process
/// `RuleStore`; the function holds the `Arc` for the duration so the
/// borrowed `&DcPolicy` / `&[ConstraintRow]` returned by the resolvers stay
/// alive.
pub async fn extract_and_assemble(
    pg_default_dsn: &str,
    ruleset: Arc<rcl::RuleSet>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<ExtractionResult> {
    let total_start = Instant::now();
    let bail = || -> Result<()> {
        if cancel.is_cancelled() {
            Err(anyhow!("article_selection: cancelled"))
        } else {
            Ok(())
        }
    };

    // ── Phase 1: COPY (8 MVs in parallel + 5 raw config tables) ──
    let (
        ph_raw,
        txs_raw,
        inv_raw,
        woc_raw,
        instock_raw,
        before_alloc_raw,
        paf_raw,
        product_dc_raw,
        store_dc_raw,
        store_groups_raw,
        sg_mapping_raw,
        dist_centres_raw,
        dc_store_rule_raw,
    ) = tokio::try_join!(
        copy_table(pg_default_dsn, "SELECT ph_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, style_color_description, product_description, sizes, product_codes, product_lifecycle, article_status_tag, brand, channel FROM inventory_smart.asv2_ph_master"),
        copy_table(pg_default_dsn, "SELECT ph_code, lw_units, lw_margin, lw_revenue, price, discount, in_stock_perc FROM inventory_smart.asv2_txs_metrics"),
        copy_table(pg_default_dsn, "SELECT ph_code, oh, oo, it, reserve_quantity, allocated_units FROM inventory_smart.asv2_inventory"),
        copy_table(pg_default_dsn, "SELECT ph_code, woc, avg_max_mod, min_woc, max_woc, woc_mapped_stores_count FROM inventory_smart.asv2_woc"),
        copy_table(pg_default_dsn, "SELECT ph_code, in_stock_perc, dc_instock FROM inventory_smart.asv2_instock"),
        copy_table(pg_default_dsn, "SELECT ph_code, eaches, packs FROM inventory_smart.asv2_before_alloc"),
        copy_table(pg_default_dsn, "SELECT product_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, brand FROM inventory_smart.asv2_paf"),
        copy_table(pg_default_dsn, "SELECT product_code, string_agg(dc_code, '|') AS dc_codes FROM inventory_smart.asv2_product_dc GROUP BY product_code"),
        copy_table(pg_default_dsn, "SELECT store_code, dc_code FROM global.product_mapping_store_dc WHERE is_active=true"),
        copy_table(pg_default_dsn, "SELECT sg_code, name, is_deleted FROM global.store_groups WHERE is_deleted=false"),
        copy_table(pg_default_dsn, "SELECT sg_code, store_code FROM global.store_groups_mapping"),
        copy_table(pg_default_dsn, "SELECT dc_code, name FROM global.distribution_centres WHERE is_active=true AND is_deleted=false"),
        copy_table(pg_default_dsn, "SELECT rule_code, rule_type, values FROM inventory_smart.dc_store_policy_user_rule"),
    ).map_err(|e| anyhow!("COPY extraction failed: {}", e))?;

    bail()?;
    let extract_ms = total_start.elapsed().as_millis();
    tracing::info!(
        "[article_selection] COPY done in {}ms (8 MVs + 5 raw tables from {})",
        extract_ms, pg_dsn_redact(pg_default_dsn)
    );

    // ── Phase 2: parse CSVs into typed maps ──
    let parse_start = Instant::now();
    let ph_master = parse_ph_master(&ph_raw);
    let txs_by_ph = parse_mv_txs(&txs_raw);
    let inv_by_ph = parse_mv_inventory(&inv_raw);
    let woc_by_ph = parse_mv_woc(&woc_raw);
    let instock_by_ph = parse_mv_instock(&instock_raw);
    let before_alloc_by_ph = parse_mv_before_alloc(&before_alloc_raw);
    let paf_by_pc = parse_paf(&paf_raw);
    let product_dc = parse_product_dc(&product_dc_raw);
    let store_dc_set: HashSet<String> = parse_store_dc(&store_dc_raw);
    let sg_mapping = parse_sg_mapping(&sg_mapping_raw);
    let store_groups_map = parse_store_groups(&store_groups_raw);
    let dist_centres = parse_dist_centres(&dist_centres_raw);
    let dc_store_rules = parse_dc_store_rules(&dc_store_rule_raw);

    tracing::info!(
        "[article_selection] parse done in {}ms: {} ph, {} txs, {} inv, {} woc, {} paf",
        parse_start.elapsed().as_millis(),
        ph_master.len(), txs_by_ph.len(), inv_by_ph.len(), woc_by_ph.len(), paf_by_pc.len()
    );
    bail()?;

    // ── Phase 3: RCL resolution (in-process, no PG round-trip) ──
    let rcl_start = Instant::now();
    let products: Vec<rcl::ProductHierarchy<'_>> = paf_by_pc
        .iter()
        .map(|(pc, paf)| rcl::ProductHierarchy {
            product_code: pc,
            l0_name: &paf.l0_name,
            l1_name: &paf.l1_name,
            l2_name: &paf.l2_name,
            l3_name: &paf.l3_name,
            l4_name: &paf.l4_name,
            l5_name: &paf.l5_name,
            brand: &paf.brand,
        })
        .collect();
    let dc_policy_by_pc = rcl::resolve_dc_policy(&ruleset, &products);
    let constraint_by_pc = rcl::resolve_constraints(&ruleset, &products);
    tracing::info!(
        "[article_selection] RCL resolved in {}ms: {} products → {} DC policies, {} constraint matches",
        rcl_start.elapsed().as_millis(),
        products.len(), dc_policy_by_pc.len(), constraint_by_pc.len()
    );

    bail()?;

    // ── Phase 4: rayon assemble ──
    let assemble_start = Instant::now();
    let active_ph: Vec<&PhMasterRow> = ph_master.iter().collect();
    let empty_inv: HashMap<String, InvPerSizeDc> = HashMap::new();
    let empty_la: HashMap<String, String> = HashMap::new();
    let empty_sn: HashMap<(String, String), String> = HashMap::new();
    let empty_pp: HashMap<String, String> = HashMap::new();
    let empty_pc_contribs: HashMap<String, Vec<(String, StoreContrib)>> = HashMap::new();
    let empty_chan: HashMap<String, String> = HashMap::new();
    let ctx = AssembleCtx {
        product_dc: &product_dc, dist_centres: &dist_centres, store_dc_set: &store_dc_set,
        sg_mapping: &sg_mapping, store_groups_map: &store_groups_map,
        dc_store_rules: &dc_store_rules,
        txs_by_ph: &txs_by_ph, inv_by_ph: &inv_by_ph, woc_by_ph: &woc_by_ph,
        instock_by_ph: &instock_by_ph, before_alloc_by_ph: &before_alloc_by_ph,
        dc_policy_by_pc: &dc_policy_by_pc, constraint_by_pc: &constraint_by_pc,
        // V4 path doesn't extract Bucket-3 sources — pass empty so those fields
        // emit None.
        inv_per_size_dc: &empty_inv,
        last_allocated: &empty_la,
        size_name_by_art_size: &empty_sn,
        default_min_type: None,
        profiles_by_ph: &empty_pp,
        pc_store_contribs: &empty_pc_contribs,
        store_channels: &empty_chan,
    };
    let rows: Vec<ArticleSelectionRow> = active_ph
        .par_iter()
        .map(|ph| assemble_row(ph, &ctx))
        .collect();

    let total_ms = total_start.elapsed().as_millis();
    tracing::info!(
        "[article_selection] assembly done: {} rows in {}ms (total: {}ms)",
        rows.len(), assemble_start.elapsed().as_millis(), total_ms
    );
    Ok(ExtractionResult { rows, total_ms })
}

// ─── DuckDB-backed assembly (V7) ─────────────────────────────────────────────

/// Reads the 8 `asv2_*` aggregated tables + 5 raw config tables from a
/// tenant DuckDB file (produced by `pl_v7_extracts` + `pl_v7_build`'s
/// `build_asv2_*` queries) and runs the same RCL + rayon assembly as
/// `extract_and_assemble`. Replaces ~22s of PG COPYs with sub-second
/// DuckDB SELECTs.
///
/// `asv2_product_dc` carries `dc_codes` as a pipe-separated string in V7's
/// shape (one row per product); we split it on `'|'` to recover the
/// `Vec<String>` the assembler expects. Other tables match their original
/// CSV-parsed layout one-to-one.
pub fn extract_and_assemble_from_duckdb(
    duckdb_path: &str,
    ruleset: std::sync::Arc<rcl::RuleSet>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<ExtractionResult> {
    let total_start = Instant::now();
    let conn = duckdb::Connection::open(duckdb_path)
        .with_context(|| format!("DuckDB open: {}", duckdb_path))?;

    // Phase boundaries that let cancellation land before the next slow
    // read or before the rayon assembly fan-out.
    let bail = || -> Result<()> {
        if cancel.is_cancelled() {
            Err(anyhow::anyhow!("article_selection_v7: cancelled"))
        } else {
            Ok(())
        }
    };

    // ── Phase 1: read 13 tables from local DuckDB ──
    let read_start = Instant::now();
    let ph_master = read_ph_master_duckdb(&conn).context("read asv2_ph_master")?;
    bail()?;
    let txs_by_ph = read_txs_metrics_duckdb(&conn).context("read asv2_txs_metrics")?;
    bail()?;
    let inv_by_ph = read_inventory_duckdb(&conn).context("read asv2_inventory")?;
    bail()?;
    let woc_by_ph = read_woc_duckdb(&conn).context("read asv2_woc")?;
    let instock_by_ph = read_instock_duckdb(&conn).context("read asv2_instock")?;
    let before_alloc_by_ph = read_before_alloc_duckdb(&conn).context("read asv2_before_alloc")?;
    let paf_by_pc = read_paf_duckdb(&conn).context("read asv2_paf")?;
    bail()?;
    let product_dc = read_product_dc_duckdb(&conn).context("read asv2_product_dc")?;
    let store_dc_set = read_store_dc_duckdb(&conn).context("read raw_store_dc_mapping")?;
    let sg_mapping = read_sg_mapping_duckdb(&conn).context("read raw_store_groups_mapping")?;
    let store_groups_map = read_store_groups_duckdb(&conn).context("read raw_store_groups")?;
    let dist_centres = read_dist_centres_duckdb(&conn).context("read raw_distribution_centres")?;
    let dc_store_rules = read_dc_store_rules_duckdb(&conn).context("read raw_dc_store_policy_user_rule")?;
    bail()?;

    tracing::info!(
        "[article_selection_v7] DuckDB reads done in {}ms: {} ph, {} txs, {} inv, {} woc, {} paf",
        read_start.elapsed().as_millis(),
        ph_master.len(), txs_by_ph.len(), inv_by_ph.len(), woc_by_ph.len(), paf_by_pc.len()
    );

    bail()?;

    // ── Phase 2: RCL resolution (identical to extract_and_assemble) ──
    let rcl_start = Instant::now();
    let products: Vec<rcl::ProductHierarchy<'_>> = paf_by_pc
        .iter()
        .map(|(pc, paf)| rcl::ProductHierarchy {
            product_code: pc,
            l0_name: &paf.l0_name,
            l1_name: &paf.l1_name,
            l2_name: &paf.l2_name,
            l3_name: &paf.l3_name,
            l4_name: &paf.l4_name,
            l5_name: &paf.l5_name,
            brand: &paf.brand,
        })
        .collect();
    let dc_policy_by_pc = rcl::resolve_dc_policy(&ruleset, &products);
    let constraint_by_pc = rcl::resolve_constraints(&ruleset, &products);
    tracing::info!(
        "[article_selection_v7] RCL resolved in {}ms: {} products → {} DC policies, {} constraints",
        rcl_start.elapsed().as_millis(),
        products.len(), dc_policy_by_pc.len(), constraint_by_pc.len()
    );



    // ── Bucket 3: extra DuckDB reads to populate oh_map / rq_map / au_map /
    // last_allocated / size_names / mapped_stores / min_type / product_profiles ──
    let inv_per_size_dc = read_inv_per_size_dc_duckdb(&conn).unwrap_or_default();
    let last_allocated = read_last_allocated_duckdb(&conn).unwrap_or_default();
    let size_name_by_art_size = read_paf_sizes_duckdb(&conn).unwrap_or_default();
    let profiles_by_ph = read_product_profiles_duckdb(&conn).unwrap_or_default();
    let default_min_type = derive_default_min_type(&dc_store_rules);

    // ── Bucket 4: PSA → store + product-store eligibility for proper
    // constraint aggregation (per legacy v2 constraints_resolved_data x
    // psm_ph_store). Without these, mapped_stores / mapped_stores_count
    // can't be computed, and constraint averages are off.
    let psa_to_stores = read_psa_store_map_duckdb(&conn).unwrap_or_default();
    let dc_to_stores = read_dc_to_stores_duckdb(&conn).unwrap_or_default();
    let store_channels = read_store_channels_duckdb(&conn).unwrap_or_default();
    let psm_eligible_stores_by_pc = read_psm_eligible_stores_duckdb(&conn).unwrap_or_default();
    let eligible_stores_by_pc = build_product_store_eligibility(&product_dc, &store_dc_set, &dc_to_stores);
    // Legacy v2 _rcl_input_query filters PSM input by default_store_groups
    // (resolved per product via DC-policy module 10003 → sg_codes →
    // store_groups_mapping). Build the same filter here so V7's PSM result
    // matches PG byte-for-byte. Without this, mapped_stores includes every
    // store in store_groups_mapping that satisfies PSAF, even ones outside
    // the article's default sg.
    let default_sg_stores_by_pc = build_default_sg_stores_by_pc(&dc_policy_by_pc, &sg_mapping);

    // Per-product memoization: expand each constraint row's PSA → eligible
    // stores once. The per-PH `compute_constraints` then just merges its
    // product_codes' contributions instead of re-expanding (O(173k×800×700)
    // → O(173k×800×700 once) + O(48k×6×~200)).
    let pc_contribs_start = Instant::now();
    let pc_store_contribs = precompute_pc_store_contribs(
        &constraint_by_pc, &psa_to_stores, &eligible_stores_by_pc, &paf_by_pc,
        &psm_eligible_stores_by_pc, &default_sg_stores_by_pc,
    );
    tracing::info!(
        "[article_selection_v7] B3+B4+B5 reads: {} inv_per_size_dc, {} last_allocated, {} size_names, {} profiles, {} psa_map entries, {} eligible products, {} psm_eligible products, {} default_sg products, {} pc_store_contribs in {}ms, default_min_type={:?}",
        inv_per_size_dc.len(), last_allocated.len(), size_name_by_art_size.len(),
        profiles_by_ph.len(), psa_to_stores.len(), eligible_stores_by_pc.len(),
        psm_eligible_stores_by_pc.len(), default_sg_stores_by_pc.len(),
        pc_store_contribs.len(), pc_contribs_start.elapsed().as_millis(),
        default_min_type
    );

    bail()?;

    // ── Phase 3: rayon assemble (identical to extract_and_assemble) ──
    let assemble_start = Instant::now();
    let active_ph: Vec<&PhMasterRow> = ph_master.iter().collect();
    let ctx = AssembleCtx {
        product_dc: &product_dc, dist_centres: &dist_centres, store_dc_set: &store_dc_set,
        sg_mapping: &sg_mapping, store_groups_map: &store_groups_map,
        dc_store_rules: &dc_store_rules,
        txs_by_ph: &txs_by_ph, inv_by_ph: &inv_by_ph, woc_by_ph: &woc_by_ph,
        instock_by_ph: &instock_by_ph, before_alloc_by_ph: &before_alloc_by_ph,
        dc_policy_by_pc: &dc_policy_by_pc, constraint_by_pc: &constraint_by_pc,
        inv_per_size_dc: &inv_per_size_dc,
        last_allocated: &last_allocated,
        size_name_by_art_size: &size_name_by_art_size,
        default_min_type: default_min_type.as_deref(),
        profiles_by_ph: &profiles_by_ph,
        pc_store_contribs: &pc_store_contribs,
        store_channels: &store_channels,
    };
    let rows: Vec<ArticleSelectionRow> = active_ph
        .par_iter()
        .map(|ph| assemble_row(ph, &ctx))
        .collect();

    let total_ms = total_start.elapsed().as_millis();
    tracing::info!(
        "[article_selection_v7] assembly done: {} rows in {}ms (total: {}ms)",
        rows.len(), assemble_start.elapsed().as_millis(), total_ms
    );
    Ok(ExtractionResult { rows, total_ms })
}

fn read_ph_master_duckdb(conn: &duckdb::Connection) -> Result<Vec<PhMasterRow>> {
    // CAST every text-ish column to VARCHAR — DuckDB auto-infers BIGINT for
    // any column whose sample is all-numeric (even after PG-side ::TEXT
    // cast, since CSV serialization is type-agnostic). Forcing VARCHAR at
    // read time guarantees the Rust `String::get` calls succeed.
    let mut stmt = conn.prepare(
        "SELECT CAST(ph_code AS VARCHAR), CAST(article AS VARCHAR),
                CAST(l0_name AS VARCHAR), CAST(l1_name AS VARCHAR),
                CAST(l2_name AS VARCHAR), CAST(l3_name AS VARCHAR),
                CAST(l4_name AS VARCHAR), CAST(l5_name AS VARCHAR),
                CAST(style_color_description AS VARCHAR),
                CAST(product_description AS VARCHAR),
                CAST(sizes AS VARCHAR), CAST(product_codes AS VARCHAR),
                CAST(product_lifecycle AS VARCHAR),
                CAST(article_status_tag AS VARCHAR),
                CAST(brand AS VARCHAR), CAST(channel AS VARCHAR)
         FROM asv2_ph_master",
    )?;
    let rows = stmt
        .query_map([], |r| {
            // NULL-safe string read: PG NULL → empty string. duckdb-rs's
            // String::get errors on NULL, so we go through Option<String>.
            let s = |i: usize| -> duckdb::Result<String> {
                r.get::<_, Option<String>>(i).map(|o| o.unwrap_or_default())
            };
            Ok(PhMasterRow {
                ph_code: s(0)?,
                article: s(1)?,
                l0_name: s(2)?,
                l1_name: s(3)?,
                l2_name: s(4)?,
                l3_name: s(5)?,
                l4_name: s(6)?,
                l5_name: s(7)?,
                style_color_description: s(8)?,
                product_description: s(9)?,
                sizes: s(10)?,
                product_codes: s(11)?,
                product_life_cycle: s(12)?,
                article_status_tag: s(13)?,
                brand: s(14)?,
                channel: s(15)?,
            })
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(rows)
}

fn read_txs_metrics_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, TxsMetrics>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(ph_code AS VARCHAR),
                CAST(lw_units AS BIGINT), CAST(lw_margin AS BIGINT), CAST(lw_revenue AS BIGINT),
                CAST(price AS DOUBLE), CAST(discount AS DOUBLE), CAST(in_stock_perc AS DOUBLE)
         FROM asv2_txs_metrics",
    )?;
    let pairs: Vec<(String, TxsMetrics)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                TxsMetrics {
                    lw_units: r.get::<_, i64>(1).unwrap_or(0),
                    lw_margin: r.get::<_, i64>(2).unwrap_or(0),
                    lw_revenue: r.get::<_, i64>(3).unwrap_or(0),
                    price: r.get::<_, f64>(4).unwrap_or(0.0),
                    discount: r.get::<_, f64>(5).unwrap_or(0.0),
                    in_stock_perc: r.get::<_, f64>(6).unwrap_or(0.0),
                },
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_inventory_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, InventoryAgg>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(ph_code AS VARCHAR),
                CAST(oh AS BIGINT), CAST(oo AS BIGINT), CAST(it AS BIGINT),
                CAST(reserve_quantity AS BIGINT), CAST(allocated_units AS BIGINT)
         FROM asv2_inventory",
    )?;
    let pairs: Vec<(String, InventoryAgg)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                InventoryAgg {
                    oh: r.get::<_, i64>(1).unwrap_or(0),
                    oo: r.get::<_, i64>(2).unwrap_or(0),
                    it: r.get::<_, i64>(3).unwrap_or(0),
                    reserve_quantity: r.get::<_, i64>(4).unwrap_or(0),
                    allocated_units: r.get::<_, i64>(5).unwrap_or(0),
                },
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_woc_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, WocAgg>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(ph_code AS VARCHAR),
                CAST(woc AS DOUBLE), CAST(avg_max_mod AS DOUBLE),
                CAST(min_woc AS DOUBLE), CAST(max_woc AS DOUBLE)
         FROM asv2_woc",
    )?;
    let pairs: Vec<(String, WocAgg)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                WocAgg {
                    woc: r.get::<_, f64>(1).unwrap_or(0.0),
                    avg_max_mod: r.get::<_, f64>(2).unwrap_or(0.0),
                    min_woc: r.get::<_, f64>(3).unwrap_or(0.0),
                    max_woc: r.get::<_, f64>(4).unwrap_or(0.0),
                },
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_instock_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, InstockAgg>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(ph_code AS VARCHAR),
                CAST(in_stock_perc AS DOUBLE), CAST(dc_instock AS DOUBLE)
         FROM asv2_instock",
    )?;
    let pairs: Vec<(String, InstockAgg)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                InstockAgg {
                    in_stock_perc: r.get::<_, f64>(1).unwrap_or(0.0),
                    dc_instock: r.get::<_, f64>(2).unwrap_or(0.0),
                },
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_before_alloc_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, BeforeAllocAgg>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(ph_code AS VARCHAR),
                CAST(eaches AS BIGINT), CAST(packs AS BIGINT)
         FROM asv2_before_alloc",
    )?;
    let pairs: Vec<(String, BeforeAllocAgg)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                BeforeAllocAgg {
                    eaches: r.get::<_, i64>(1).unwrap_or(0),
                    packs: r.get::<_, i64>(2).unwrap_or(0),
                },
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_paf_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, PafRow>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(product_code AS VARCHAR), CAST(article AS VARCHAR),
                CAST(l0_name AS VARCHAR), CAST(l1_name AS VARCHAR),
                CAST(l2_name AS VARCHAR), CAST(l3_name AS VARCHAR),
                CAST(l4_name AS VARCHAR), CAST(l5_name AS VARCHAR),
                CAST(brand AS VARCHAR)
         FROM asv2_paf",
    )?;
    let pairs: Vec<(String, PafRow)> = stmt
        .query_map([], |r| {
            let s = |i: usize| -> duckdb::Result<String> {
                r.get::<_, Option<String>>(i).map(|o| o.unwrap_or_default())
            };
            Ok((
                s(0)?,
                PafRow {
                    article: s(1)?,
                    l0_name: s(2)?,
                    l1_name: s(3)?,
                    l2_name: s(4)?,
                    l3_name: s(5)?,
                    l4_name: s(6)?,
                    l5_name: s(7)?,
                    brand: s(8)?,
                },
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_product_dc_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(product_code AS VARCHAR), CAST(dc_codes AS VARCHAR) FROM asv2_product_dc",
    )?;
    let pairs: Vec<(String, Vec<String>)> = stmt
        .query_map([], |r| {
            let pc: String = r.get::<_, Option<String>>(0)?.unwrap_or_default();
            let dcs_str: String = r.get::<_, Option<String>>(1)?.unwrap_or_default();
            let dcs: Vec<String> = dcs_str
                .split('|')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            Ok((pc, dcs))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_store_dc_duckdb(conn: &duckdb::Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT CAST(dc_code AS VARCHAR) FROM raw_store_dc_mapping",
    )?;
    let rows: HashSet<String> = stmt
        .query_map([], |r| {
            r.get::<_, Option<String>>(0).map(|o| o.unwrap_or_default())
        })?
        .collect::<duckdb::Result<HashSet<_>>>()?;
    Ok(rows)
}

fn read_sg_mapping_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(sg_code AS VARCHAR), CAST(store_code AS VARCHAR) FROM raw_store_groups_mapping",
    )?;
    let pairs: Vec<(String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in pairs {
        map.entry(k).or_default().push(v);
    }
    Ok(map)
}

fn read_store_groups_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(sg_code AS VARCHAR), CAST(name AS VARCHAR) FROM raw_store_groups",
    )?;
    let pairs: Vec<(String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_dist_centres_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(dc_code AS VARCHAR), CAST(name AS VARCHAR) FROM raw_distribution_centres",
    )?;
    let pairs: Vec<(String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

fn read_dc_store_rules_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, String>> {
    // Original CSV-parser maps rule_code → values (skipping rule_type at idx 1).
    // `values` is a SQL reserved word in DuckDB, hence the double-quotes.
    let mut stmt = conn.prepare(
        "SELECT CAST(rule_code AS VARCHAR), CAST(\"values\" AS VARCHAR) FROM raw_dc_store_policy_user_rule",
    )?;
    let pairs: Vec<(String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().collect())
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Strip password from a DSN for log output.
fn pg_dsn_redact(dsn: &str) -> String {
    dsn.split_whitespace()
        .filter(|t| !t.starts_with("password="))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn copy_table(dsn: &str, query: &str) -> Result<Vec<u8>> {
    let (client, conn) = tokio_postgres::connect(dsn, NoTls)
        .await
        .with_context(|| format!("PG connect for COPY"))?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::warn!(error=%e, "[article_selection] copy_table connection ended");
        }
    });
    let copy_sql = format!("COPY ({query}) TO STDOUT WITH (FORMAT CSV, HEADER true)");
    let stream = client.copy_out(&copy_sql).await.map_err(|e| {
        if let Some(db_err) = e.as_db_error() {
            anyhow!("COPY {}: {}", db_err.severity(), db_err.message())
        } else { anyhow!("COPY: {}", e) }
    })?;
    let mut data: Vec<u8> = Vec::new();
    let mut s = std::pin::pin!(stream);
    while let Some(chunk) = s.next().await {
        let bytes = chunk.map_err(|e| anyhow!("stream: {}", e))?;
        data.extend_from_slice(&bytes);
    }
    Ok(data)
}

// ── CSV utilities ──────────────────────────────────────────────────────────

fn csv_field(fields: &[&str], idx: usize) -> String {
    fields.get(idx).unwrap_or(&"").trim_matches(|c| c == '{' || c == '}' || c == '"').to_string()
}
fn csv_i64(fields: &[&str], idx: usize) -> i64 {
    fields.get(idx).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}
fn csv_f64(fields: &[&str], idx: usize) -> f64 {
    fields.get(idx).and_then(|s| s.trim().parse().ok()).unwrap_or(0.0)
}
/// Split the `product_codes` column. asv2_ph_master stores them pipe-separated
/// (`25516199|25516205|...`); the legacy V4 PG CSV path used commas (PG array
/// literal `{a,b,c}`). Accept both delimiters so the same helper works for
/// V4 (CSV) and V7 (DuckDB MV) reads.
///
/// Load-bearing: every per-product RCL/SG/DC lookup runs through this
/// iterator. The wrong delimiter turns the whole concatenated string into
/// one bogus key — every lookup misses — and constraint / store_group /
/// dc / allocation_rules collapse to zeros and `[]`.
fn split_product_codes(raw: &str) -> impl Iterator<Item = &str> {
    raw.trim_matches(|c: char| c == '{' || c == '}' || c == '"' || c.is_whitespace())
        .split(|c: char| c == '|' || c == ',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}
fn csv_lines(text: &str) -> Vec<&str> {
    let mut lines = text.lines();
    lines.next();
    lines.filter(|l| !l.is_empty()).collect()
}

// ── Bucket 3 readers ──────────────────────────────────────────────────────

/// `asv2_inventory_per_size_dc` is the DuckDB-built aggregate
/// `(ph_code, size, dc_code, oh, rq)`. Group rows by ph_code so the
/// JSON-format step can emit `{size: {dc: oh}}` per PH directly.
fn read_inv_per_size_dc_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, InvPerSizeDc>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(ph_code AS VARCHAR), CAST(size AS VARCHAR), CAST(dc_code AS VARCHAR),
                CAST(oh AS BIGINT), CAST(rq AS BIGINT)
         FROM asv2_inventory_per_size_dc",
    )?;
    let mut by_ph: HashMap<String, InvPerSizeDc> = HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        ))
    })?;
    for row in rows {
        let (ph, size, dc, oh, rq) = row?;
        by_ph.entry(ph).or_default().push((size, dc, oh, rq));
    }
    Ok(by_ph)
}

/// `raw_last_allocated_details` → `article` → "MM/DD/YYYY". Format matches
/// legacy v2's `TO_CHAR(allocated_time::TIMESTAMP, 'MM/DD/YYYY')`.
fn read_last_allocated_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(article AS VARCHAR), CAST(updated_at AS VARCHAR)
         FROM raw_last_allocated_details",
    )?;
    let mut by_article = HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    })?;
    for row in rows {
        let (article, ts) = row?;
        if article.is_empty() || ts.is_empty() { continue; }
        // ts is like "2026-04-27 13:45:00..." — slice "YYYY-MM-DD" and reformat.
        let formatted = if ts.len() >= 10 && &ts[4..5] == "-" && &ts[7..8] == "-" {
            format!("{}/{}/{}", &ts[5..7], &ts[8..10], &ts[0..4])
        } else { ts };
        by_article.insert(article, formatted);
    }
    Ok(by_article)
}

/// `raw_paf_sizes` → `(article, size)` → size_name. Used to project
/// `size_names` from the PH's sizes column.
fn read_paf_sizes_duckdb(conn: &duckdb::Connection) -> Result<HashMap<(String, String), String>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(article AS VARCHAR), CAST(size AS VARCHAR), CAST(size_name AS VARCHAR)
         FROM raw_paf_sizes",
    )?;
    let mut out = HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    })?;
    for row in rows {
        let (a, s, n) = row?;
        if a.is_empty() || s.is_empty() { continue; }
        out.insert((a, s), n);
    }
    Ok(out)
}

/// Pick the `min_type` from `dc_store_policy_user_rule.rule_code = 1` —
/// legacy v2's fallback when an article's allocation rule doesn't set one.
/// `dc_store_rules` is keyed by rule_code; the value is the JSON `values`
/// column.
fn derive_default_min_type(dc_store_rules: &HashMap<String, String>) -> Option<String> {
    let raw = dc_store_rules.get("1")?;
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get("min_type").and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// `raw_psa_store_map` → `psa_code` → `(l1_name, Vec<store_code>)`. Each
/// PSA in `global.product_store_attributes_filter` is keyed by (l0, l1)
/// — RCL constraint rows for an rcl_code span every l1 the rule covers,
/// but only PSAs matching the product's own l1 should expand to that
/// product's stores. Without this filter we'd inflate constraint
/// aggregation by ~45× (one extra l1 PSA contributing per other l1) and
/// blow out compute time too.
///
/// Joined with `raw_store_master` and filtered to `active = true AND
/// NOT is_deleted` — mirrors legacy v2's `psaf_<uuid>` temp table in
/// `inventory_smart.generate_rcl_l01_constraint_data_subset`.
fn read_psa_store_map_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, (String, Vec<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(psaf.psa_code AS VARCHAR), CAST(psaf.l1_name AS VARCHAR), CAST(psaf.store_code AS VARCHAR) \
         FROM raw_psa_store_map psaf \
         JOIN raw_store_master sm USING (store_code) \
         WHERE sm.active = true AND sm.is_deleted = false",
    )?;
    let mut by_psa: HashMap<String, (String, Vec<String>)> = HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    })?;
    for row in rows {
        let (psa, l1, store) = row?;
        if psa.is_empty() || store.is_empty() { continue; }
        let entry = by_psa.entry(psa).or_insert_with(|| (l1.clone(), Vec::new()));
        entry.1.push(store);
    }
    for (_, stores) in by_psa.values_mut() {
        stores.sort(); stores.dedup();
    }
    Ok(by_psa)
}

/// `product_dc × store_dc` → `product_code` → `HashSet<store_code>` of
/// stores eligible for that product. Mirrors legacy v2's `psm_ph_store`
/// product-store eligibility table.
fn build_product_store_eligibility(
    product_dc: &HashMap<String, Vec<String>>,
    store_dc_set: &HashSet<String>,
    raw_store_dc_dc_to_stores: &HashMap<String, Vec<String>>,
) -> HashMap<String, HashSet<String>> {
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    for (pc, dcs) in product_dc {
        let mut stores = HashSet::new();
        for dc in dcs {
            if !store_dc_set.contains(dc) { continue; }
            if let Some(s) = raw_store_dc_dc_to_stores.get(dc) {
                stores.extend(s.iter().cloned());
            }
        }
        if !stores.is_empty() {
            out.insert(pc.clone(), stores);
        }
    }
    out
}

/// PSM eligibility resolved per product. Mirrors
/// `global.generate_rcl_psm_data(_, 101, current_date)`: walk
/// rcl_codes in priority order ([65538, 33, 16]), find the first whose
/// hash entry on the product matches a `rcl_product_mapping_product_store_rule`
/// dimension, then read eligible store_codes from
/// `rcl_product_mapping_product_store` for that (rcl_code, rule_code).
///
/// All inputs read in Rust (raw_paf_rcl_hash, raw_rcl_psm_rule_dim,
/// raw_rcl_psm_eligibility, raw_rcl_psm_priorities). Earlier we tried to
/// pre-resolve in DuckDB SQL, which produced a 705M-row table — way too
/// many to fit. The per-rcl_rule eligibility map is small (~2570 groups
/// × ~5K stores × ~10B/store ≈ 130MB) and looking up per-product is just
/// a hash lookup.
///
/// Returns `product_code → HashSet<store_code>` of eligible stores. An
/// empty result triggers the V7 assembly's "PSM filter disabled" branch.
fn read_psm_eligible_stores_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, HashSet<String>>> {
    // Bail out if any input table is missing (e.g. fresh tenant before
    // pl_v7_extracts has run with the new extracts). PSM filter then
    // simply doesn't apply.
    let priorities: Vec<(String, i32)> = match conn.prepare(
        "SELECT CAST(rcl_code AS VARCHAR), CAST(priority AS INTEGER) FROM raw_rcl_psm_priorities ORDER BY priority ASC",
    ) {
        Ok(mut s) => match s.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<i32>>(1)?.unwrap_or(0),
            ))
        }) {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => return Ok(HashMap::new()),
        },
        Err(_) => return Ok(HashMap::new()),
    };
    if priorities.is_empty() { return Ok(HashMap::new()); }

    // (rcl_code, dim_md5) → rule_code from rcl_product_mapping_product_store_rule.
    let mut rule_dim: HashMap<(String, String), String> = HashMap::new();
    if let Ok(mut s) = conn.prepare(
        "SELECT CAST(rcl_code AS VARCHAR), CAST(rule_code AS VARCHAR), CAST(dim_md5 AS VARCHAR) FROM raw_rcl_psm_rule_dim",
    ) {
        if let Ok(rows) = s.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        }) {
            for row in rows.flatten() {
                let (rcl, rule, hash) = row;
                if rcl.is_empty() || hash.is_empty() { continue; }
                rule_dim.insert((rcl, hash), rule);
            }
        }
    }

    // (rcl_code, rule_code) → set of eligible store_codes. Each row's
    // psa_code is `<l0>_<store>` — split off the store_code part.
    let mut eligibility: HashMap<(String, String), HashSet<String>> = HashMap::new();
    if let Ok(mut s) = conn.prepare(
        "SELECT CAST(rcl_code AS VARCHAR), CAST(rule_code AS VARCHAR), CAST(psa_code AS VARCHAR) FROM raw_rcl_psm_eligibility",
    ) {
        if let Ok(rows) = s.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        }) {
            for row in rows.flatten() {
                let (rcl, rule, psa) = row;
                if rcl.is_empty() || rule.is_empty() || psa.is_empty() { continue; }
                // Split `<l0>_<store>` at the first `_`.
                let store = match psa.split_once('_') {
                    Some((_, s)) if !s.is_empty() => s.to_string(),
                    _ => continue,
                };
                eligibility.entry((rcl, rule)).or_default().insert(store);
            }
        }
    }

    // Walk product → priority chain → resolve to (rcl_code, rule_code) →
    // look up eligible stores.
    let mut by_pc: HashMap<String, HashSet<String>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT CAST(product_code AS VARCHAR), CAST(rcl_hash AS VARCHAR) FROM raw_paf_rcl_hash",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    })?;
    for row in rows.flatten() {
        let (product_code, rcl_hash_text) = row;
        if product_code.is_empty() || rcl_hash_text.is_empty() { continue; }
        let hash_obj: serde_json::Value = match serde_json::from_str(&rcl_hash_text) {
            Ok(v) => v, Err(_) => continue,
        };
        // First-match-wins through priority list. priorities is already
        // sorted by priority ASC.
        for (rcl, _prio) in &priorities {
            let Some(hash_val) = hash_obj.get(rcl).and_then(|v| v.as_str()) else { continue };
            let Some(rule_code) = rule_dim.get(&(rcl.clone(), hash_val.to_string())) else { continue };
            if let Some(stores) = eligibility.get(&(rcl.clone(), rule_code.clone())) {
                by_pc.insert(product_code.clone(), stores.clone());
                break;
            }
        }
    }
    Ok(by_pc)
}

/// `raw_store_channels` → `store_code` → channel. Active-only stores are
/// kept; inactive stores are dropped (legacy v2's `_store_active_filter`
/// implicitly applies `active=true`).
fn read_store_channels_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(store_code AS VARCHAR), CAST(channel AS VARCHAR), active FROM raw_store_channels WHERE active = true",
    )?;
    let mut out: HashMap<String, String> = HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    })?;
    for row in rows {
        let (store, channel) = row?;
        if store.is_empty() { continue; }
        out.insert(store, channel);
    }
    Ok(out)
}

/// Reverse `raw_store_dc_mapping` → `dc_code → Vec<store_code>`.
fn read_dc_to_stores_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(dc_code AS VARCHAR), CAST(store_code AS VARCHAR) FROM raw_store_dc_mapping",
    )?;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?.unwrap_or_default(),
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    })?;
    for row in rows {
        let (dc, store) = row?;
        if dc.is_empty() || store.is_empty() { continue; }
        out.entry(dc).or_default().push(store);
    }
    Ok(out)
}

/// Build per-PH product profiles JSON. Legacy v2 emits one entry per
/// ia-recommended profile linked via `product_profile_master.ph_code`.
/// The full legacy logic also folds in user-default-profile (udpp) entries
/// and uses an `is_default` flag — we emit the iapp entry with
/// `is_default=true` to cover the dominant case.
fn read_product_profiles_duckdb(conn: &duckdb::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(ph_code AS VARCHAR), CAST(pp_code AS VARCHAR),
                CAST(name AS VARCHAR), CAST(special_classification AS VARCHAR)
         FROM raw_product_profile_master
         WHERE special_classification = 'ia-recommended'
         ORDER BY ph_code, pp_code",
    )?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let mut by_ph: HashMap<String, Vec<String>> = HashMap::new();
    for (ph, pp_code, name, label) in rows {
        if ph.is_empty() { continue; }
        let pp_code_n: i64 = pp_code.parse().unwrap_or(0);
        let entry = format!(
            r#"{{"value":{},"name":"{}","label":"{}","is_default":true}}"#,
            pp_code_n, json_escape(&name), json_escape(&label)
        );
        by_ph.entry(ph).or_default().push(entry);
    }
    Ok(by_ph.into_iter().map(|(k, v)| (k, format!("[{}]", v.join(",")))).collect())
}

// ── Constraint aggregation ─────────────────────────────────────────────────

/// Per-(product_code, store_code) aggregated constraint contributions.
/// Computed once for every product_code in
/// [`precompute_pc_store_contribs`] and reused across PHs that share that
/// product. Without this memoization, the per-PH computation re-expanded
/// each constraint row's PSA → stores ~6×810×700 = 3.4M times, which made
/// the assembly intractable at 48k PHs.
#[derive(Clone, Default)]
struct StoreContrib {
    aps: f64,
    wos_sum: f64, wos_n: u32,
    min_sum: f64, min_n: u32,
    max_sum: f64, max_n: u32,
    min_validator: f64,
    max_validator: f64,
}

impl StoreContrib {
    fn new() -> Self {
        Self { min_validator: f64::INFINITY, max_validator: f64::NEG_INFINITY, ..Self::default() }
    }
    fn add_row(&mut self, r: &rcl::ConstraintRow) {
        self.aps += r.aps;
        self.wos_sum += r.wos; self.wos_n += 1;
        self.min_sum += r.min_stock; self.min_n += 1;
        self.max_sum += r.max_stock; self.max_n += 1;
        if r.max_stock < self.min_validator { self.min_validator = r.max_stock; }
        if r.min_stock > self.max_validator { self.max_validator = r.min_stock; }
    }
    fn merge(&mut self, other: &StoreContrib) {
        self.aps += other.aps;
        self.wos_sum += other.wos_sum; self.wos_n += other.wos_n;
        self.min_sum += other.min_sum; self.min_n += other.min_n;
        self.max_sum += other.max_sum; self.max_n += other.max_n;
        if other.min_validator < self.min_validator { self.min_validator = other.min_validator; }
        if other.max_validator > self.max_validator { self.max_validator = other.max_validator; }
    }
}

/// Pre-compute `product_code → Vec<(store, StoreContrib)>` once.
/// Done in parallel via rayon. Three layered filters mirror legacy v2's
/// PSM eligibility chain:
///   1. PSA × l1 — only PSAs matching the product's l1 (compute scope).
///   2. eligible_stores_by_pc — product-DC × store-DC eligibility.
///   3. psm_eligible_stores_by_pc — `rcl_product_mapping_product_store`
///      lookup via the (rcl_code, rule_code) resolved per-product through
///      module 101's RCL priority chain (see asv2_psm_eligible_stores).
///   4. default_sg_stores_by_pc — stores reachable via
///      `store_groups_mapping` from the product's `default_store_groups`
///      (resolved via DC-policy module 10003 → rcl_codes [183, 2]).
///      Mirrors legacy v2's `_rcl_input_query` join from `store_group esg
///      join sgm on sgm.sg_code = esg.default_sg_code`.
/// When the PSM map is empty (legacy V4 path or fresh tenant), filter (3)
/// is skipped — degrades to permissive but doesn't break assembly. Same
/// for filter (4): empty map = no DC policies resolved → permissive.
fn precompute_pc_store_contribs(
    constraint_by_pc: &HashMap<String, &[rcl::ConstraintRow]>,
    psa_to_stores: &HashMap<String, (String, Vec<String>)>,
    eligible_stores_by_pc: &HashMap<String, HashSet<String>>,
    paf_by_pc: &HashMap<String, PafRow>,
    psm_eligible_stores_by_pc: &HashMap<String, HashSet<String>>,
    default_sg_stores_by_pc: &HashMap<String, HashSet<String>>,
) -> HashMap<String, Vec<(String, StoreContrib)>> {
    let psm_filter_enabled = !psm_eligible_stores_by_pc.is_empty();
    let sg_filter_enabled = !default_sg_stores_by_pc.is_empty();
    constraint_by_pc.par_iter().map(|(pc, rows)| {
        let mut by_store: HashMap<&str, StoreContrib> = HashMap::new();
        let eligible = eligible_stores_by_pc.get(pc);
        let psm_eligible = if psm_filter_enabled {
            psm_eligible_stores_by_pc.get(pc)
        } else {
            None
        };
        let sg_eligible = if sg_filter_enabled {
            default_sg_stores_by_pc.get(pc)
        } else {
            None
        };
        let product_l1 = paf_by_pc.get(pc).map(|p| p.l1_name.as_str());
        for r in *rows {
            let Some((psa_l1, stores)) = psa_to_stores.get(&r.psa_code) else { continue };
            if let Some(plr) = product_l1 {
                if psa_l1 != plr { continue; }
            }
            for store in stores {
                if let Some(elig) = eligible {
                    if !elig.contains(store) { continue; }
                }
                // PSM eligibility — only filter when we have a non-empty
                // map for this product (empty pc entry = no PSM mapping →
                // product is fully ineligible per legacy semantics).
                if psm_filter_enabled {
                    match psm_eligible {
                        Some(set) if set.contains(store) => {}
                        _ => continue,
                    }
                }
                // default_store_groups eligibility — same semantics as PSM.
                if sg_filter_enabled {
                    match sg_eligible {
                        Some(set) if set.contains(store) => {}
                        _ => continue,
                    }
                }
                let entry = by_store.entry(store.as_str()).or_insert_with(StoreContrib::new);
                entry.add_row(r);
            }
        }
        let v: Vec<(String, StoreContrib)> = by_store.into_iter()
            .map(|(s, c)| (s.to_string(), c))
            .collect();
        (pc.clone(), v)
    }).collect()
}

/// `dc_policy_by_pc.default_store_groups` × `sg_mapping` →
/// `product_code → HashSet<store_code>`. Mirrors legacy v2's
/// `_rcl_input_query` filter where the PSM input is restricted to stores
/// in the article's resolved default_store_groups (via DC-policy module
/// 10003, see [`rcl::resolve_dc_policy`]).
///
/// Returns an empty map if no products have a resolved DC policy — caller
/// degrades to permissive.
fn build_default_sg_stores_by_pc(
    dc_policy_by_pc: &HashMap<String, &rcl::DcPolicy>,
    sg_mapping: &HashMap<String, Vec<String>>,
) -> HashMap<String, HashSet<String>> {
    dc_policy_by_pc
        .iter()
        .filter_map(|(pc, pol)| {
            let mut stores = HashSet::new();
            for sg_code in &pol.default_store_groups {
                if let Some(s) = sg_mapping.get(sg_code) {
                    stores.extend(s.iter().cloned());
                }
            }
            if stores.is_empty() { None } else { Some((pc.clone(), stores)) }
        })
        .collect()
}

/// Aggregate constraints per (ph, store) and roll up per ph, mirroring
/// legacy v2's `constraint_data` CTE:
///
/// ```sql
/// inner: SELECT ph, store, SUM(aps), AVG(wos), AVG(min), AVG(max),
///                          MIN(max) AS min_validator, MAX(min) AS max_validator
///        GROUP BY ph, store
/// outer: SELECT ph,        AVG(aps), AVG(wos), AVG(min), AVG(max),
///                          MIN(min_validator), MAX(max_validator),
///                          ARRAY_AGG(store)
///        GROUP BY ph
/// ```
///
/// Reads from the pre-computed `pc_store_contribs` so the per-PH cost is
/// `O(product_codes × per_pc_stores)` instead of re-scanning constraint
/// rows × PSA stores × eligibility for every PH.
///
/// Filters stores by channel (legacy v2's `_store_active_filter` applies
/// `channel = ph.channel`) — without this, V7 includes BFL/etc. stores for
/// a `bls`-channel PH and inflates `mapped_stores` by ~17%.
///
/// Returns `None` when no constraint+store pair resolves, matching legacy's
/// inner-join `JOIN constraint_data` (drops PHs with no constraints).
fn compute_constraints(
    product_codes: &[&str],
    pc_store_contribs: &HashMap<String, Vec<(String, StoreContrib)>>,
    store_channels: &HashMap<String, String>,
    ph_channel: &str,
) -> Option<ConstraintsAgg> {
    let mut by_store: HashMap<String, StoreContrib> = HashMap::new();
    for pc in product_codes {
        if let Some(contribs) = pc_store_contribs.get(*pc) {
            for (store, contrib) in contribs {
                // Legacy v2's `_store_active_filter` keeps only stores whose
                // channel matches the PH's channel (and active=true, which
                // was applied at extract time). Skip non-matching stores so
                // they don't contribute to mapped_stores or the averages.
                if !ph_channel.is_empty() && !store_channels.is_empty() {
                    match store_channels.get(store) {
                        Some(ch) if ch == ph_channel => {}
                        _ => continue,
                    }
                }
                let entry = by_store.entry(store.clone()).or_insert_with(StoreContrib::new);
                entry.merge(contrib);
            }
        }
    }
    if by_store.is_empty() { return None; }

    let n_stores = by_store.len() as f64;
    let mut aps_sum = 0.0; let mut wos_sum = 0.0;
    let mut min_sum = 0.0; let mut max_sum = 0.0;
    let mut outer_min_validator = f64::INFINITY;
    let mut outer_max_validator = f64::NEG_INFINITY;
    let mut stores: Vec<String> = Vec::with_capacity(by_store.len());

    for (store, inner) in &by_store {
        aps_sum += inner.aps;
        if inner.wos_n > 0 { wos_sum += inner.wos_sum / inner.wos_n as f64; }
        if inner.min_n > 0 { min_sum += inner.min_sum / inner.min_n as f64; }
        if inner.max_n > 0 { max_sum += inner.max_sum / inner.max_n as f64; }
        if inner.min_validator < outer_min_validator { outer_min_validator = inner.min_validator; }
        if inner.max_validator > outer_max_validator { outer_max_validator = inner.max_validator; }
        stores.push(store.clone());
    }
    stores.sort();

    Some(ConstraintsAgg {
        aps: aps_sum / n_stores,
        wos: wos_sum / n_stores,
        min_stock: min_sum / n_stores,
        max_stock: max_sum / n_stores,
        min_stock_validator: outer_min_validator,
        max_stock_validator: outer_max_validator,
        mapped_stores_count: stores.len() as i64,
        mapped_stores: stores,
    })
}

/// Treat both PG-style empty literals as missing data, so legacy v2's
/// `null`-on-no-rows semantics are mirrored. Used at the assembly boundary
/// where compute_*_config helpers return `"[]"` / `""`.
fn opt_json(s: String) -> Option<String> {
    if s.is_empty() || s == "[]" { None } else { Some(s) }
}

/// Minimal JSON-string escaper for embedding identifiers into hand-built
/// JSON. Handles `"` and `\`; control chars are rare in our domain.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Convert a PG array-literal-as-text (`{a,b,c}`) into a JSON string-array
/// (`["a","b","c"]`). Used at the row-emit boundary so columns like `sizes`
/// match legacy v2's `sizes` (text[] → JSON array).
fn pg_array_to_json(s: &str) -> Option<String> {
    let trimmed = s.trim_matches(|c: char| c == '{' || c == '}' || c.is_whitespace());
    if trimmed.is_empty() { return None; }
    let parts: Vec<String> = trimmed.split(',')
        .map(|p| p.trim().trim_matches('"'))
        .filter(|p| !p.is_empty())
        .map(|p| format!("\"{}\"", json_escape(p)))
        .collect();
    if parts.is_empty() { None } else { Some(format!("[{}]", parts.join(","))) }
}

/// Split a delimiter-separated list (e.g. pipe-joined product_codes from
/// asv2_ph_master) into a JSON string-array.
fn delim_to_json_array(s: &str, delims: &[char]) -> Option<String> {
    let parts: Vec<String> = s.split(|c: char| delims.contains(&c))
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| format!("\"{}\"", json_escape(p)))
        .collect();
    if parts.is_empty() { None } else { Some(format!("[{}]", parts.join(","))) }
}

/// One row from the asv2_inventory_per_size_dc DuckDB build step:
/// `(size, dc_code, oh, rq)` for a given ph_code. Aggregated up to nested
/// JSON (`{size: {dc: oh}}`) at assembly time.
type InvPerSizeDc = Vec<(String, String, i64, i64)>;

/// Context for [`assemble_row`]. Bundles the dozen+ lookup maps so the
/// shared row-builder can grow new fields without bloating the call sites.
/// V4 (CSV) and V4-scoped (CDC) paths construct this with empty Bucket-3
/// maps; V7 fills them in from extra DuckDB reads.
#[allow(dead_code)]
pub(super) struct AssembleCtx<'a> {
    pub product_dc: &'a HashMap<String, Vec<String>>,
    pub dist_centres: &'a HashMap<String, String>,
    pub store_dc_set: &'a HashSet<String>,
    pub sg_mapping: &'a HashMap<String, Vec<String>>,
    pub store_groups_map: &'a HashMap<String, String>,
    pub dc_store_rules: &'a HashMap<String, String>,
    pub txs_by_ph: &'a HashMap<String, TxsMetrics>,
    pub inv_by_ph: &'a HashMap<String, InventoryAgg>,
    pub woc_by_ph: &'a HashMap<String, WocAgg>,
    pub instock_by_ph: &'a HashMap<String, InstockAgg>,
    pub before_alloc_by_ph: &'a HashMap<String, BeforeAllocAgg>,
    pub dc_policy_by_pc: &'a HashMap<String, &'a rcl::DcPolicy>,
    pub constraint_by_pc: &'a HashMap<String, &'a [rcl::ConstraintRow]>,

    // ── Bucket 3 lookups (empty for V4/V4-scoped) ──────────────────────
    /// `ph_code` → `[(size, dc_code, oh, rq)]`, source of oh_map/rq_map/au_map.
    pub inv_per_size_dc: &'a HashMap<String, InvPerSizeDc>,
    /// `article` → formatted "MM/DD/YYYY" date.
    pub last_allocated: &'a HashMap<String, String>,
    /// `(article, size)` → size_name.
    pub size_name_by_art_size: &'a HashMap<(String, String), String>,
    /// Default min_type from `dc_store_policy_user_rule.rule_code = 1`.
    pub default_min_type: Option<&'a str>,
    /// `ph_code` → JSON-array text of product profile entries.
    pub profiles_by_ph: &'a HashMap<String, String>,
    /// Pre-aggregated per-product constraint contributions per store.
    /// Built once via [`precompute_pc_store_contribs`].
    pub pc_store_contribs: &'a HashMap<String, Vec<(String, StoreContrib)>>,
    /// `store_code → channel` for active stores. Used to filter
    /// `mapped_stores` (and therefore the per-(ph, store) constraint
    /// aggregation) to stores whose channel matches the PH's channel —
    /// legacy v2 applies the same filter via `_store_active_filter`.
    pub store_channels: &'a HashMap<String, String>,
}

/// Build one [`ArticleSelectionRow`] from a [`PhMasterRow`] and an
/// [`AssembleCtx`]. Shared between V4 (PG-CSV), V4-scoped (CDC), and V7
/// (DuckDB) so a future change only has to update one site.
fn assemble_row(
    ph: &PhMasterRow,
    ctx: &AssembleCtx<'_>,
) -> ArticleSelectionRow {
    let product_dc = ctx.product_dc;
    let dist_centres = ctx.dist_centres;
    let store_dc_set = ctx.store_dc_set;
    let sg_mapping = ctx.sg_mapping;
    let store_groups_map = ctx.store_groups_map;
    let dc_store_rules = ctx.dc_store_rules;
    let txs_by_ph = ctx.txs_by_ph;
    let inv_by_ph = ctx.inv_by_ph;
    let woc_by_ph = ctx.woc_by_ph;
    let instock_by_ph = ctx.instock_by_ph;
    let before_alloc_by_ph = ctx.before_alloc_by_ph;
    let dc_policy_by_pc = ctx.dc_policy_by_pc;
    let constraint_by_pc = ctx.constraint_by_pc;
    let product_codes: Vec<&str> = split_product_codes(&ph.product_codes).collect();

    let txs_opt = txs_by_ph.get(&ph.ph_code);
    let inv = inv_by_ph.get(&ph.ph_code).cloned().unwrap_or_default();
    let woc = woc_by_ph.get(&ph.ph_code).cloned().unwrap_or_default();
    let ins_opt = instock_by_ph.get(&ph.ph_code);
    let ba = before_alloc_by_ph.get(&ph.ph_code).cloned().unwrap_or_default();

    let _ = constraint_by_pc;  // shadowing stays for compute_alloc_rules etc.
    let con_opt = compute_constraints(
        &product_codes, ctx.pc_store_contribs,
        ctx.store_channels, &ph.channel,
    );
    let (sg_json, _) = compute_store_groups(&product_codes, dc_policy_by_pc, store_groups_map, sg_mapping);
    let dc_json = compute_dc_config(&product_codes, product_dc, dist_centres, store_dc_set);
    let alloc_json = compute_alloc_rules(&product_codes, dc_policy_by_pc, dc_store_rules);

    let net = inv.oh - inv.reserve_quantity - inv.allocated_units;

    // mapped_stores comes from constraint resolution (per legacy v2's
    // constraint_data CTE), not from the SG-default expansion. When no
    // constraints resolve (PH outside any RCL constraint scope), legacy
    // returns NULL for both mapped_stores and mapped_stores_count.
    let mapped_stores_count = con_opt.as_ref().map(|c| c.mapped_stores_count).unwrap_or(0);

    let ph_code_i64: i64 = ph.ph_code.parse().unwrap_or_default();

    // ── Bucket 3 derived fields ───────────────────────────────────────
    let (oh_map_json, rq_map_json, au_map_json) =
        build_inventory_maps(ctx.inv_per_size_dc.get(&ph.ph_code));
    let last_allocated_v = ctx.last_allocated.get(&ph.article).cloned();
    let size_names_v = build_size_names(&ph.article, &ph.sizes, ctx.size_name_by_art_size);
    // mapped_stores from constraint resolution; None when no constraints
    // resolved (matches legacy LEFT JOIN constraint_data behavior).
    let mapped_stores_v = con_opt.as_ref().and_then(|c|
        if c.mapped_stores.is_empty() { None } else { build_mapped_stores_array(&c.mapped_stores) }
    );
    let min_type_v = extract_min_type(&alloc_json, ctx.default_min_type);
    // Legacy v2 surfaces min_type as a separate column and keeps it OUT of
    // allocation_rules (it returns just the rule body — typically only
    // demand_type). Strip it here to match.
    let alloc_rules_pg_shape = strip_min_type(&alloc_json);
    let product_profiles_v = ctx.profiles_by_ph.get(&ph.ph_code).cloned();
    let product_life_cycle_v = if ph.product_life_cycle.is_empty() {
        String::new()  // serde helper renders empty as null
    } else {
        ph.product_life_cycle.clone()
    };

    // Convert raw text encodings to JSON-array text so the DuckDB column
    // matches legacy v2's array shape (and, with the ser_json_text serde
    // helpers, the gRPC wire format too). Empty input → empty string,
    // which the serde helper renders as null.
    let sizes_json = pg_array_to_json(&ph.sizes).unwrap_or_default();
    let upc_json = delim_to_json_array(&ph.product_codes, &['|', ',']).unwrap_or_default();
    let channel_json = delim_to_json_array(&ph.channel, &[',']).unwrap_or_default();

    ArticleSelectionRow {
        ph_code: ph_code_i64, article: ph.article.clone(),
        l0_name: ph.l0_name.clone(), l1_name: ph.l1_name.clone(),
        l2_name: ph.l2_name.clone(), l3_name: ph.l3_name.clone(),
        l4_name: ph.l4_name.clone(), l5_name: ph.l5_name.clone(),
        style_color_description: ph.style_color_description.clone(),
        product_description: ph.product_description.clone(),
        sizes: sizes_json, upc: upc_json,
        product_life_cycle: product_life_cycle_v,
        article_status_tag: ph.article_status_tag.clone(),
        brand: ph.brand.clone(), channel: channel_json,
        oh: inv.oh, oo: inv.oo, it: inv.it,
        reserve_quantity: inv.reserve_quantity,
        allocated_units: inv.allocated_units,
        net_available_inventory: net,
        oh_map: oh_map_json, rq_map: rq_map_json, au_map: au_map_json,
        last_allocated: last_allocated_v,
        pack_type_id: None,
        lw_units: txs_opt.map(|t| t.lw_units).unwrap_or(0),
        lw_margin: txs_opt.map(|t| t.lw_margin).unwrap_or(0),
        lw_revenue: txs_opt.map(|t| t.lw_revenue).unwrap_or(0),
        price: txs_opt.map(|t| t.price),
        discount: txs_opt.map(|t| t.discount),
        in_stock_perc: ins_opt.map(|i| i.in_stock_perc),
        aps: con_opt.as_ref().map(|c| c.aps),
        min_stock: con_opt.as_ref().map(|c| c.min_stock as i64).unwrap_or(0),
        max_stock: con_opt.as_ref().map(|c| c.max_stock as i64).unwrap_or(0),
        min_stock_validator: con_opt.as_ref().map(|c| c.min_stock_validator as i64).unwrap_or(0),
        max_stock_validator: con_opt.as_ref().map(|c| c.max_stock_validator as i64).unwrap_or(0),
        mapped_stores_count,
        wos: woc.woc as i64, avg_max_mod: woc.avg_max_mod as i64,
        min_woc: woc.min_woc as i64, max_woc: woc.max_woc as i64,
        dcs: opt_json(dc_json), store_groups: opt_json(sg_json),
        beginning_available_to_allocate_eaches: ba.eaches,
        beginning_available_to_allocate_packs: ba.packs,
        allocation_rules: alloc_rules_pg_shape,
        mapped_stores: mapped_stores_v,
        min_type: min_type_v,
        product_profiles: product_profiles_v,
        size_names: size_names_v,
    }
}

/// Strip `min_type` from the allocation_rules JSON. Legacy v2 surfaces
/// `min_type` as its own column and emits the rule body (usually just
/// `demand_type`) as `allocation_rules`. We do the same so the wire format
/// and the DuckDB column shape line up.
fn strip_min_type(s: &str) -> Option<String> {
    if s.is_empty() { return None; }
    let mut v: serde_json::Value = match serde_json::from_str(s) {
        Ok(v) => v, Err(_) => return Some(s.to_string()),
    };
    if let Some(obj) = v.as_object_mut() {
        obj.remove("min_type");
        if obj.is_empty() { return None; }
    }
    serde_json::to_string(&v).ok()
}

/// Build (oh_map, rq_map, au_map) from the per-(size,dc) rows for this PH.
/// Maps are nested JSON objects: `{size: {dc_code: qty}}`. au_map fills
/// zeros — `sku_dc_allocated_units` is a function call we don't extract;
/// most positions are 0 in the legacy output anyway.
fn build_inventory_maps(rows: Option<&InvPerSizeDc>) -> (Option<String>, Option<String>, Option<String>) {
    let Some(rows) = rows else { return (None, None, None); };
    if rows.is_empty() { return (None, None, None); }
    use std::collections::BTreeMap;
    let mut oh_by_size: BTreeMap<&str, BTreeMap<&str, i64>> = BTreeMap::new();
    let mut rq_by_size: BTreeMap<&str, BTreeMap<&str, i64>> = BTreeMap::new();
    let mut au_by_size: BTreeMap<&str, BTreeMap<&str, i64>> = BTreeMap::new();
    for (size, dc, oh, rq) in rows {
        oh_by_size.entry(size.as_str()).or_default().insert(dc.as_str(), *oh);
        rq_by_size.entry(size.as_str()).or_default().insert(dc.as_str(), *rq);
        au_by_size.entry(size.as_str()).or_default().insert(dc.as_str(), 0);
    }
    let to_json = |m: &BTreeMap<&str, BTreeMap<&str, i64>>| -> String {
        let mut out = String::with_capacity(rows.len() * 16);
        out.push('{');
        let mut first_size = true;
        for (size, by_dc) in m {
            if !first_size { out.push(','); } first_size = false;
            out.push_str(&format!("\"{}\":{{", size));
            let mut first_dc = true;
            for (dc, q) in by_dc {
                if !first_dc { out.push(','); } first_dc = false;
                out.push_str(&format!("\"{}\":{}", dc, q));
            }
            out.push('}');
        }
        out.push('}');
        out
    };
    (Some(to_json(&oh_by_size)), Some(to_json(&rq_by_size)), Some(to_json(&au_by_size)))
}

/// Compose `size_names` JSON array from the PH's `sizes` field (PG array
/// literal `{1060,1070,...}`) by looking up each size's display name in the
/// (article, size) map. Order follows the PH's `sizes` field — legacy v2
/// orders by paf.ord, but the PH's sizes column is already produced in
/// canonical order upstream.
fn build_size_names(
    article: &str,
    sizes_raw: &str,
    size_name_by_art_size: &HashMap<(String, String), String>,
) -> Option<String> {
    if size_name_by_art_size.is_empty() { return None; }
    let trimmed = sizes_raw.trim_matches(|c: char| c == '{' || c == '}' || c == '"' || c.is_whitespace());
    let mut names = Vec::new();
    for s in trimmed.split(',') {
        let s = s.trim();
        if s.is_empty() { continue; }
        if let Some(n) = size_name_by_art_size.get(&(article.to_string(), s.to_string())) {
            names.push(format!("\"{}\"", n.replace('"', "\\\"")));
        }
    }
    if names.is_empty() { return None; }
    Some(format!("[{}]", names.join(",")))
}

/// JSON array of store_codes mapped to this PH. Approximation — legacy v2
/// derives from `constraints_resolved × psm_ph_store`, V7 uses the
/// already-deduped `all_stores` from `compute_store_groups`.
fn build_mapped_stores_array(stores: &[String]) -> Option<String> {
    if stores.is_empty() { return None; }
    let parts: Vec<String> = stores.iter().map(|s| format!("\"{}\"", s)).collect();
    Some(format!("[{}]", parts.join(",")))
}

/// Extract `min_type` from the resolved allocation_rules JSON.
/// Falls back to `default_min_type` (from `dc_store_policy_user_rule`
/// where rule_code=1, rule_type='dc-store-rule') when the rule has none.
fn extract_min_type(alloc_json: &str, default_min_type: Option<&str>) -> Option<String> {
    if !alloc_json.is_empty() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(alloc_json) {
            if let Some(s) = v.get("min_type").and_then(|x| x.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    default_min_type.map(|s| s.to_string())
}

// ── Config computation (store_groups / dcs / alloc_rules) ──────────────────

fn compute_store_groups(
    product_codes: &[&str],
    dc_policy: &HashMap<String, &rcl::DcPolicy>,
    store_groups_map: &HashMap<String, String>,
    sg_mapping: &HashMap<String, Vec<String>>,
) -> (String, Vec<String>) {
    let mut all_stores = Vec::new();
    let mut entries: Vec<(String, i64)> = Vec::new();
    for pc in product_codes {
        if let Some(pol) = dc_policy.get(*pc) {
            for sg_code in &pol.default_store_groups {
                if let Some(name) = store_groups_map.get(sg_code) {
                    if let Ok(code) = sg_code.parse::<i64>() {
                        entries.push((name.clone(), code));
                    }
                }
                if let Some(stores) = sg_mapping.get(sg_code) {
                    all_stores.extend(stores.iter().cloned());
                }
            }
        }
    }
    entries.sort(); entries.dedup();
    let parts: Vec<String> = entries.into_iter()
        .map(|(label, code)| format!(r#"{{"value":{},"label":"{}","is_default":true}}"#, code, json_escape(&label)))
        .collect();
    (format!("[{}]", parts.join(",")), all_stores)
}

fn compute_dc_config(
    product_codes: &[&str],
    product_dc: &HashMap<String, Vec<String>>,
    dist_centres: &HashMap<String, String>,
    store_dc_set: &HashSet<String>,
) -> String {
    let mut entries: Vec<(String, i64)> = Vec::new();
    for pc in product_codes {
        if let Some(dcs) = product_dc.get(*pc) {
            for dc in dcs {
                if store_dc_set.contains(dc) {
                    if let Some(name) = dist_centres.get(dc) {
                        if let Ok(code) = dc.parse::<i64>() {
                            entries.push((name.clone(), code));
                        }
                    }
                }
            }
        }
    }
    // Dedup by (label, code) and sort by label — legacy v2's
    // ARRAY_AGG(DISTINCT ...) implicitly sorts by the constructed JSONB,
    // and `label` is the first text-ordering field there.
    entries.sort();
    entries.dedup();
    let parts: Vec<String> = entries.into_iter()
        .map(|(label, code)| format!(r#"{{"value":{},"label":"{}","is_default":true}}"#, code, json_escape(&label)))
        .collect();
    format!("[{}]", parts.join(","))
}

fn compute_alloc_rules(
    product_codes: &[&str],
    dc_policy: &HashMap<String, &rcl::DcPolicy>,
    dc_store_rules: &HashMap<String, String>,
) -> String {
    // Legacy v2 aggregates `MAX(dc_store_rule)` over the PH's product_codes
    // (see `_ph_configuration_mapping` build in article_selection_list_v2).
    // Picking the first match instead diverged on every multi-code PH.
    let mut chosen: Option<&String> = None;
    for pc in product_codes {
        if let Some(pol) = dc_policy.get(*pc) {
            let r = &pol.dc_store_rule;
            if chosen.map(|c| r > c).unwrap_or(true) {
                chosen = Some(r);
            }
        }
    }
    if let Some(rule_code) = chosen {
        if let Some(rules) = dc_store_rules.get(rule_code) {
            return rules.clone();
        }
    }
    String::new()
}

// ── MV parsers ─────────────────────────────────────────────────────────────

fn parse_mv_txs(raw: &[u8]) -> HashMap<String, TxsMetrics> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(7, ',').collect();
        (csv_field(&f, 0), TxsMetrics {
            lw_units: csv_i64(&f, 1), lw_margin: csv_i64(&f, 2), lw_revenue: csv_i64(&f, 3),
            price: csv_f64(&f, 4), discount: csv_f64(&f, 5), in_stock_perc: csv_f64(&f, 6),
        })
    }).collect()
}

fn parse_mv_inventory(raw: &[u8]) -> HashMap<String, InventoryAgg> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(6, ',').collect();
        (csv_field(&f, 0), InventoryAgg {
            oh: csv_i64(&f, 1), oo: csv_i64(&f, 2), it: csv_i64(&f, 3),
            reserve_quantity: csv_i64(&f, 4), allocated_units: csv_i64(&f, 5),
        })
    }).collect()
}

fn parse_mv_woc(raw: &[u8]) -> HashMap<String, WocAgg> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(6, ',').collect();
        (csv_field(&f, 0), WocAgg {
            woc: csv_f64(&f, 1), avg_max_mod: csv_f64(&f, 2),
            min_woc: csv_f64(&f, 3), max_woc: csv_f64(&f, 4),
        })
    }).collect()
}

fn parse_mv_instock(raw: &[u8]) -> HashMap<String, InstockAgg> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(3, ',').collect();
        (csv_field(&f, 0), InstockAgg {
            in_stock_perc: csv_f64(&f, 1), dc_instock: csv_f64(&f, 2),
        })
    }).collect()
}

fn parse_mv_before_alloc(raw: &[u8]) -> HashMap<String, BeforeAllocAgg> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(3, ',').collect();
        (csv_field(&f, 0), BeforeAllocAgg { eaches: csv_i64(&f, 1), packs: csv_i64(&f, 2) })
    }).collect()
}

fn parse_ph_master(raw: &[u8]) -> Vec<PhMasterRow> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(17, ',').collect();
        PhMasterRow {
            ph_code: csv_field(&f, 0), article: csv_field(&f, 1),
            l0_name: csv_field(&f, 2), l1_name: csv_field(&f, 3),
            l2_name: csv_field(&f, 4), l3_name: csv_field(&f, 5),
            l4_name: csv_field(&f, 6), l5_name: csv_field(&f, 7),
            style_color_description: csv_field(&f, 8), product_description: csv_field(&f, 9),
            sizes: csv_field(&f, 10), product_codes: csv_field(&f, 11),
            product_life_cycle: csv_field(&f, 12), article_status_tag: csv_field(&f, 13),
            brand: csv_field(&f, 14), channel: csv_field(&f, 15),
        }
    }).collect()
}

fn parse_paf(raw: &[u8]) -> HashMap<String, PafRow> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(9, ',').collect();
        (csv_field(&f, 0), PafRow {
            article: csv_field(&f, 1), l0_name: csv_field(&f, 2), l1_name: csv_field(&f, 3),
            l2_name: csv_field(&f, 4), l3_name: csv_field(&f, 5),
            l4_name: csv_field(&f, 6), l5_name: csv_field(&f, 7), brand: csv_field(&f, 8),
        })
    }).collect()
}

/// Parses the server-aggregated shape `(product_code, "dc1|dc2|dc3")` produced
/// by `string_agg(dc_code, '|') GROUP BY product_code`. One row per product
/// instead of one per (product, dc) pair — ~50% fewer rows over the wire,
/// and the parser becomes a simple per-line map (no fold/reduce needed).
fn parse_product_dc(raw: &[u8]) -> HashMap<String, Vec<String>> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(2, ',').collect();
        let pc = csv_field(&f, 0);
        let dcs: Vec<String> = f.get(1)
            .map(|s| {
                s.split('|')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        (pc, dcs)
    }).collect()
}

fn parse_store_dc(raw: &[u8]) -> HashSet<String> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().filter_map(|line| {
        line.splitn(2, ',').nth(1).map(|s| s.trim().to_string())
    }).collect()
}

fn parse_sg_mapping(raw: &[u8]) -> HashMap<String, Vec<String>> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter()
        .fold(HashMap::new, |mut map: HashMap<String, Vec<String>>, line| {
            let f: Vec<&str> = line.splitn(2, ',').collect();
            map.entry(csv_field(&f, 0)).or_default().push(csv_field(&f, 1));
            map
        })
        .reduce(HashMap::new, |mut a, b| {
            for (k, v) in b { a.entry(k).or_default().extend(v); }
            a
        })
}

fn parse_store_groups(raw: &[u8]) -> HashMap<String, String> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(3, ',').collect();
        (csv_field(&f, 0), csv_field(&f, 1))
    }).collect()
}

fn parse_dist_centres(raw: &[u8]) -> HashMap<String, String> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(2, ',').collect();
        (csv_field(&f, 0), csv_field(&f, 1))
    }).collect()
}

fn parse_dc_store_rules(raw: &[u8]) -> HashMap<String, String> {
    let text = String::from_utf8_lossy(raw);
    csv_lines(&text).par_iter().map(|line| {
        let f: Vec<&str> = line.splitn(3, ',').collect();
        (csv_field(&f, 0), csv_field(&f, 2)) // rule_code → values
    }).collect()
}

// ─── Scoped recompute (Phase 3 partial_recompute) ────────────────────────────

/// SQL-safe key check. Keys come from CDC (server-side) but we belt-and-suspender
/// validate before splicing into SQL. Allow alphanumerics, underscore, hyphen,
/// dot — covers `ph_code` / `product_code` shapes seen in the asv2_* MVs.
fn is_safe_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 128
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// Build `'a','b','c'` from a list of validated keys.
fn sql_in_list(keys: &[String]) -> String {
    keys.iter()
        .map(|k| format!("'{}'", k))
        .collect::<Vec<_>>()
        .join(",")
}

/// Recompute `Vec<ArticleSelectionRow>` for the given `ph_codes` only.
///
/// Scopes the 6 ph_code-keyed asv2_* MV reads with `WHERE ph_code IN (...)`,
/// resolves the affected `product_code`s from the scoped ph_master, then reads
/// `asv2_paf` / `asv2_product_dc` scoped to those product_codes. The 5 config
/// tables (store_dc, store_groups, sg_mapping, distribution_centres,
/// dc_store_policy_user_rule) are read in full — they're small dimension
/// tables and not worth the round-trip to scope.
///
/// Returns rows only for the requested `ph_codes`. Caller is responsible for
/// applying them surgically (DELETE+INSERT / in-memory replace) — this
/// function does NOT replace the full result set.
pub async fn extract_and_assemble_scoped(
    pg_default_dsn: &str,
    ruleset: Arc<rcl::RuleSet>,
    ph_codes: &[String],
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<ExtractionResult> {
    if ph_codes.is_empty() {
        return Ok(ExtractionResult { rows: Vec::new(), total_ms: 0 });
    }
    for k in ph_codes {
        if !is_safe_key(k) {
            return Err(anyhow!("scoped recompute: invalid ph_code '{}'", k));
        }
    }
    let bail = || -> Result<()> {
        if cancel.is_cancelled() {
            Err(anyhow!("article_selection scoped: cancelled"))
        } else {
            Ok(())
        }
    };
    let ph_in = sql_in_list(ph_codes);
    let total_start = Instant::now();

    // Phase 1: pull ph_master scoped — needed first to learn product_codes.
    let ph_raw = copy_table(
        pg_default_dsn,
        &format!(
            "SELECT ph_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, \
                    style_color_description, product_description, sizes, product_codes, \
                    product_lifecycle, article_status_tag, brand, channel \
             FROM inventory_smart.asv2_ph_master WHERE ph_code IN ({ph_in})"
        ),
    )
    .await
    .with_context(|| "scoped: asv2_ph_master")?;
    let ph_master = parse_ph_master(&ph_raw);
    if ph_master.is_empty() {
        tracing::info!(
            "[article_selection] scoped: no ph_master rows for {} keys; nothing to do",
            ph_codes.len()
        );
        return Ok(ExtractionResult {
            rows: Vec::new(),
            total_ms: total_start.elapsed().as_millis(),
        });
    }

    // Resolve affected product_codes from the scoped ph_master.
    let mut product_codes_set: HashSet<String> = HashSet::new();
    for ph in &ph_master {
        for pc in split_product_codes(&ph.product_codes) {
            if is_safe_key(pc) {
                product_codes_set.insert(pc.to_string());
            }
        }
    }
    let product_codes: Vec<String> = product_codes_set.into_iter().collect();
    let pc_in = if product_codes.is_empty() {
        // No products → paf / product_dc / store_dc still need to NOT match
        // anything; '__none__' is a safe sentinel that won't appear in the data.
        "'__none__'".to_string()
    } else {
        sql_in_list(&product_codes)
    };

    // Phase 2: parallel COPY of remaining MVs (5 ph-keyed) + 2 product-keyed
    // + 5 config tables in full. Pre-format SQL strings so they outlive the
    // try_join! await boundary (the futures borrow these &strs).
    let q_txs = format!("SELECT ph_code, lw_units, lw_margin, lw_revenue, price, discount, in_stock_perc FROM inventory_smart.asv2_txs_metrics WHERE ph_code IN ({ph_in})");
    let q_inv = format!("SELECT ph_code, oh, oo, it, reserve_quantity, allocated_units FROM inventory_smart.asv2_inventory WHERE ph_code IN ({ph_in})");
    let q_woc = format!("SELECT ph_code, woc, avg_max_mod, min_woc, max_woc, woc_mapped_stores_count FROM inventory_smart.asv2_woc WHERE ph_code IN ({ph_in})");
    let q_instock = format!("SELECT ph_code, in_stock_perc, dc_instock FROM inventory_smart.asv2_instock WHERE ph_code IN ({ph_in})");
    let q_before_alloc = format!("SELECT ph_code, eaches, packs FROM inventory_smart.asv2_before_alloc WHERE ph_code IN ({ph_in})");
    let q_paf = format!("SELECT product_code, article, l0_name, l1_name, l2_name, l3_name, l4_name, l5_name, brand FROM inventory_smart.asv2_paf WHERE product_code IN ({pc_in})");
    let q_product_dc = format!("SELECT product_code, string_agg(dc_code, '|') AS dc_codes FROM inventory_smart.asv2_product_dc WHERE product_code IN ({pc_in}) GROUP BY product_code");

    let (
        txs_raw,
        inv_raw,
        woc_raw,
        instock_raw,
        before_alloc_raw,
        paf_raw,
        product_dc_raw,
        store_dc_raw,
        store_groups_raw,
        sg_mapping_raw,
        dist_centres_raw,
        dc_store_rule_raw,
    ) = tokio::try_join!(
        copy_table(pg_default_dsn, &q_txs),
        copy_table(pg_default_dsn, &q_inv),
        copy_table(pg_default_dsn, &q_woc),
        copy_table(pg_default_dsn, &q_instock),
        copy_table(pg_default_dsn, &q_before_alloc),
        copy_table(pg_default_dsn, &q_paf),
        copy_table(pg_default_dsn, &q_product_dc),
        copy_table(pg_default_dsn, "SELECT store_code, dc_code FROM global.product_mapping_store_dc WHERE is_active=true"),
        copy_table(pg_default_dsn, "SELECT sg_code, name, is_deleted FROM global.store_groups WHERE is_deleted=false"),
        copy_table(pg_default_dsn, "SELECT sg_code, store_code FROM global.store_groups_mapping"),
        copy_table(pg_default_dsn, "SELECT dc_code, name FROM global.distribution_centres WHERE is_active=true AND is_deleted=false"),
        copy_table(pg_default_dsn, "SELECT rule_code, rule_type, values FROM inventory_smart.dc_store_policy_user_rule"),
    ).map_err(|e| anyhow!("scoped COPY extraction failed: {}", e))?;

    let copy_ms = total_start.elapsed().as_millis();
    tracing::info!(
        "[article_selection] scoped COPY done in {}ms ({} ph_codes, {} product_codes)",
        copy_ms, ph_codes.len(), product_codes.len()
    );
    bail()?;

    // Parse (same helpers as full path).
    let txs_by_ph = parse_mv_txs(&txs_raw);
    let inv_by_ph = parse_mv_inventory(&inv_raw);
    let woc_by_ph = parse_mv_woc(&woc_raw);
    let instock_by_ph = parse_mv_instock(&instock_raw);
    let before_alloc_by_ph = parse_mv_before_alloc(&before_alloc_raw);
    let paf_by_pc = parse_paf(&paf_raw);
    let product_dc = parse_product_dc(&product_dc_raw);
    let store_dc_set: HashSet<String> = parse_store_dc(&store_dc_raw);
    let sg_mapping = parse_sg_mapping(&sg_mapping_raw);
    let store_groups_map = parse_store_groups(&store_groups_raw);
    let dist_centres = parse_dist_centres(&dist_centres_raw);
    let dc_store_rules = parse_dc_store_rules(&dc_store_rule_raw);

    // RCL resolution restricted to the products we actually need.
    let products: Vec<rcl::ProductHierarchy<'_>> = paf_by_pc
        .iter()
        .map(|(pc, paf)| rcl::ProductHierarchy {
            product_code: pc,
            l0_name: &paf.l0_name,
            l1_name: &paf.l1_name,
            l2_name: &paf.l2_name,
            l3_name: &paf.l3_name,
            l4_name: &paf.l4_name,
            l5_name: &paf.l5_name,
            brand: &paf.brand,
        })
        .collect();
    let dc_policy_by_pc = rcl::resolve_dc_policy(&ruleset, &products);
    let constraint_by_pc = rcl::resolve_constraints(&ruleset, &products);
    bail()?;

    // Assemble (rayon-parallel even for small inputs — cheap when small).
    let active_ph: Vec<&PhMasterRow> = ph_master.iter().collect();
    let empty_inv: HashMap<String, InvPerSizeDc> = HashMap::new();
    let empty_la: HashMap<String, String> = HashMap::new();
    let empty_sn: HashMap<(String, String), String> = HashMap::new();
    let empty_pp: HashMap<String, String> = HashMap::new();
    let empty_pc_contribs: HashMap<String, Vec<(String, StoreContrib)>> = HashMap::new();
    let empty_chan: HashMap<String, String> = HashMap::new();
    let ctx = AssembleCtx {
        product_dc: &product_dc, dist_centres: &dist_centres, store_dc_set: &store_dc_set,
        sg_mapping: &sg_mapping, store_groups_map: &store_groups_map,
        dc_store_rules: &dc_store_rules,
        txs_by_ph: &txs_by_ph, inv_by_ph: &inv_by_ph, woc_by_ph: &woc_by_ph,
        instock_by_ph: &instock_by_ph, before_alloc_by_ph: &before_alloc_by_ph,
        dc_policy_by_pc: &dc_policy_by_pc, constraint_by_pc: &constraint_by_pc,
        // Scoped CDC path doesn't extract Bucket-3 sources — the rows that
        // are actually rewritten will have B3 fields = None until the next
        // full V7 build. Acceptable since CDC keys are small.
        inv_per_size_dc: &empty_inv,
        last_allocated: &empty_la,
        size_name_by_art_size: &empty_sn,
        default_min_type: None,
        profiles_by_ph: &empty_pp,
        pc_store_contribs: &empty_pc_contribs,
        store_channels: &empty_chan,
    };
    let rows: Vec<ArticleSelectionRow> = active_ph
        .par_iter()
        .map(|ph| assemble_row(ph, &ctx))
        .collect();

    let total_ms = total_start.elapsed().as_millis();
    tracing::info!(
        "[article_selection] scoped assembly: {} rows for {} keys in {}ms",
        rows.len(), ph_codes.len(), total_ms
    );
    Ok(ExtractionResult { rows, total_ms })
}
