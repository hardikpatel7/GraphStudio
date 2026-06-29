//! Shared-pipeline runner that delegates to the `pipeline` crate from
//! rust-shared-utils. Replaces `pipeline_handler::run_shared_pipeline_stream`.
//!
//! Translates smartstudio's stored JSON shape into the crate's typed `Step`
//! enum, runs the pipeline against a tmp DuckDB, then merges the resulting
//! tables into the tenant's persistent `tenant_data.duckdb`. We don't use the
//! crate's atomic-swap mode because shared pipelines accumulate tables across
//! runs — swapping the whole file would clobber tables produced by other
//! pipelines.
//!
//! Step types supported by the crate (and therefore by this handler):
//!   - pg_extract       (target: parquet | duckdb | memory)
//!   - duckdb_query
//!   - duckdb_table     (load a parquet file into a DuckDB table)
//!
//! Step types NOT supported here (legacy executor in `crate::pipeline` still
//! handles them via the deprecated dataview-pipeline routes):
//!   loop, bq_export, gcs_download, grpc_call

use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::{anyhow, Result};

use crate::AppState;
use super::err;

// ----------------------------------------------------------------------------
// Reusable pipeline run helper.
//
// Both the SSE handler (`run_stream`) and the in-process scheduler (Phase 2)
// drive a pipeline run the same way: load JSON → translate → execute against
// a tmp DuckDB → merge into the tenant DB. The shared logic lives in
// `execute_pipeline_run`. The SSE handler bridges events to the wire; the
// scheduler drops them.
// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PipelineRunOptions {
    pub pipeline_id: String,
    pub skip_ids: HashSet<String>,
    pub mode_progress_interval: Option<Duration>,
    pub mode_quantify: bool,
    /// Free-form attribution: `"manual"`, `"cdc:src_id"`, `"rcl_change"`,
    /// `"scheduled"`. Recorded in the activity log so operators can tell
    /// what kicked off the run.
    pub trigger_source: String,
    /// Phase 3: keys that changed since the last run, forwarded into the
    /// CustomRust assembly via `AssemblyDeps::partial_recompute_keys`. Empty
    /// = full recompute (default for manual / scheduled / RCL-triggered runs).
    pub partial_recompute_keys: Vec<String>,
    /// When true, all top-level steps are wrapped in a single
    /// `Step::Group{Parallel}` so the executor fires them concurrently.
    /// Default false = top-level implicit sequence (existing behavior).
    /// Selected per-run from the UI toggle / `?execution=parallel` query param.
    pub parallel: bool,
}

#[derive(Debug)]
pub struct PipelineRunReport {
    pub success: bool,
    pub total_ms: u64,
    pub total_rows: i64,
    pub skipped_count: usize,
    pub error: Option<String>,
    pub nodes: Vec<Value>,
    /// Tables merged from the tmp DuckDB into the tenant DB on success.
    pub merged_tables: Vec<String>,
}

/// Drive a pipeline run end-to-end: load definition, translate, execute,
/// merge, log. `event_tx` is the channel the executor pushes step events to;
/// the caller decides whether to bridge them to SSE, log them, or drop them.
///
/// Must be called on a thread with a tokio runtime where `!Send` futures can
/// be polled — i.e. the same `std::thread::spawn` + `block_on` pattern
/// `run_stream` uses. Acquires the tenant pipeline run lock internally.
pub async fn execute_pipeline_run(
    state: Arc<AppState>,
    opts: PipelineRunOptions,
    event_tx: tokio::sync::mpsc::UnboundedSender<pipeline::StepEvent>,
) -> PipelineRunReport {
    let pipeline_id = opts.pipeline_id.clone();
    let trigger_source = opts.trigger_source.clone();

    // 1. Load stored pipeline JSON + placement (no lock needed — read-only).
    let row = match state.db.query_one(
        "SELECT * FROM pipelines WHERE id = ?1",
        &[&pipeline_id as &dyn rusqlite::types::ToSql],
    ) {
        Ok(r) => r,
        Err(_) => return PipelineRunReport::failure(0, "Pipeline not found", vec![]),
    };
    let placement = parse_placement(row.get("placement"));
    let raw = row.get("pipeline").cloned().unwrap_or_else(|| json!("[]"));
    let nodes_array: Vec<Value> = match raw {
        Value::String(s) => serde_json::from_str(&s).unwrap_or_default(),
        Value::Array(arr) => arr,
        _ => Vec::new(),
    };
    let nodes: Vec<Value> = nodes_array.into_iter()
        .filter(|n| {
            let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("");
            !opts.skip_ids.contains(id)
        })
        .collect();

    // Tenant-write fast path: a pipeline that has zero pg_extract steps
    // doesn't need the tmp-scratch + merge dance. Run its duckdb_query and
    // custom_rust steps directly against tenant_data.duckdb so SQL can
    // reference local tables by their plain names. Each branch owns its
    // own pipeline_run_lock acquisition (don't take it here yet).
    let any_pg_extract = nodes.iter().any(|n| {
        n.get("type").and_then(|v| v.as_str()) == Some("pg_extract")
    });
    if !any_pg_extract {
        return execute_tenant_write_pipeline(
            state.clone(),
            opts.clone(),
            event_tx,
            nodes,
            placement,
        ).await;
    }

    let _lock = match acquire_run_lock(&state).await {
        Ok(g) => g,
        Err(msg) => return PipelineRunReport::failure(0, &msg, nodes),
    };
    let _run_handle = ActiveRunGuard::register(&state, &pipeline_id).await;

    // 2. Translate to typed steps.
    let typed_steps = match json_to_steps(&nodes) {
        Ok(v) => v,
        Err(e) => return PipelineRunReport::failure(0, &format!("Pipeline translation: {}", e), nodes),
    };
    if typed_steps.is_empty() {
        return PipelineRunReport::failure(0, "No pipeline tree defined", nodes);
    }

    // 3. Build the typed Pipeline<Ready>. When `opts.parallel` is set,
    // restructure for concurrency:
    //
    //   The crate's `execute_parallel_group` only spawns `Step::PgExtract`
    //   children as concurrent tasks; `Step::Group{Sequence}` and other
    //   leaves run inline. `json_to_steps` wraps each pg_extract+load into a
    //   `Step::sequence()` so the load waits for its extract — but that
    //   wrapping turns each pair into a non-PgExtract child, defeating
    //   parallel mode.
    //
    //   Fix: in parallel mode, unwrap the Seq(extract, load) pairs and emit
    //
    //     [Parallel(extracts…), load_1, load_2, …, other_steps…]
    //
    //   so extracts run concurrently against PG, then loads (which serialize
    //   on the shared DuckDB writer) run after, and custom_rust / other
    //   steps follow. Anything that isn't an `extract+load` pair flows
    //   through unchanged.
    let typed_steps = if opts.parallel && typed_steps.len() > 1 {
        let mut extracts: Vec<pipeline::Step> = Vec::new();
        let mut loads: Vec<pipeline::Step> = Vec::new();
        let mut others: Vec<pipeline::Step> = Vec::new();
        for step in typed_steps {
            if let pipeline::Step::Group { kind: pipeline::GroupKind::Sequence, children } = &step {
                if children.len() == 2
                    && matches!(children[0], pipeline::Step::PgExtract { .. })
                    && matches!(children[1], pipeline::Step::DuckDbLoad { .. })
                {
                    let pipeline::Step::Group { children, .. } = step else { unreachable!() };
                    let mut iter = children.into_iter();
                    extracts.push(iter.next().unwrap());
                    loads.push(iter.next().unwrap());
                    continue;
                }
            }
            others.push(step);
        }
        let mut out: Vec<pipeline::Step> = Vec::new();
        if extracts.len() > 1 {
            out.push(pipeline::Step::Group {
                kind: pipeline::GroupKind::Parallel,
                children: extracts,
            });
        } else {
            out.extend(extracts);
        }
        out.extend(loads);
        out.extend(others);
        out
    } else {
        typed_steps
    };
    let mut iter = typed_steps.into_iter();
    let first = iter.next().unwrap();
    let parquet_home = state.parquet_home.clone();
    let duckdb_output_name = format!("pipeline_run_{}", pipeline_id);
    let p_configured = pipeline::Pipeline::new(&pipeline_id)
        .with_duckdb_output(&duckdb_output_name)
        .with_output_placement(placement);
    let mut p_with_steps = p_configured.add_step(first);
    for s in iter { p_with_steps = p_with_steps.add_step(s); }
    let ready = p_with_steps.build();

    // 4. Build ExecutionContext.
    let pg_pool_name = resolve_pg_pool_name(&state).unwrap_or_default();
    let connection_map = build_connection_map(&state);
    let assembly_dispatcher: Arc<dyn pipeline::AssemblyDispatcher> = Arc::new(
        crate::pipeline_assemblies::PipelineAssemblyRegistry::new(state.clone()),
    );
    // Clone the event sender so we can emit merge-phase StepEvents AFTER
    // the pipeline crate's ctx is dropped. The bridge in `run_stream`
    // exits only once every clone is dropped, so the merge step's events
    // flow through SSE before `pipeline_done` fires.
    let merge_event_tx = event_tx.clone();
    let ctx = pipeline::ExecutionContext {
        pg_pool_name,
        parquet_home: parquet_home.clone(),
        data_dir: state.data_dir.clone(),
        event_tx,
        connection_map,
        progress_interval: opts.mode_progress_interval,
        quantify: opts.mode_quantify,
        assembly_dispatcher: Some(assembly_dispatcher),
        partial_recompute_keys: opts.partial_recompute_keys.clone(),
        tenant_attach_path: Some(state.duckdb_path.clone()),
        cancel: _run_handle.cancel(),
    };

    // 5. Run.
    let t = Instant::now();
    let exec_result = ready.execute(&ctx).await;
    drop(ctx); // releases the crate's event_tx clone
    let total_ms = t.elapsed().as_millis() as u64;

    match exec_result {
        Ok(result) => {
            // 6. Merge tmp DuckDB into tenant.
            let mut merged_tables = Vec::new();
            if let Some(writer) = result.writer {
                let tmp_path = writer.path.clone();
                if let Err(e) = pipeline::DuckDbManager::checkpoint(writer) {
                    return PipelineRunReport::failure(total_ms, &format!("checkpoint tmp: {}", e), nodes);
                }
                match merge_tmp_into_persistent(&state.duckdb_path, &tmp_path, &merge_event_tx, &_run_handle.cancel()) {
                    Ok(tables) => {
                        tracing::info!(
                            pipeline = %pipeline_id, tables = ?tables,
                            "Merged pipeline output into tenant DuckDB"
                        );
                        merged_tables = tables;
                    }
                    Err(e) => {
                        return PipelineRunReport::failure(total_ms, &format!("merge tmp → tenant: {}", e), nodes);
                    }
                }
            }
            // Drop the merge-phase sender so the bridge exits and the
            // run_stream caller can emit `pipeline_done` cleanly.
            drop(merge_event_tx);

            update_source_lineage(&state, &pipeline_id, &nodes);

            // Phase 3: post-run in-memory rehydration. If placement asked for
            // it AND this run produced an `article_selection` table, reload
            // the in-memory store from the freshly-merged DuckDB.
            if matches!(result.placement, pipeline::Placement::DuckDbAndInMemory)
                && merged_tables.iter().any(|t| t == "article_selection")
            {
                rehydrate_article_selection_store(&state, &pipeline_id);
            }

            // Time-to-cook is total_ms here. Trigger source is recorded in the
            // activity message so operators can distinguish push vs manual runs.
            state.traces.log_activity(
                &state.tenant_id, "shared_pipeline", &pipeline_id, "success",
                &format!("trigger={} time-to-cook={}ms", trigger_source, total_ms),
                None, Some(total_ms as i64),
            ).ok();

            PipelineRunReport {
                success: true,
                total_ms,
                total_rows: result.total_rows,
                skipped_count: result.steps_skipped,
                error: None,
                nodes,
                merged_tables,
            }
        }
        Err(e) => {
            let msg = e.to_string();
            update_source_lineage_failed(&state, &pipeline_id, &nodes, &msg);
            state.traces.log_error(
                &state.tenant_id, &format!("shared_pipeline:{}", pipeline_id), &msg, "",
            ).ok();
            PipelineRunReport::failure(total_ms, &msg, nodes)
        }
    }
}

/// Try to acquire the tenant-wide pipeline run lock with a short timeout.
/// Returns a clear error message instead of hanging silently when an earlier
/// run is still active. The held run can be cancelled mid-step via
/// `POST /api/pipelines/cancel` — that drops PG COPY streams and interrupts
/// in-flight DuckDB statements via [`pipeline::ExecutionContext::cancel`].
async fn acquire_run_lock(
    state: &Arc<AppState>,
) -> std::result::Result<tokio::sync::OwnedMutexGuard<()>, String> {
    let attempt = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        state.pipeline_run_lock.clone().lock_owned(),
    ).await;
    match attempt {
        Ok(g) => Ok(g),
        Err(_) => Err(
            "Another pipeline run is still active server-side. Cancel it via POST /api/pipelines/cancel or wait for it to finish.".to_string()
        ),
    }
}

/// RAII handle for the active-run registry on AppState. Drop clears the slot
/// so a subsequent run sees no active run. The held `cancel` token is what
/// the cancel endpoint fires to abort the run mid-step — it's identical to
/// the token plumbed into `pipeline::ExecutionContext::cancel` and
/// `pipeline::AssemblyDeps::cancel`.
pub struct ActiveRunGuard {
    state: Arc<AppState>,
    cancel: tokio_util::sync::CancellationToken,
}

impl ActiveRunGuard {
    pub async fn register(state: &Arc<AppState>, pipeline_id: &str) -> Self {
        let cancel = tokio_util::sync::CancellationToken::new();
        {
            let mut slot = state.active_run.write().await;
            *slot = Some(crate::ActiveRun {
                pipeline_id: pipeline_id.to_string(),
                started_at: std::time::Instant::now(),
                cancel: cancel.clone(),
            });
        }
        Self { state: state.clone(), cancel }
    }

    pub fn cancel(&self) -> tokio_util::sync::CancellationToken {
        self.cancel.clone()
    }
}

impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        // Clear the slot. spawn_blocking off the runtime since Drop is sync.
        let state = self.state.clone();
        tokio::spawn(async move {
            *state.active_run.write().await = None;
        });
    }
}

/// `GET /api/pipelines/active` — read-only snapshot of the in-flight run.
///
/// Returns `{ pipeline_id, ran_for_ms }` if a run is registered, else
/// `{}`. The frontend polls this from a global banner so the user
/// always sees that another run is in flight, not just when they try
/// to start a new one. Cheap: a single RwLock read.
pub async fn active_run(
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let active = state.active_run.read().await.clone();
    match active {
        Some(run) => Json(json!({
            "pipeline_id": run.pipeline_id,
            "ran_for_ms": run.started_at.elapsed().as_millis() as u64,
        })),
        None => Json(json!({})),
    }
}

/// `POST /api/pipelines/cancel` — fire the active run's cancellation token.
///
/// Returns 200 with the cancelled `pipeline_id`, or 409 if no run is active.
/// The cancelled run releases the `pipeline_run_lock` once propagation
/// reaches a step boundary (mid-COPY: drops the PG stream future; mid-DuckDB:
/// `Connection::interrupt_handle().interrupt()`).
pub async fn cancel_run(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let active = state.active_run.read().await.clone();
    match active {
        Some(run) => {
            run.cancel.cancel();
            Ok(Json(json!({
                "status": "cancelling",
                "pipeline_id": run.pipeline_id,
                "ran_for_ms": run.started_at.elapsed().as_millis() as u64,
            })))
        }
        None => Err((
            axum::http::StatusCode::CONFLICT,
            Json(json!({"error": "no active pipeline run"})),
        )),
    }
}

/// Run a duckdb-only / custom_rust pipeline directly against tenant DuckDB,
/// no tmp scratch. Used when the pipeline has zero `pg_extract` steps —
/// typical for "build" pipelines that consume tables produced by an earlier
/// "extracts" pipeline. SQL can reference local tables by plain name.
async fn execute_tenant_write_pipeline(
    state: Arc<AppState>,
    opts: PipelineRunOptions,
    event_tx: tokio::sync::mpsc::UnboundedSender<pipeline::StepEvent>,
    nodes: Vec<Value>,
    placement: pipeline::Placement,
) -> PipelineRunReport {
    let pipeline_id = opts.pipeline_id.clone();
    let trigger_source = opts.trigger_source.clone();
    let _lock = match acquire_run_lock(&state).await {
        Ok(g) => g,
        Err(msg) => return PipelineRunReport::failure(0, &msg, nodes),
    };
    let _run_handle = ActiveRunGuard::register(&state, &pipeline_id).await;
    let t_pipeline = Instant::now();
    let mut produced_tables: Vec<String> = Vec::new();

    // Open tenant DuckDB once for the duckdb_query phase. Drop before the
    // custom_rust step so the assembly can open its own connection without
    // multi-Database conflicts on the same file.
    let mut conn_opt = match duckdb::Connection::open(&state.duckdb_path) {
        Ok(c) => Some(c),
        Err(e) => {
            return PipelineRunReport::failure(
                t_pipeline.elapsed().as_millis() as u64,
                &format!("open tenant DuckDB: {}", e),
                nodes,
            );
        }
    };

    let send_event = |id: &str, kind: &str, label: &str, status: pipeline::StepStatus,
                      message: &str, rows: i64, duration_ms: u64| {
        let _ = event_tx.send(pipeline::StepEvent {
            id: id.to_string(),
            step_type: kind.to_string(),
            label: label.to_string(),
            status,
            message: message.to_string(),
            row_count: rows,
            duration_ms,
        });
    };

    for node in &nodes {
        let id = node.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let kind = node.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let label = node.get("label").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
        let cfg = node.get("config").cloned().unwrap_or_else(|| json!({}));

        send_event(&id, &kind, &label, pipeline::StepStatus::Start, "", 0, 0);
        let t_step = Instant::now();

        match kind.as_str() {
            "duckdb_query" => {
                let sql = cfg.get("sql").and_then(|v| v.as_str())
                    .or_else(|| cfg.get("query").and_then(|v| v.as_str()))
                    .unwrap_or("").to_string();
                if sql.trim().is_empty() {
                    let msg = format!("duckdb_query '{}' has no sql", id);
                    send_event(&id, &kind, &label, pipeline::StepStatus::Failed, &msg, 0, t_step.elapsed().as_millis() as u64);
                    return PipelineRunReport::failure(t_pipeline.elapsed().as_millis() as u64, &msg, nodes);
                }
                let conn_ref = match conn_opt.as_ref() {
                    Some(c) => c,
                    None => {
                        let msg = "tenant DuckDB connection unexpectedly closed".to_string();
                        send_event(&id, &kind, &label, pipeline::StepStatus::Failed, &msg, 0, t_step.elapsed().as_millis() as u64);
                        return PipelineRunReport::failure(t_pipeline.elapsed().as_millis() as u64, &msg, nodes);
                    }
                };
                let exec_result = tokio::task::block_in_place(|| conn_ref.execute_batch(&sql));
                let dur = t_step.elapsed().as_millis() as u64;
                if let Err(e) = exec_result {
                    let msg = format!("DuckDB query '{}': {}", id, e);
                    send_event(&id, &kind, &label, pipeline::StepStatus::Failed, &msg, 0, dur);
                    return PipelineRunReport::failure(t_pipeline.elapsed().as_millis() as u64, &msg, nodes);
                }
                if let Some(t) = cfg.get("table_name").and_then(|v| v.as_str()) {
                    produced_tables.push(t.to_string());
                }
                send_event(&id, &kind, &label, pipeline::StepStatus::Success, "", 0, dur);
            }
            "custom_rust" => {
                // Drop the duckdb_query connection before invoking the assembly:
                // the assembly opens its own connection to tenant_data.duckdb,
                // and DuckDB doesn't allow two Database instances on the same
                // file in one process.
                conn_opt = None;

                let dispatcher: Arc<dyn pipeline::AssemblyDispatcher> = Arc::new(
                    crate::pipeline_assemblies::PipelineAssemblyRegistry::new(state.clone()),
                );
                let assembly_id = cfg.get("assembly_id")
                    .and_then(|v| v.as_str()).unwrap_or("").to_string();
                if assembly_id.is_empty() {
                    let msg = format!("custom_rust '{}' missing assembly_id", id);
                    send_event(&id, &kind, &label, pipeline::StepStatus::Failed, &msg, 0, t_step.elapsed().as_millis() as u64);
                    return PipelineRunReport::failure(t_pipeline.elapsed().as_millis() as u64, &msg, nodes);
                }
                let deps = pipeline::AssemblyDeps {
                    pg_pool_name: resolve_pg_pool_name(&state).unwrap_or_default(),
                    connection_map: build_connection_map(&state),
                    event_tx: event_tx.clone(),
                    step_id: id.clone(),
                    label: label.clone(),
                    partial_recompute_keys: opts.partial_recompute_keys.clone(),
                    cancel: _run_handle.cancel(),
                };
                let result = dispatcher.dispatch(&assembly_id, &cfg, deps).await;
                let dur = t_step.elapsed().as_millis() as u64;
                match result {
                    Ok(rows) => {
                        send_event(&id, &kind, &label, pipeline::StepStatus::Success, "", rows, dur);
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        send_event(&id, &kind, &label, pipeline::StepStatus::Failed, &msg, 0, dur);
                        return PipelineRunReport::failure(t_pipeline.elapsed().as_millis() as u64, &msg, nodes);
                    }
                }
            }
            other => {
                let msg = format!("step type '{}' not supported in tenant-write mode", other);
                send_event(&id, &kind, &label, pipeline::StepStatus::Failed, &msg, 0, t_step.elapsed().as_millis() as u64);
                return PipelineRunReport::failure(t_pipeline.elapsed().as_millis() as u64, &msg, nodes);
            }
        }
    }

    // Drop conn_opt explicitly (might still be set if no custom_rust step).
    conn_opt = None;
    let _ = conn_opt; // silence unused-assignment warning

    let total_ms = t_pipeline.elapsed().as_millis() as u64;

    // Post-run rehydrate the in-memory article_selection store if placement asks
    // for it AND article_selection was produced.
    if matches!(placement, pipeline::Placement::DuckDbAndInMemory)
        && nodes.iter().any(|n| {
            n.get("type").and_then(|v| v.as_str()) == Some("custom_rust")
        })
    {
        rehydrate_article_selection_store(&state, &pipeline_id);
    }

    state.traces.log_activity(
        &state.tenant_id, "shared_pipeline", &pipeline_id, "success",
        &format!("trigger={} time-to-cook={}ms (tenant-write)", trigger_source, total_ms),
        None, Some(total_ms as i64),
    ).ok();

    PipelineRunReport {
        success: true,
        total_ms,
        total_rows: 0,
        skipped_count: 0,
        error: None,
        nodes,
        merged_tables: produced_tables,
    }
}

impl PipelineRunReport {
    fn failure(total_ms: u64, msg: &str, nodes: Vec<Value>) -> Self {
        Self {
            success: false,
            total_ms,
            total_rows: 0,
            skipped_count: 0,
            error: Some(msg.to_string()),
            nodes,
            merged_tables: Vec::new(),
        }
    }
}

/// Parse the `placement` column. Anything we don't recognize falls back to
/// `DuckDbOnly` — the safe default that matches pre-Phase-3 behavior.
fn parse_placement(v: Option<&Value>) -> pipeline::Placement {
    let s = v.and_then(|x| x.as_str()).unwrap_or("duck_db_only");
    match s {
        "duck_db_and_in_memory" => pipeline::Placement::DuckDbAndInMemory,
        _ => pipeline::Placement::DuckDbOnly,
    }
}

/// Reload the in-memory `article_selection` mirror from tenant DuckDB.
/// Called post-run whenever a pipeline with placement = DuckDbAndInMemory
/// produced the `article_selection` table.
fn rehydrate_article_selection_store(state: &Arc<AppState>, pipeline_id: &str) {
    let path = state.duckdb_path.clone();
    let store = state.article_selection_store.clone();
    let pid = pipeline_id.to_string();
    // Read on a blocking thread to avoid stalling the runtime.
    tokio::task::spawn_blocking(move || {
        match crate::article_selection::load_from_duckdb(&path) {
            Ok(rows) => {
                let n = rows.len();
                store.swap(rows);
                tracing::info!(
                    pipeline = %pid, rows = n,
                    "[article_selection] in-memory store rehydrated from DuckDB"
                );
            }
            Err(e) => {
                tracing::warn!(
                    pipeline = %pid, error = %e,
                    "[article_selection] post-run rehydrate failed"
                );
            }
        }
    });
}

// ----------------------------------------------------------------------------
// JSON → typed Step translation
// ----------------------------------------------------------------------------

/// Convert smartstudio's stored pipeline JSON (a flat array of step nodes) into
/// the `pipeline` crate's typed `Step` list. A pg_extract with target=duckdb is
/// expanded into a (PgExtract → DuckDbLoad) sequence so the parquet output can
/// be loaded into the requested DuckDB table.
fn json_to_steps(nodes: &[Value]) -> Result<Vec<pipeline::Step>> {
    let mut out: Vec<pipeline::Step> = Vec::new();

    for n in nodes {
        let step_type = n.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let label = n.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cfg = n.get("config").cloned().unwrap_or_else(|| json!({}));

        match step_type {
            "pg_extract" => {
                let query = cfg.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if query.is_empty() {
                    return Err(anyhow!("pg_extract '{}' has no query", id));
                }
                // Normalize legacy "memory" → "duckdb"
                let target_raw = cfg.get("target").and_then(|v| v.as_str()).unwrap_or("parquet");
                let target = if target_raw == "memory" { "duckdb" } else { target_raw };
                let table_name  = cfg.get("table_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let output_path = cfg.get("output_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let connection_ref = cfg.get("connection")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(pipeline::ConnectionRefId::from);

                // Always materialize parquet first; for target=duckdb a sibling
                // DuckDbLoad converts it into the requested table.
                let parquet_path = if !output_path.is_empty() {
                    output_path
                } else if !table_name.is_empty() {
                    format!("__pipeline_tmp/{}", table_name)
                } else {
                    return Err(anyhow!("pg_extract '{}' must set output_path or table_name", id));
                };

                let target_source_id = cfg.get("target_source_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from);

                // Optional partitioned-extract config. When `partition_column`
                // is set, the pipeline crate runs the partitioned path
                // (one parquet per distinct value, Hive layout) instead of
                // the hash-parallel COPY. The query string must contain
                // a literal `{partition_value}` placeholder.
                let partition_column = cfg.get("partition_column")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                let partition_values_sql = cfg.get("partition_values_sql")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from);

                let pg_extract = pipeline::PgExtractConfig {
                    id: pipeline::StepId::from(id.clone()),
                    label: if label.is_empty() { format!("PG extract → {}", parquet_path) } else { label.clone() },
                    query,
                    output_path: parquet_path.clone(),
                    connection_ref,
                    pg_table: String::new(),
                    duckdb_table: table_name.clone(),
                    primary_key: Vec::new(),
                    change_key: pipeline::ChangeKey::None,
                    target_source_id: target_source_id.clone(),
                    partition_column,
                    partition_values_sql,
                };

                if target == "duckdb" && !table_name.is_empty() {
                    // Partitioned extracts emit a Hive-layout directory
                    // (`<output>/<col>=<val>/data.parquet`); the loader
                    // must glob and enable hive_partitioning. Non-
                    // partitioned extracts emit a single
                    // `<output>/data.parquet` as before.
                    let is_partitioned = pg_extract.partition_column.is_some();
                    let (source_parquet, hive_partitioning) = if is_partitioned {
                        (format!("{}/**/*.parquet", parquet_path), true)
                    } else {
                        (format!("{}/data.parquet", parquet_path), false)
                    };
                    let load = pipeline::DuckDbLoadConfig {
                        id: pipeline::StepId::from(format!("{}__load", id)),
                        label: format!("Load → {}", table_name),
                        table_name: table_name.clone(),
                        source_parquet,
                        hive_partitioning,
                        target_source_id,
                    };
                    // Wrap extract+load in a sequence so parallel-mode wrapping
                    // (in execute_pipeline_run) doesn't fan the load out before
                    // the extract has produced its parquet. Sequential mode
                    // sees the same (a sequence inside an implicit sequence).
                    out.push(
                        pipeline::Step::sequence()
                            .add_step(pg_extract)
                            .add_step(load),
                    );
                } else {
                    out.push(pg_extract.into());
                }
            }
            "duckdb_query" => {
                let sql = cfg.get("sql").and_then(|v| v.as_str())
                    .or_else(|| cfg.get("query").and_then(|v| v.as_str()))
                    .unwrap_or("").to_string();
                if sql.trim().is_empty() {
                    return Err(anyhow!("duckdb_query '{}' has no sql", id));
                }
                let q = pipeline::DuckDbQueryConfig {
                    id: pipeline::StepId::from(id),
                    label,
                    query: sql,
                    scoped_delete: None,
                    scoped_insert: None,
                    output_table: cfg.get("table_name").and_then(|v| v.as_str()).map(String::from),
                    target_source_id: cfg.get("target_source_id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                };
                out.push(q.into());
            }
            "duckdb_table" => {
                let table_name = cfg.get("table_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let parquet_path = cfg.get("parquet_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if table_name.is_empty() || parquet_path.is_empty() {
                    return Err(anyhow!("duckdb_table '{}' requires table_name and parquet_path", id));
                }
                let load = pipeline::DuckDbLoadConfig {
                    id: pipeline::StepId::from(id),
                    label,
                    table_name,
                    source_parquet: parquet_path,
                    hive_partitioning: cfg.get("hive_partitioning").and_then(|v| v.as_bool()).unwrap_or(false),
                    target_source_id: cfg.get("target_source_id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                };
                out.push(load.into());
            }
            "custom_rust" => {
                // The `assembly_id` selects which Rust assembly the host
                // dispatches to (see `pipeline_assemblies.rs`). The whole
                // `config` JSON is forwarded to the assembly verbatim — its
                // shape is per-assembly.
                let assembly_id = cfg.get("assembly_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("custom_rust '{}' missing assembly_id", id))?;
                let cr = pipeline::CustomRustConfig {
                    id: pipeline::StepId::from(id),
                    label,
                    assembly_id,
                    config: cfg.clone(),
                    output_table: cfg.get("output_table").and_then(|v| v.as_str()).map(String::from),
                    target_source_id: cfg.get("target_source_id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                };
                out.push(cr.into());
            }
            "run_pipeline" => {
                // Sub-pipeline call. Translated to a CustomRustConfig
                // bound to the `run_pipeline` assembly, which recursively
                // invokes `execute_pipeline_run` for the child pipeline_id.
                // The child's saved `execution` flag drives sequence vs
                // parallel for itself.
                let child_id = cfg.get("pipeline_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("run_pipeline '{}' missing pipeline_id in config", id))?;
                let cr = pipeline::CustomRustConfig {
                    id: pipeline::StepId::from(id),
                    label,
                    assembly_id: "run_pipeline".into(),
                    config: json!({ "pipeline_id": child_id }),
                    output_table: None,
                    target_source_id: None,
                };
                out.push(cr.into());
            }
            other => {
                return Err(anyhow!(
                    "step type '{}' is not supported by the pipeline crate (legacy executor handles loop/bq_export/gcs_download/grpc_call)",
                    other
                ));
            }
        }
    }

    Ok(out)
}

// ----------------------------------------------------------------------------
// Connection resolution (PG DSN + named connection map)
// ----------------------------------------------------------------------------

/// Default PG pool name to hand to `ExecutionContext.pg_pool_name`.
/// Returns the id of the connection marked `is_default = 1`, falling
/// back to the first pg connection. If none exists, returns an empty
/// string — pipelines without a default pool can still run if every
/// pg_extract sets an explicit `connection_ref`. The pool itself was
/// initialized at boot via `pg_pools::init_from_connections`.
fn resolve_pg_pool_name(state: &AppState) -> Option<String> {
    let sources = state.db.query("SELECT * FROM connections", &[]).ok()?;
    let is_pg = |c: &&Value| {
        let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        t == "pg" || t == "postgres"
    };
    let is_default = |c: &&Value| c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
    let conn = sources.iter().find(|c| is_pg(c) && is_default(c))
        .or_else(|| sources.iter().find(is_pg))?;
    conn.get("id").and_then(|v| v.as_str()).map(String::from)
}

/// Map of connection_ref → pool name. Today the convention is identity
/// (pool registered with the same name as the connection id), but we
/// still build the map so the pipeline crate's `resolve_pool_name`
/// receives a valid lookup table.
fn build_connection_map(state: &AppState) -> HashMap<pipeline::ConnectionRefId, String> {
    let mut map = HashMap::new();
    if let Ok(sources) = state.db.query("SELECT * FROM connections", &[]) {
        for s in sources {
            let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let t = s.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if (t == "pg" || t == "postgres") && !id.is_empty() {
                map.insert(pipeline::ConnectionRefId::from(id.clone()), id);
            }
        }
    }
    map
}

// ----------------------------------------------------------------------------
// Merge tmp DuckDB → persistent tenant_data.duckdb
// ----------------------------------------------------------------------------

/// After the pipeline writes to its tmp DuckDB, copy the resulting tables into
/// the tenant's persistent DuckDB and delete the tmp file. Atomic-swap is wrong
/// for the shared-pipeline model (it'd clobber tables from other pipelines).
///
/// Emits StepEvents for the synthetic `_merge_to_tenant` step so the UI can
/// render progress for what would otherwise be an opaque post-extract phase
/// (see the synthetic node injected in `run_stream`'s tree echo).
fn merge_tmp_into_persistent(
    persistent_path: &str,
    tmp_path: &std::path::Path,
    event_tx: &tokio::sync::mpsc::UnboundedSender<pipeline::StepEvent>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<String>> {
    use pipeline::{StepEvent, StepStatus};
    const STEP_ID: &str = "_merge_to_tenant";
    const STEP_TYPE: &str = "merge";
    const STEP_LABEL: &str = "Merge tmp → tenant DuckDB";
    let merge_started = Instant::now();

    let _ = event_tx.send(StepEvent {
        id: STEP_ID.into(),
        step_type: STEP_TYPE.into(),
        label: STEP_LABEL.into(),
        status: StepStatus::Start,
        message: String::new(),
        row_count: 0,
        duration_ms: 0,
    });

    let conn = duckdb::Connection::open(persistent_path)
        .map_err(|e| anyhow!("open persistent DuckDB: {}", e))?;
    conn.execute_batch(&format!("ATTACH '{}' AS pipeline_tmp (READ_ONLY)", tmp_path.display()))
        .map_err(|e| anyhow!("ATTACH tmp: {}", e))?;

    // Discover tables in the tmp DB's main schema.
    // information_schema isn't queryable across attached databases in DuckDB; use the
    // built-in `duckdb_tables()` table function instead.
    let mut stmt = conn.prepare(
        "SELECT table_name FROM duckdb_tables() WHERE database_name = 'pipeline_tmp' AND schema_name = 'main'"
    ).map_err(|e| anyhow!("list tmp tables: {}", e))?;
    let tables: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| anyhow!("query tmp tables: {}", e))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let total = tables.len();
    for (i, t) in tables.iter().enumerate() {
        // Cancel check between tables. The CREATE OR REPLACE itself is a
        // single DuckDB statement we can't interrupt without an InterruptHandle
        // on this connection — but we can at least bail before starting the
        // next one. Stops a "cancelling" merge from grinding through 20+
        // tables after the user clicked Cancel.
        if cancel.is_cancelled() {
            let _ = event_tx.send(StepEvent {
                id: STEP_ID.into(),
                step_type: STEP_TYPE.into(),
                label: STEP_LABEL.into(),
                status: StepStatus::Failed,
                message: "cancelled mid-merge".into(),
                row_count: 0,
                duration_ms: merge_started.elapsed().as_millis() as u64,
            });
            return Err(anyhow!("merge cancelled after {} of {} tables", i, total));
        }
        let _ = event_tx.send(StepEvent {
            id: STEP_ID.into(),
            step_type: STEP_TYPE.into(),
            label: STEP_LABEL.into(),
            status: StepStatus::Progress,
            message: format!("copying {} ({}/{})", t, i + 1, total),
            row_count: 0,
            duration_ms: 0,
        });
        let sql = format!(r#"CREATE OR REPLACE TABLE main."{0}" AS SELECT * FROM pipeline_tmp.main."{0}""#, t);
        if let Err(e) = conn.execute_batch(&sql) {
            let _ = event_tx.send(StepEvent {
                id: STEP_ID.into(),
                step_type: STEP_TYPE.into(),
                label: STEP_LABEL.into(),
                status: StepStatus::Failed,
                message: format!("copy {}: {}", t, e),
                row_count: 0,
                duration_ms: merge_started.elapsed().as_millis() as u64,
            });
            return Err(anyhow!("copy table {}: {}", t, e));
        }
    }
    let _ = event_tx.send(StepEvent {
        id: STEP_ID.into(),
        step_type: STEP_TYPE.into(),
        label: STEP_LABEL.into(),
        status: StepStatus::Progress,
        message: "checkpoint".into(),
        row_count: 0,
        duration_ms: 0,
    });
    conn.execute_batch("DETACH pipeline_tmp").ok();
    conn.execute_batch("CHECKPOINT").ok();
    drop(conn);

    // Tmp file + WAL cleanup
    std::fs::remove_file(tmp_path).ok();
    let wal = tmp_path.with_extension("duckdb.wal");
    if wal.exists() { std::fs::remove_file(&wal).ok(); }

    let _ = event_tx.send(StepEvent {
        id: STEP_ID.into(),
        step_type: STEP_TYPE.into(),
        label: STEP_LABEL.into(),
        status: StepStatus::Success,
        message: format!("{} tables", total),
        row_count: total as i64,
        duration_ms: merge_started.elapsed().as_millis() as u64,
    });
    Ok(tables)
}

// ----------------------------------------------------------------------------
// The new SSE handler
// ----------------------------------------------------------------------------

/// GET /api/pipelines/{id}/tree-stream
/// Runs the named shared pipeline via the `pipeline` crate, streaming step
/// progress as SSE events back to the caller.
pub async fn run_stream(
    State(state): State<Arc<AppState>>,
    Path(pipeline_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> axum::response::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    use axum::response::sse::{Event, KeepAlive};
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::ReceiverStream;

    let (sse_tx, sse_rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(256);
    let send_done = sse_tx.clone();
    let pipeline_id_for_task = pipeline_id.clone();

    // Skip-list lets the UI deselect specific steps before running.
    let skip_ids: HashSet<String> = params.get("skip")
        .map(|s| s.split(',').map(|id| id.trim().to_string()).filter(|id| !id.is_empty()).collect())
        .unwrap_or_default();

    // Phase 3 partial_recompute: comma-separated keys (e.g. ph_codes) that the
    // assembly should recompute scoped instead of doing a full pull. Empty =
    // full recompute, matching pre-Phase-3 behavior.
    let partial_recompute_keys: Vec<String> = params.get("keys")
        .map(|s| s.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect())
        .unwrap_or_default();

    // Execution mode: `?execution=parallel` fans out top-level steps via
    // Step::Group{Parallel}. When the query param is absent, fall back
    // to the pipeline's saved `execution` column so a parent pipeline
    // (or scheduler) calling /tree-stream without an override still
    // honors the pipeline's chosen default.
    let parallel = match params.get("execution").map(String::as_str) {
        Some("parallel") => true,
        Some(_) => false,
        None => {
            // SQL read of the saved column. Fall back to sequence on any
            // miss (column missing, row missing, type mismatch).
            state
                .db
                .query_one(
                    "SELECT execution FROM pipelines WHERE id = ?1",
                    &[&pipeline_id as &dyn rusqlite::types::ToSql],
                )
                .ok()
                .and_then(|row| row.get("execution").and_then(|v| v.as_str().map(String::from)))
                .map(|s| s == "parallel")
                .unwrap_or(false)
        }
    };

    // Run mode: quiet | normal (default) | detailed
    //   quiet    → no intra-step progress events (only phase boundaries)
    //   normal   → byte ticking on the configured interval
    //   detailed → byte ticking + pre-COUNT(*) + row totals + ETA
    let run_mode = params.get("mode").map(String::as_str).unwrap_or("normal");
    let (mode_progress_interval, mode_quantify) = match run_mode {
        "quiet"    => (None, false),
        "detailed" => (state.pipeline_progress_interval, true),
        _          => (state.pipeline_progress_interval, false), // normal
    };

    // The crate's Pipeline::execute future holds a duckdb::Connection (!Send), so it
    // can't be tokio::spawn'd on the main runtime. Run on a dedicated OS thread with a
    // private multi-thread tokio runtime: block_on pins the !Send future to this thread
    // while the runtime's blocking pool services the crate's internal spawn_blocking
    // calls (CSV→Parquet, file IO).
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build pipeline runtime");
        rt.block_on(async move {
            // Tree shape echo so the UI can render rows. We need to pre-load the
            // pipeline JSON for this; execute_pipeline_run reloads it but the
            // duplication is cheap (single SQLite read).
            let mut nodes_for_echo = load_pipeline_nodes(&state, &pipeline_id_for_task, &skip_ids);
            // Append a synthetic node for the post-extract merge phase so the
            // UI shows progress for tmp → tenant DuckDB copying. Only meaningful
            // when at least one pg_extract step exists (tenant-write pipelines
            // skip the merge entirely).
            let has_pg_extract = nodes_for_echo.iter().any(|n| {
                n.get("type").and_then(|v| v.as_str()) == Some("pg_extract")
            });
            if has_pg_extract {
                nodes_for_echo.push(json!({
                    "id": "_merge_to_tenant",
                    "type": "merge",
                    "label": "Merge tmp → tenant DuckDB",
                }));
            }
            let _ = send_done.send(Ok(Event::default().data(
                json!({"type":"tree","nodes":nodes_for_echo}).to_string()
            ))).await;

            // Bridge step events to SSE.
            let (event_tx, mut event_rx) = mpsc::unbounded_channel::<pipeline::StepEvent>();
            let bridge_tx = send_done.clone();
            let bridge = tokio::spawn(async move {
                while let Some(evt) = event_rx.recv().await {
                    let data = json!({
                        "type": "node_event",
                        "node_id": evt.id,
                        "node_type": evt.step_type,
                        "label": evt.label,
                        "status": evt.status.to_string(),
                        "message": evt.message,
                        "row_count": evt.row_count,
                        "duration_ms": evt.duration_ms,
                        "phase": if evt.status == pipeline::StepStatus::Progress { Some(evt.message.clone()) } else { None },
                    });
                    if bridge_tx.send(Ok(Event::default().data(data.to_string()))).await.is_err() {
                        break;
                    }
                }
            });

            let opts = PipelineRunOptions {
                pipeline_id: pipeline_id_for_task,
                skip_ids,
                mode_progress_interval,
                mode_quantify,
                trigger_source: "manual".to_string(),
                partial_recompute_keys,
                parallel,
            };
            let report = execute_pipeline_run(state.clone(), opts, event_tx).await;
            let _ = bridge.await;

            // Final pipeline_done event.
            let done = if report.success {
                json!({
                    "type": "pipeline_done",
                    "status": "success",
                    "total_time_ms": report.total_ms,
                    "rows": report.total_rows,
                    "skipped_count": report.skipped_count,
                })
            } else {
                json!({
                    "type": "pipeline_done",
                    "status": "failed",
                    "message": report.error.unwrap_or_default(),
                    "total_time_ms": report.total_ms,
                })
            };
            let _ = send_done.send(Ok(Event::default().data(done.to_string()))).await;
        });
    });

    axum::response::Sse::new(ReceiverStream::new(sse_rx)).keep_alive(KeepAlive::default())
}

/// Read pipeline JSON for the `tree` SSE echo only. The actual run reloads it
/// inside `execute_pipeline_run`; this duplication is cheap (one SQLite row).
fn load_pipeline_nodes(state: &Arc<AppState>, pipeline_id: &str, skip_ids: &HashSet<String>) -> Vec<Value> {
    let row = match state.db.query_one(
        "SELECT * FROM pipelines WHERE id = ?1",
        &[&pipeline_id as &dyn rusqlite::types::ToSql],
    ) { Ok(r) => r, Err(_) => return Vec::new() };
    let raw = row.get("pipeline").cloned().unwrap_or_else(|| json!("[]"));
    let nodes_array: Vec<Value> = match raw {
        Value::String(s) => serde_json::from_str(&s).unwrap_or_default(),
        Value::Array(arr) => arr,
        _ => Vec::new(),
    };
    nodes_array.into_iter()
        .filter(|n| {
            let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("");
            !skip_ids.contains(id)
        })
        .collect()
}

// ----------------------------------------------------------------------------
// Source lineage updates (Phase 3).
//
// After a pipeline run, scan the stored step JSON for `target_source_id`
// fields and update the corresponding `sources` row + insert into
// `pipeline_source_targets`. Best-effort — never fails the run.
// ----------------------------------------------------------------------------

/// Pull every `target_source_id` mentioned in a pipeline's step nodes.
/// Looks at both top-level and `config.target_source_id` so we tolerate
/// either nesting from older saved JSON.
fn collect_target_source_ids(nodes: &[Value]) -> Vec<String> {
    let mut ids = Vec::new();
    for node in nodes {
        let candidates = [
            node.get("target_source_id"),
            node.get("config").and_then(|c| c.get("target_source_id")),
        ];
        for v in candidates.into_iter().flatten() {
            if let Some(id) = v.as_str() {
                let s = id.trim();
                if !s.is_empty() && !ids.contains(&s.to_string()) {
                    ids.push(s.to_string());
                }
            }
        }
    }
    ids
}

fn update_source_lineage(state: &Arc<AppState>, pipeline_id: &str, nodes: &[Value]) {
    let ids = collect_target_source_ids(nodes);
    if ids.is_empty() { return; }
    for sid in &ids {
        if let Err(e) = state.db.execute(
            "UPDATE sources SET status = 'populated', last_populated_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            &[&sid as &dyn rusqlite::types::ToSql],
        ) {
            tracing::warn!(source_id = %sid, error = %e, "[pipeline_v2] failed to update Source status");
        }
        if let Err(e) = state.db.execute(
            "INSERT INTO pipeline_source_targets (pipeline_id, source_id, last_run_at, last_run_status) \
             VALUES (?1, ?2, datetime('now'), 'success') \
             ON CONFLICT(pipeline_id, source_id) DO UPDATE SET \
                 last_run_at = datetime('now'), last_run_status = 'success'",
            &[&pipeline_id as &dyn rusqlite::types::ToSql, &sid as _],
        ) {
            tracing::warn!(source_id = %sid, error = %e, "[pipeline_v2] failed to upsert pipeline_source_targets");
        }
    }
    tracing::info!(pipeline = %pipeline_id, sources = ?ids, "[pipeline_v2] Source lineage updated");
}

fn update_source_lineage_failed(state: &Arc<AppState>, pipeline_id: &str, nodes: &[Value], _msg: &str) {
    let ids = collect_target_source_ids(nodes);
    if ids.is_empty() { return; }
    for sid in &ids {
        let _ = state.db.execute(
            "UPDATE sources SET status = 'failed', updated_at = datetime('now') WHERE id = ?1",
            &[&sid as &dyn rusqlite::types::ToSql],
        );
        let _ = state.db.execute(
            "INSERT INTO pipeline_source_targets (pipeline_id, source_id, last_run_at, last_run_status) \
             VALUES (?1, ?2, datetime('now'), 'failed') \
             ON CONFLICT(pipeline_id, source_id) DO UPDATE SET \
                 last_run_at = datetime('now'), last_run_status = 'failed'",
            &[&pipeline_id as &dyn rusqlite::types::ToSql, &sid as _],
        );
    }
}

async fn send_error<S: AsRef<str>>(tx: &tokio::sync::mpsc::Sender<Result<axum::response::sse::Event, std::convert::Infallible>>, msg: S) {
    let body = json!({"type":"error","message": msg.as_ref()}).to_string();
    let _ = tx.send(Ok(axum::response::sse::Event::default().data(body))).await;
    let _ = tx.send(Ok(axum::response::sse::Event::default().data(json!({
        "type":"pipeline_done","status":"failed","message": msg.as_ref()
    }).to_string()))).await;
}

/// POST /api/pipeline/test-pg-query
/// Identical wire shape to the legacy version in `pipeline_handler`. Reuses the
/// same connection-resolution logic so future migrations away from
/// `pipeline_handler` are independent of this endpoint.
pub async fn test_pg_query(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    crate::handlers::pipeline_handler::test_pg_query(State(state), Json(body)).await
}
