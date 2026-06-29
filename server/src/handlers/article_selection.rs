//! HTTP entry point for the article-selection materializer.
//!
//! `POST /api/article-selection/materialize` runs the V7 DuckDB assembler:
//! it reads the `asv2_*` + `raw_*` tables already materialized in
//! `tenant_data.duckdb` (by the `pl_v7_extracts` + `pl_v7_build` pipelines),
//! resolves RCL via the in-process [`rcl::RuleStore`] snapshot, and writes
//! the assembled rows back into `tenant_data.duckdb::article_selection`.
//! The dataview row `dv_article_selection` (created on first run) reads
//! from there via its `source = duckdb_table` config.

use std::sync::Arc;
use std::time::Instant;

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::AppState;
use crate::article_selection::{extract_and_assemble_from_duckdb, materialize_to_duckdb};

use super::err;

const DATAVIEW_ID: &str = "dv_article_selection";
const DATAVIEW_DISPLAY_NAME: &str = "Article Selection";

/// `POST /api/article-selection/materialize`
///
/// Returns `{ rows, extract_ms, duckdb_ms, total_ms, dataview_id }`.
pub async fn materialize(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let total_start = Instant::now();

    // 1. Resolve in-process RCL rule snapshot.
    let store = {
        let guard = state.rcl_store.read().await;
        guard.clone()
    };
    let store = match store {
        Some(s) => s,
        None => return Err(err(503, "RCL service is not running. Set [rcl] enabled = true in environment.toml and restart.")),
    };
    let ruleset = store.snapshot();

    // 2. Run V7 assembler off the tokio runtime — DuckDB reads + rayon work are
    // blocking. HTTP path is non-cancellable today; pass a fresh token so the
    // extractor signature is satisfied. (Pipeline-driven runs go through
    // pipeline_v2 and get the live cancel token from `AppState.active_run`.)
    let duckdb_path_for_extract = state.duckdb_path.clone();
    let ruleset_for_extract = ruleset.clone();
    let cancel = tokio_util::sync::CancellationToken::new();
    let extract = match tokio::task::spawn_blocking(move || {
        extract_and_assemble_from_duckdb(&duckdb_path_for_extract, ruleset_for_extract, &cancel)
    })
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(err(500, &format!("extract failed: {e:#}"))),
        Err(e) => return Err(err(500, &format!("task join: {e}"))),
    };
    let extract_ms = extract.total_ms;

    // 4. Materialize to DuckDB. Blocking call; spawn_blocking it.
    let _guard = state.pipeline_run_lock.clone().lock_owned().await;
    let duckdb_path = state.duckdb_path.clone();
    let rows = extract.rows;
    let mat = match tokio::task::spawn_blocking(move || materialize_to_duckdb(&duckdb_path, &rows)).await {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => return Err(err(500, &format!("materialize failed: {e:#}"))),
        Err(e) => return Err(err(500, &format!("task join: {e}"))),
    };

    // 5. Upsert the dataview row so it points at the materialized table.
    if let Err(e) = upsert_dataview(&state) {
        tracing::warn!(error=%e, "[article_selection] dataview upsert failed (table is materialized; UI may not see it)");
    }

    // 6. Stamp the src_article_selection row so /status reports it as fresh.
    // Mirrors pipeline_v2::update_source_lineage's convention.
    if let Err(e) = state.db.execute(
        "UPDATE sources SET status = 'populated', last_populated_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
        &[&"src_article_selection" as &dyn rusqlite::types::ToSql],
    ) {
        tracing::warn!(error=%e, "[article_selection] sources status update failed");
    }

    let total_ms = total_start.elapsed().as_millis() as u64;
    Ok(Json(json!({
        "dataview_id": DATAVIEW_ID,
        "rows": mat.rows_written,
        "extract_ms": extract_ms,
        "duckdb_ms": mat.duckdb_ms,
        "total_ms": total_ms,
        "rcl_version": ruleset.version,
    })))
}

/// Insert the dataview row if it doesn't exist, with `source = duckdb_table`
/// and the 46-column projection. Idempotent.
fn upsert_dataview(state: &AppState) -> anyhow::Result<()> {
    let columns = column_metadata();
    let source = json!({
        "type": "duckdb_table",
        "config": { "table_name": "article_selection" }
    });
    let columns_json = serde_json::to_string(&columns)?;
    let source_json = serde_json::to_string(&source)?;

    // INSERT OR IGNORE — preserves any UI-side overrides on subsequent runs.
    state.db.execute(
        "INSERT OR IGNORE INTO dataviews (id, display_name, columns, source) VALUES (?1, ?2, ?3, ?4)",
        &[
            &DATAVIEW_ID as &dyn rusqlite::types::ToSql,
            &DATAVIEW_DISPLAY_NAME as _,
            &columns_json as _,
            &source_json as _,
        ],
    )?;
    Ok(())
}

/// 46-column metadata matching `materialize::CREATE_SQL`.
fn column_metadata() -> Vec<Value> {
    fn col(name: &str, ty: &str) -> Value {
        json!({"name": name, "type": ty, "visible": true, "sortable": true, "searchable": false})
    }
    vec![
        col("ph_code", "VARCHAR"), col("article", "VARCHAR"),
        col("l0_name", "VARCHAR"), col("l1_name", "VARCHAR"),
        col("l2_name", "VARCHAR"), col("l3_name", "VARCHAR"),
        col("l4_name", "VARCHAR"), col("l5_name", "VARCHAR"),
        col("style_color_description", "VARCHAR"), col("product_description", "VARCHAR"),
        col("sizes", "VARCHAR"), col("upc", "VARCHAR"),
        col("product_life_cycle", "VARCHAR"), col("article_status_tag", "VARCHAR"),
        col("brand", "VARCHAR"), col("channel", "VARCHAR"),
        col("oh", "BIGINT"), col("oo", "BIGINT"), col("it", "BIGINT"),
        col("reserve_quantity", "BIGINT"), col("allocated_units", "BIGINT"),
        col("net_available_inventory", "BIGINT"),
        col("oh_map", "VARCHAR"), col("rq_map", "VARCHAR"), col("au_map", "VARCHAR"),
        col("last_allocated", "VARCHAR"),
        col("lw_units", "BIGINT"), col("lw_margin", "BIGINT"), col("lw_revenue", "BIGINT"),
        col("price", "DOUBLE"), col("discount", "DOUBLE"), col("in_stock_perc", "DOUBLE"),
        col("aps", "DOUBLE"), col("min_stock", "BIGINT"), col("max_stock", "BIGINT"),
        col("min_stock_validator", "BIGINT"), col("max_stock_validator", "BIGINT"),
        col("mapped_stores_count", "BIGINT"),
        col("wos", "BIGINT"), col("avg_max_mod", "BIGINT"),
        col("min_woc", "BIGINT"), col("max_woc", "BIGINT"),
        col("dcs", "VARCHAR"), col("store_groups", "VARCHAR"),
        col("beginning_available_to_allocate_eaches", "BIGINT"),
        col("beginning_available_to_allocate_packs", "BIGINT"),
        col("allocation_rules", "VARCHAR"),
    ]
}

