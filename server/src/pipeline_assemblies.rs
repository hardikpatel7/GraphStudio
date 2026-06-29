//! Smartstudio's [`pipeline::AssemblyDispatcher`] implementation.
//!
//! The pipeline crate exposes `Step::CustomRust { config }` but knows nothing
//! about what assemblies actually do. This module wires concrete Rust
//! assemblies (today: V4 article_selection) to assembly ids and hands the
//! dispatcher into [`pipeline::ExecutionContext`] at run time.
//!
//! Adding an assembly: add a `match` arm in `dispatch_inner` and a function
//! that takes `(deps, AppState, AssemblyConfig)` → `Result<i64>` (rows
//! written). Pipelines reference it by `assembly_id` in their JSON.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use crate::AppState;
use crate::graph::legacy;
use crate::article_selection::{
    extract_and_assemble, extract_and_assemble_from_duckdb, extract_and_assemble_scoped,
    materialize_partial_to_duckdb, materialize_to_duckdb,
};

/// Implements [`pipeline::AssemblyDispatcher`] by holding the live AppState.
///
/// One instance is constructed per pipeline run in
/// `handlers::pipeline_v2::run_stream` and dropped when the pipeline finishes.
pub struct PipelineAssemblyRegistry {
    state: Arc<AppState>,
}

impl PipelineAssemblyRegistry {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

impl pipeline::AssemblyDispatcher for PipelineAssemblyRegistry {
    fn dispatch<'a>(
        &'a self,
        assembly_id: &'a str,
        config: &'a Value,
        deps: pipeline::AssemblyDeps,
    ) -> Pin<Box<dyn Future<Output = Result<i64>> + Send + 'a>> {
        Box::pin(async move {
            match assembly_id {
                "article_selection" => run_article_selection(&self.state, config, deps).await,
                "article_selection_v7" => run_article_selection_v7(&self.state, config, deps).await,
                "build_article_graph" => run_build_article_graph(&self.state, config, deps).await,
                "build_graph" => run_build_graph(&self.state, config, deps).await,
                "ch_extract" => run_ch_extract(&self.state, config, deps).await,
                "run_pipeline" => run_sub_pipeline(self.state.clone(), config, deps).await,
                other => Err(anyhow!("unknown assembly_id '{}'", other)),
            }
        })
    }
}

/// V7 path. Reads asv2_* + raw config tables from tenant DuckDB (built by
/// `pl_v7_extracts` + `pl_v7_build`'s `build_asv2_*` queries) instead of
/// pulling from PG. Same RCL + assemble work; same materialize_to_duckdb
/// output. Replaces the ~22s PG-COPY phase with a sub-second DuckDB read.
async fn run_article_selection_v7(
    state: &AppState,
    _config: &Value,
    deps: pipeline::AssemblyDeps,
) -> Result<i64> {
    let store = {
        let guard = state.rcl_store.read().await;
        guard.clone()
    };
    let store = store.ok_or_else(|| {
        anyhow!("RCL service is not running; enable [rcl] in environment.toml and restart")
    })?;
    let ruleset = store.snapshot();

    let duckdb_path = state.duckdb_path.clone();
    let cancel = deps.cancel.clone();
    // Flatten the anyhow chain into the message so the SSE event surfaces
    // the actual cause (e.g. "column 'X' not found") instead of just the
    // outer context — `e.to_string()` only shows the top layer.
    let extract = tokio::task::spawn_blocking(move || {
        extract_and_assemble_from_duckdb(&duckdb_path, ruleset, &cancel)
    })
    .await
    .map_err(|e| anyhow!("extract_and_assemble_from_duckdb join: {:#}", e))?
    .map_err(|e| anyhow!("extract_and_assemble_from_duckdb: {:#}", e))?;

    let rows = extract.rows;
    let duckdb_path = state.duckdb_path.clone();
    let mat = tokio::task::spawn_blocking(move || materialize_to_duckdb(&duckdb_path, &rows))
        .await
        .map_err(|e| anyhow!("materialize_to_duckdb join: {:#}", e))?
        .map_err(|e| anyhow!("materialize_to_duckdb: {:#}", e))?;
    Ok(mat.rows_written as i64)
}

/// Article-selection assembly. Routes to one of two paths based on
/// `deps.partial_recompute_keys`:
///
/// - Empty keys → full path: `extract_and_assemble` + `materialize_to_duckdb`
///   (replaces the entire DuckDB table). Boot rehydration of the in-memory
///   store happens via the standard post-run hook in `pipeline_v2`.
///
/// - Non-empty keys → scoped path: `extract_and_assemble_scoped` filters PG
///   reads to those `ph_codes`, `materialize_partial_to_duckdb` does
///   DELETE+INSERT for just those rows, and the in-memory store is updated
///   surgically via `update_rows`. Phase 3 partial_recompute.
async fn run_article_selection(
    state: &AppState,
    _config: &Value,
    deps: pipeline::AssemblyDeps,
) -> Result<i64> {
    let store = {
        let guard = state.rcl_store.read().await;
        guard.clone()
    };
    let store = store.ok_or_else(|| {
        anyhow!("RCL service is not running; enable [rcl] in environment.toml and restart")
    })?;
    let ruleset = store.snapshot();

    let dsn = resolve_default_pg_dsn(state)
        .ok_or_else(|| anyhow!("no default PG connection — mark one as default for type=pg"))?;

    if deps.partial_recompute_keys.is_empty() {
        // Full recompute (Phase 1 path).
        let extract = extract_and_assemble(&dsn, ruleset.clone(), &deps.cancel)
            .await
            .context("article_selection extract_and_assemble")?;
        let rows = extract.rows;

        let duckdb_path = state.duckdb_path.clone();
        let mat = tokio::task::spawn_blocking(move || materialize_to_duckdb(&duckdb_path, &rows))
            .await
            .context("materialize_to_duckdb join")?
            .context("materialize_to_duckdb")?;
        return Ok(mat.rows_written as i64);
    }

    // Scoped recompute (Phase 3 partial_recompute path).
    let keys = deps.partial_recompute_keys.clone();
    tracing::info!(
        keys = keys.len(),
        "[article_selection] scoped recompute (partial_recompute path)"
    );
    let extract = extract_and_assemble_scoped(&dsn, ruleset.clone(), &keys, &deps.cancel)
        .await
        .context("article_selection extract_and_assemble_scoped")?;
    let rows = extract.rows;

    // DuckDB surgical apply.
    let duckdb_path = state.duckdb_path.clone();
    let keys_for_db = keys.clone();
    let rows_for_db = rows.clone();
    let mat = tokio::task::spawn_blocking(move || {
        materialize_partial_to_duckdb(&duckdb_path, &keys_for_db, &rows_for_db)
    })
    .await
    .context("materialize_partial_to_duckdb join")?
    .context("materialize_partial_to_duckdb")?;

    // In-memory store surgical apply. Cheap enough to do inline.
    state.article_selection_store.update_rows(&keys, rows);

    Ok(mat.rows_written as i64)
}

/// V8 path. Builds an in-memory `ArticleGraph` from the same `asv2_*` /
/// `raw_*` DuckDB tables V7 reads (via `graph::legacy::source::DuckDbReader`)
/// and ArcSwaps it into `state.legacy_graph`. No DuckDB write-back —
/// the graph is the materialization. Returns the article-node count so
/// the pipeline runner's row-count contract still holds.
///
/// Step config (currently unused; reserved for `{"source":"duckdb"}` /
/// `{"source":"parquet","path":…}` / `{"source":"pg"}` / `{"source":"bq"}`
/// once Phase 6 ships the alternate readers).
async fn run_build_article_graph(
    state: &AppState,
    _config: &Value,
    deps: pipeline::AssemblyDeps,
) -> Result<i64> {
    use crate::graph::legacy::source::duckdb::DuckDbReader;
    use crate::graph::legacy::build::build_graph;

    let duckdb_path = state.duckdb_path.clone();
    let graph_arc = state.legacy_graph.clone();
    let cancel = deps.cancel.clone();

    // Snapshot the live RuleSet (when RCL is enabled) so the graph build
    // can pre-bind per-article rule_pointers. With pointers bound, Live
    // View projections skip the per-row priority/specificity walk and
    // do an O(1) lookup against `rules.policies` / `rules.constraints`.
    let ruleset_snapshot = {
        let guard = state.rcl_store.read().await;
        guard.as_ref().map(|store| store.snapshot())
    };

    // Bump version off the previous graph (None → 1, Some(g) → g.version+1).
    let next_version = graph_arc
        .load()
        .as_ref()
        .map(|g| g.graph_version + 1)
        .unwrap_or(1);

    let (graph, stats) = tokio::task::spawn_blocking(move || -> Result<_> {
        let reader = DuckDbReader::open(&duckdb_path)
            .map_err(|e| anyhow!("legacy_graph open duckdb: {:#}", e))?;
        build_graph(&reader, next_version, &cancel, ruleset_snapshot.as_deref())
            .map_err(|e| anyhow!("legacy_graph build: {:#}", e))
    })
    .await
    .map_err(|e| anyhow!("legacy_graph build join: {:#}", e))??;

    let new_graph = std::sync::Arc::new(graph);
    graph_arc.store(Some(new_graph.clone()));

    // UAM cold-load: entitlements are resolved against the live graph
    // snapshot, so they go stale whenever the graph rebuilds. Kick a
    // refresh in the background — failures are logged but don't fail
    // the pipeline run (cross-filter degrades gracefully when UAM is
    // empty).
    tracing::info!("[uam] kicking post-graph-build refresh");
    if let Some(dsn) = resolve_default_pg_dsn(state) {
        let uam = state.uam.clone();
        let graph_for_uam = new_graph;
        tokio::spawn(async move {
            tracing::info!("[uam] post-graph-build refresh: starting cold-load");
            match uam.cold_load(&dsn, graph_for_uam).await {
                Ok(_) => tracing::info!("[uam] post-graph-build refresh: done"),
                Err(e) => tracing::warn!(error=%e, "[uam] post-graph-build refresh failed"),
            }
        });
    } else {
        tracing::warn!("[uam] no default PG connection — skipping post-graph-build refresh");
    }

    Ok(stats.articles as i64)
}

/// `build_graph` assembly — builds an `article_graph::Graph`
/// from a TOML spec stored in the `graphs` SQLite table, and swaps
/// the resulting snapshot into `state.graphs[graph_id]`.
///
/// Config shape: `{ "graph_id": "bealls-inventory-graph" }`. The
/// graph row is fetched on each run so edits in the UI (or via
/// `PUT /api/graphs/:id`) take effect on the next pipeline trigger
/// without a server restart.
///
/// Returns the total node count so the step's `row_count` shows
/// what was materialized.
async fn run_build_graph(
    state: &AppState,
    config: &Value,
    _deps: pipeline::AssemblyDeps,
) -> Result<i64> {
    let graph_id = config
        .get("graph_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("build_graph: missing config.graph_id"))?
        .to_string();

    // 1. Pull TOML from SQLite. UI-edited specs land here via
    // `PUT /api/graphs/:id`, so this read is always against the
    // user-current shape.
    let row = state
        .db
        .query_one(
            "SELECT toml_text FROM graphs WHERE id = ?1",
            &[&graph_id as &dyn rusqlite::types::ToSql],
        )
        .with_context(|| format!("build_graph: graph `{graph_id}` not found"))?;
    let toml_text = row["toml_text"].as_str().unwrap_or("").to_string();

    // 2. Parse + pre-validate. Build-time validation re-runs even
    // after the UI write path validated — the underlying DuckDB
    // catalog might have drifted between save and pipeline run.
    let spec = crate::graph::from_toml(&toml_text)
        .map_err(|e| anyhow!("build_graph: parse `{graph_id}`: {e:#}"))?;
    let issues = crate::graph::validate(&spec);
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| matches!(i.severity, crate::graph::Severity::Error))
        .collect();
    if !errors.is_empty() {
        return Err(anyhow!(
            "build_graph: graph `{graph_id}` failed validation with {} error(s); fix in the UI before running this pipeline",
            errors.len()
        ));
    }

    // 3. Build off the runtime — DuckDB I/O is blocking and the
    // build itself is CPU-bound. Same pattern v1's article_graph_v8
    // assembly uses.
    let duckdb_path = state.duckdb_path.clone();
    let spec_arc = Arc::new(spec);
    let spec_for_build = spec_arc.clone();
    let graph_id_for_log = graph_id.clone();
    let (graph, stats) = tokio::task::spawn_blocking(move || -> Result<_> {
        let reader =
            crate::graph::source::duckdb::DuckDbSourceReader::open(&duckdb_path)
                .with_context(|| format!("build_graph `{graph_id_for_log}`: open duckdb"))?;
        crate::graph::build_graph(&spec_for_build, &reader, 1)
            .with_context(|| format!("build_graph `{graph_id_for_log}`: build_graph"))
    })
    .await
    .map_err(|e| anyhow!("build_graph join: {e:#}"))??;

    // 4. Atomic publish via ArcSwap. Existing readers keep the
    // previous snapshot until they reload — no torn reads.
    let slot = {
        let mut graphs = state.graphs.write().await;
        graphs
            .entry(graph_id.clone())
            .or_insert_with(|| Arc::new(arc_swap::ArcSwapOption::from(None)))
            .clone()
    };
    slot.store(Some(Arc::new(graph)));

    tracing::info!(
        graph_id = %graph_id,
        nodes = stats.total_nodes,
        metrics = stats.primary_metric_count,
        composite_metrics = stats.composite_metric_count,
        strings = stats.strings_interned,
        elapsed_ms = stats.elapsed_ms,
        "build_graph complete"
    );
    Ok(stats.total_nodes as i64)
}

/// Same default-PG resolution used by the legacy
/// `POST /api/article-selection/materialize` handler. Kept locally to avoid
/// pulling that handler module into the pipeline path.
fn resolve_default_pg_dsn(state: &AppState) -> Option<String> {
    let sources = state.db.query("SELECT * FROM connections", &[]).ok()?;
    let is_pg = |c: &&Value| {
        let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        t == "pg" || t == "postgres"
    };
    let is_default = |c: &&Value| c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
    let conn = sources
        .iter()
        .find(|c| is_pg(c) && is_default(c))
        .or_else(|| sources.iter().find(is_pg))?;
    crate::query::pg_conn_str(conn.get("config")?)
}

/// `run_pipeline` assembly — recursively runs another saved pipeline.
///
/// Config shape: `{ "pipeline_id": "..." }`. The child's stored
/// `execution` flag (sequence / parallel) drives its own fan-out; the
/// parent's run-mode flags do not propagate. Events emitted by the
/// child are drained without forwarding for now — when the parent's
/// SSE stream is the consumer, child step events would re-render
/// confusingly under the parent's tree shape.
///
/// Returns the child run's reported row count so the parent step
/// shows a non-zero `row_count` in its event.
///
/// CAVEAT: no cycle detection. A pipeline that calls itself (directly
/// or through a chain) will recurse until the run lock + duckdb
/// connection pool give out. Author the call graph carefully.
async fn run_sub_pipeline(
    state: Arc<AppState>,
    config: &Value,
    _deps: pipeline::AssemblyDeps,
) -> Result<i64> {
    let child_id = config
        .get("pipeline_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("run_pipeline assembly: config.pipeline_id is required"))?;

    let opts = crate::handlers::pipeline_v2::PipelineRunOptions {
        pipeline_id: child_id.clone(),
        skip_ids: std::collections::HashSet::new(),
        partial_recompute_keys: Vec::new(),
        // Child can be flipped to parallel via its own saved row, but we
        // don't currently re-read it here (Phase 1 keeps the call simple
        // and treats every nested run as sequential). Promoting this is
        // a follow-up — wire `state.db` lookup of the child's `execution`
        // column.
        parallel: false,
        mode_progress_interval: None,
        mode_quantify: false,
        trigger_source: "run_pipeline assembly".to_string(),
    };

    // execute_pipeline_run holds a !Send duckdb::Connection in its
    // future, so we can't .await it directly inside this Send-bound
    // assembly future. Mirror the pattern run_stream uses: spawn an OS
    // thread with a private tokio runtime, run the child there, and
    // ferry the result back over a oneshot.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<crate::handlers::pipeline_v2::PipelineRunReport>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build sub-pipeline runtime");
        rt.block_on(async move {
            // Drain child events into a black hole — the parent SSE
            // stream renders the parent's own tree shape, where child
            // step events would be confusing without a sub-tree
            // surface. (Future: bridge as nested events under the
            // assembly's own step row.)
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<pipeline::StepEvent>();
            tokio::spawn(async move {
                while rx.recv().await.is_some() { /* drop */ }
            });
            let report = crate::handlers::pipeline_v2::execute_pipeline_run(state, opts, tx).await;
            let _ = done_tx.send(report);
        });
    });
    let report = done_rx
        .await
        .map_err(|e| anyhow!("child pipeline '{}' runtime closed: {}", child_id, e))?;
    if !report.success {
        return Err(anyhow!(
            "child pipeline '{}' failed: {}",
            child_id,
            report.error.unwrap_or_else(|| "(no error message)".into())
        ));
    }
    Ok(report.total_rows)
}

/// `ch_extract` assembly — query a ClickHouse connection, write the
/// rows into a DuckDB table in the tenant DuckDB. Same role as
/// `pg_extract` + `write_parquet` but the destination is a DuckDB
/// table (parquet round-trip would require CH→parquet bridge work
/// that isn't in this MVP).
///
/// Step config:
/// ```json
/// {
///   "assembly_id": "ch_extract",
///   "connection_ref": "ds_xxx",   // id of a connections row with type="clickhouse"
///   "sql": "SELECT ... FROM db.table",
///   "target_table": "ch_my_extract"
/// }
/// ```
///
/// Returns row count.
///
/// Implementation: runs SQL via `clickhouse::query_exec`, dumps the
/// rows to a temp NDJSON file, then `CREATE OR REPLACE TABLE
/// <target> AS SELECT * FROM read_json_auto('<temp>')`. The JSON
/// auto-loader handles type inference per column; the temp file is
/// removed on success or failure.
async fn run_ch_extract(
    state: &AppState,
    config: &Value,
    _deps: pipeline::AssemblyDeps,
) -> Result<i64> {
    use std::io::Write;

    let connection_ref = config.get("connection_ref")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ch_extract: missing config.connection_ref"))?
        .to_string();
    let sql = config.get("sql")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ch_extract: missing config.sql"))?
        .to_string();
    let target_table = config.get("target_table")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ch_extract: missing config.target_table"))?
        .to_string();

    // Pull the CH connection config from the connections table.
    let row = state.db.query_one(
        "SELECT * FROM connections WHERE id = ?1",
        &[&connection_ref as &dyn rusqlite::types::ToSql],
    ).map_err(|_| anyhow!("ch_extract: connection '{}' not found", connection_ref))?;
    let conn_type = row.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if conn_type != "clickhouse" {
        return Err(anyhow!(
            "ch_extract: connection '{}' has type='{}', expected 'clickhouse'",
            connection_ref, conn_type
        ));
    }
    let ch_config = row.get("config").cloned()
        .ok_or_else(|| anyhow!("ch_extract: connection has no config blob"))?;
    let conn = crate::clickhouse::ChConnection::from_config(&ch_config)
        .map_err(|e| anyhow!("ch_extract: ClickHouse config invalid: {e:#}"))?;

    // Run the query.
    let started = std::time::Instant::now();
    let result = crate::clickhouse::query_exec(&conn, &sql).await
        .map_err(|e| anyhow!("ch_extract: ClickHouse query failed: {e:#}"))?;
    let rows = result.rows;
    let count = rows.len() as i64;
    tracing::info!(
        connection_ref = %connection_ref,
        target_table = %target_table,
        rows = count,
        query_ms = started.elapsed().as_millis(),
        "[ch_extract] query complete"
    );

    // Dump rows to a temp NDJSON file (one JSON object per line) for
    // DuckDB's read_json_auto to consume. Keep the file under
    // tenant_data_dir/duckdb_temp so it lives on the tenant volume.
    let temp_dir = std::path::Path::new(&state.data_dir).join("duckdb_temp");
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("ch_extract: create temp dir {}", temp_dir.display()))?;
    let temp_file = temp_dir.join(format!("ch_extract_{}.ndjson", uuid_for_temp()));
    {
        let mut f = std::fs::File::create(&temp_file)
            .with_context(|| format!("ch_extract: create temp file {}", temp_file.display()))?;
        for row in &rows {
            writeln!(f, "{}", serde_json::to_string(row).unwrap_or_default())
                .context("ch_extract: write temp NDJSON line")?;
        }
    }

    // Load into DuckDB.
    let duckdb_path = state.duckdb_path.clone();
    let temp_path_str = temp_file.to_string_lossy().to_string();
    let target_for_db = target_table.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<()> {
        let db = duckdb::Connection::open(&duckdb_path)
            .with_context(|| format!("ch_extract: open DuckDB at {}", duckdb_path))?;
        let create_sql = format!(
            "CREATE OR REPLACE TABLE {target_for_db} AS SELECT * FROM read_json_auto('{temp_path_str}')"
        );
        db.execute(&create_sql, []).with_context(|| {
            format!("ch_extract: CREATE TABLE failed: {create_sql}")
        })?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow!("ch_extract: DuckDB task join: {e:#}"))?;

    let _ = std::fs::remove_file(&temp_file);
    result?;

    tracing::info!(
        target_table = %target_table,
        rows = count,
        total_ms = started.elapsed().as_millis(),
        "[ch_extract] materialized to DuckDB"
    );
    Ok(count)
}

/// Tiny ad-hoc unique id for temp filenames. Uses nanosecond clock +
/// thread id; collision-resistant enough for short-lived files in a
/// single-process tenant.
fn uuid_for_temp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos()).unwrap_or(0);
    format!("{nanos:x}_{:?}", std::thread::current().id())
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "")
}
