use std::sync::Arc;

/// One active pipeline run.
#[derive(Clone)]
pub struct ActiveRun {
    pub pipeline_id: String,
    pub started_at: std::time::Instant,
    /// Cancel token plumbed into the run's `pipeline::ExecutionContext`.
    /// Firing it interrupts in-flight PG COPY and DuckDB statements.
    pub cancel: tokio_util::sync::CancellationToken,
}

pub struct AppState {
    pub db: crate::db::Database,
    pub parquet_home: String,
    pub traces: crate::trace_db::TraceManager,
    pub duckdb_path: String,
    /// Tenant data dir (`{home}/smartstudio/{tenant_id}/data`). Used as
    /// DuckDB's `home_directory` for every connection we open, so the
    /// extension cache lives next to the rest of the tenant's artifacts
    /// instead of relying on the OS `HOME` env var (which can point at a
    /// missing path on headless deploys).
    pub data_dir: String,
    /// SQLite metadata DB path (`{home}/smartstudio/{tenant_id}/data/smartstudio.db`).
    /// Stored on AppState so /api/health can report the live config without
    /// the env-var round-trip the original implementation used.
    pub db_path: String,
    /// HTTP listen port string ("3001" by default). Same rationale as `db_path`.
    pub port: String,
    pub cdc_manager: cdc::CdcManager,
    /// Cached PG connections keyed by DSN. Avoids ~1s TCP handshake on each materialize.
    pub pg_pool: tokio::sync::Mutex<std::collections::HashMap<String, tokio_postgres::Client>>,
    /// Tenant identity from environment.toml. Single source of truth for who this
    /// instance serves. Used in place of any per-row client_id/app_id.
    pub tenant_id: String,
    pub client: String,
    pub app_type: String,
    pub environment: String,
    /// Guards shared-pipeline runs. DuckDB doesn't allow two separate Database
    /// instances on the same file, so we serialize writes from pipeline executions.
    /// Read-only queries (Live View, Schema introspect) bypass this lock.
    pub pipeline_run_lock: Arc<tokio::sync::Mutex<()>>,
    /// Active pipeline run registry. Set when a run acquires the lock,
    /// cleared on completion. `POST /api/pipelines/cancel` reads this and
    /// fires the token, which propagates through `pipeline::ExecutionContext`
    /// down to PG COPY streams (drop the future) and DuckDB statements
    /// (`Connection::interrupt_handle().interrupt()`).
    pub active_run: Arc<tokio::sync::RwLock<Option<ActiveRun>>>,
    /// Global cadence for intra-step pipeline progress events. `None` disables
    /// (only phase-boundary events fire). Read from `[pipeline]
    /// progress_interval_ms` in environment.toml at boot.
    pub pipeline_progress_interval: Option<std::time::Duration>,
    /// Live RCL rule corpus, populated when [rcl].enabled = true. In-process
    /// consumers (e.g., the article-selection materializer) clone the Arc and
    /// resolve locally. None when the RCL service is disabled or failed to start.
    pub rcl_store: tokio::sync::RwLock<Option<Arc<rcl::RuleStore>>>,
    /// Broadcasts CDC change events as they're applied. Published by
    /// `handlers::sources::cdc_start` (wraps the on_lsn_update callback);
    /// consumed by `services::pipeline_scheduler` to fire `Cdc` triggers.
    /// Phase 2 of misty-hinton.
    pub cdc_change_tx: tokio::sync::broadcast::Sender<
        crate::services::pipeline_scheduler::CdcChangeEvent,
    >,
    /// In-memory mirror of `article_selection`. Phase 3 of misty-hinton.
    /// Rehydrated on boot and after each pipeline run with placement =
    /// DuckDbAndInMemory. Read by the article_selection Tonic service.
    pub article_selection_store: Arc<crate::article_selection::ArticleSelectionStore>,
    /// Hand-coded article-graph snapshot (`graph::legacy::ArticleGraph`).
    /// None until the first `pl_build_article_graph` run completes;
    /// replaced atomically (ArcSwap) on every full rebuild and on every
    /// CDC delta. Readers (gRPC legacy service, internal aggregate
    /// lookups) clone the inner Arc and traverse without locking.
    ///
    /// Slated for deletion once the metadata-driven `graph::Graph`
    /// covers the article-level read surface (match_product,
    /// resolve_rcl, exception list, brands, UAM cold-load); see
    /// `docs/v1-cleanup-todo.md`.
    pub legacy_graph: Arc<arc_swap::ArcSwapOption<crate::graph::legacy::ArticleGraph>>,
    /// TOML-defined graph snapshots, one per graph_id. `POST
    /// /api/graphs/:id/build` reads the spec from SQLite, builds via
    /// `graph::build_graph`, and atomically swaps the `ArcSwapOption`
    /// slot. Readers clone the inner Arc and traverse without locking
    /// — same shape as `legacy_graph` above.
    pub graphs: Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<
                String,
                Arc<arc_swap::ArcSwapOption<crate::graph::Graph>>,
            >,
        >,
    >,
    /// Default graph id (from `[graphs] default_id` in environment.toml).
    /// Handlers that need "the" graph without a per-request id read this.
    /// `None` means no default — endpoints depending on it return 503.
    pub default_graph_id: Option<String>,
    /// UAM (User Access Management) entitlements per (user_code,
    /// acl_code). Cold-loaded at boot from
    /// `global.user_access_hierarchy_mapping` and resolved against the
    /// live graph. Phase A: refreshed on demand. Phase B will subscribe
    /// to PG NOTIFY for incremental updates.
    pub uam: Arc<crate::uam::UamStore>,
    /// Agent module state (SQLite for workspaces/sessions/prompts/usage,
    /// LRU cache for idempotent tool results, channel handle for the
    /// metering writer task). See `crate::agent::AgentState`.
    pub agent: Arc<crate::agent::AgentState>,
}
