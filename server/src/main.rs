// `cdc` is now an external crate (rust-shared-utils/cdc); no local module.

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use std::sync::Arc;
use graphstudio_server::{
    agent, article_selection, db, graph, handlers, instance_config,
    pg_pools, query, seed, service, services, trace_db, uam,
    AppState,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("graphstudio_server=info".parse().unwrap())
            .add_directive("tower_http=info".parse().unwrap()))
        .init();

    // Required: load environment.toml. Determines tenant_id, db_path, parquet_home, port.
    let cfg_path = match instance_config::discover() {
        Ok(p) => p,
        Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
    };
    let cfg = match instance_config::load(&cfg_path) {
        Ok(c) => c,
        Err(e) => { eprintln!("Error: {e:#}"); std::process::exit(1); }
    };
    let resolved = match instance_config::resolve(cfg) {
        Ok(r) => r,
        Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
    };
    if let Err(e) = instance_config::ensure_ready(&resolved) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    // Pull LLM API keys (OPENAI_API_KEY, ANTHROPIC_API_KEY) from GCP
    // Secret Manager into the process env BEFORE any Rig client is
    // built. Configured via `[agent]` in environment.toml; absent
    // config = silently skip and trust whatever's already in the
    // shell env (local dev / dotenv / k8s manifest).
    load_llm_secrets(&resolved.config.agent).await;
    tracing::info!(
        "Loaded environment.toml from {} (tenant '{}', root {})",
        cfg_path.display(), resolved.tenant_id, resolved.tenant_root
    );

    // Resolved paths flow through AppState — no env-var round-trip. `home_path`
    // from environment.toml is the only durable input; everything below derives
    // from it via instance_config::resolve.
    let db_path = resolved.db_path.clone();
    let parquet_home = resolved.parquet_home.clone();
    let log_db_path = resolved.log_db_path.clone();
    let duckdb_path = resolved.duckdb_path.clone();
    let data_dir = resolved.data_dir.clone();
    let port = resolved.port.clone();

    // dist_dir is the only non-tenant-bound path. Keep the existing exe/cwd-relative
    // resolution with the DIST_DIR env-var override.
    let exe_dir = std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    // Two depth profiles in play:
    //   • dev: `cargo run` from `smartstudio/server/` builds to
    //     `smartstudio/server/target/<profile>/`. dist sits at
    //     `smartstudio/dist/` — three `..` from exe_dir.
    //   • deploy: supervisor runs `<root>/target/release/smartstudio-server`
    //     with dist at `<root>/dist` — two `..` from exe_dir.
    // Try both before falling back to exe_dir/cwd.
    let candidates = vec![
        exe_dir.join("../../.."),
        exe_dir.join("../.."),
        exe_dir.clone(),
        cwd.clone(),
    ];
    let resolve = |env_key: &str, default: &str| -> String {
        if let Ok(v) = std::env::var(env_key) { return v; }
        for base in &candidates {
            let p = base.join(default);
            if p.exists() { return p.to_string_lossy().to_string(); }
        }
        cwd.join(default).to_string_lossy().to_string()
    };

    let dist_dir = resolve("DIST_DIR", "dist");

    tracing::info!("exe_dir={}, cwd={}", exe_dir.display(), cwd.display());
    tracing::info!("dist_dir={}, exists={}", dist_dir, std::path::Path::new(&dist_dir).exists());
    tracing::info!("db_path={}, exists={}", db_path, std::path::Path::new(&db_path).exists());
    tracing::info!("log_db_path={}", log_db_path);
    tracing::info!("duckdb_path={}", duckdb_path);

    let database = db::Database::open(&db_path).expect("Failed to open database");
    let traces = trace_db::TraceManager::new(&log_db_path);
    let cdc_manager = cdc::CdcManager::new();

    // Agent DB lives next to smartstudio.db in the tenant data dir. Open, run
    // the (idempotent) schema apply, and seed the pricing-config + model
    // allowlist if they're empty. Then spawn the single meter-writer task
    // that drains api_call/llm_usage inserts off the SSE hot path.
    let agent_db_path = format!("{}/agent.db", resolved.data_dir);
    let agent_db = Arc::new(
        agent::db::AgentDb::open(&agent_db_path).expect("Failed to open agent.db"),
    );
    if let Err(e) = agent::config::seed_pricing_config(&agent_db) {
        tracing::warn!(error = %e, "[agent] seed_pricing_config failed");
    }
    if let Err(e) = agent::config::seed_model_allowlist(&agent_db) {
        tracing::warn!(error = %e, "[agent] seed_model_allowlist failed");
    }
    if let Err(e) = agent::config::seed_workspaces(&agent_db) {
        tracing::warn!(error = %e, "[agent] seed_workspaces failed");
    }
    if let Err(e) = agent::config::seed_workspace_kind_tools(&agent_db) {
        tracing::warn!(error = %e, "[agent] seed_workspace_kind_tools failed");
    }
    if let Err(e) = agent::config::cleanup_deprecated_tools(&agent_db) {
        tracing::warn!(error = %e, "[agent] cleanup_deprecated_tools failed");
    }
    if let Err(e) = agent::config::migrate_placeholders_to_braces(&agent_db) {
        tracing::warn!(error = %e, "[agent] migrate_placeholders_to_braces failed");
    }
    let meter_tx = agent::meter::writer::spawn(agent_db.clone());
    let agent_state = Arc::new(agent::AgentState::new(
        agent_db,
        Arc::new(agent::cache::ToolCache::new()),
        meter_tx,
    ));

    let state = Arc::new(AppState {
        db: database,
        parquet_home,
        traces,
        duckdb_path,
        data_dir,
        db_path: resolved.db_path.clone(),
        port: resolved.port.clone(),
        cdc_manager,
        pg_pool: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        tenant_id: resolved.tenant_id.clone(),
        client: resolved.config.client.clone(),
        app_type: resolved.config.app_type.clone(),
        environment: resolved.config.environment.clone(),
        pipeline_run_lock: Arc::new(tokio::sync::Mutex::new(())),
        active_run: Arc::new(tokio::sync::RwLock::new(None)),
        pipeline_progress_interval: resolved.config.pipeline.progress_interval_ms
            .filter(|ms| *ms > 0)
            .map(std::time::Duration::from_millis),
        rcl_store: tokio::sync::RwLock::new(None),
        cdc_change_tx: tokio::sync::broadcast::channel(256).0,
        article_selection_store: Arc::new(article_selection::ArticleSelectionStore::new()),
        legacy_graph: Arc::new(arc_swap::ArcSwapOption::from(None)),
        graphs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        default_graph_id: resolved.config.graphs.default_id.clone(),
        uam: Arc::new(uam::UamStore::new()),
        agent: agent_state,
    });

    // DuckDB-view seed. Apply any `data/duckdb_views/*.sql` against the
    // tenant DuckDB so that materialized views referenced by sources /
    // dataviews / the graph spec are present on a fresh tenant. Idempotent
    // (CREATE OR REPLACE VIEW); ordering follows filename lexicographic
    // order so callers can express dependencies via naming. Runs BEFORE
    // graph seed so the graph's metric/spine sources can resolve.
    seed::duckdb_views::seed_duckdb_views(&state);

    // Sources seed. Upsert `data/sources/<id>.toml` into the `sources`
    // SQLite table so a fresh tenant has the canonical pg_query /
    // duckdb_table source bindings. Runs AFTER duckdb_views (target_tables
    // may reference views) and BEFORE the graph seed.
    seed::sources::seed_sources(&state);

    // DataViews seed. Upsert `data/dataviews/<id>.toml` into the
    // `dataviews` SQLite table. Runs after sources so the source-binding
    // existence check resolves; binding to an unknown source is rejected
    // per file rather than silently auto-creating a placeholder.
    seed::dataviews::seed_dataviews(&state);

    // Feedback queue. CREATE IF NOT EXISTS on the tenant DuckDB so the
    // first GET /api/feedback doesn't 500 on a fresh tenant.
    if let Err(e) = handlers::feedback::ensure_table(&state.duckdb_path) {
        tracing::warn!(error = %e, "[feedback] table ensure failed");
    }

    let api = graphstudio_server::build_router(state.clone());

    let app = Router::new()
        .nest("/api", api)
        .fallback_service(
            ServeDir::new(&dist_dir)
                .append_index_html_on_directories(true)
                .not_found_service(ServeFile::new(format!("{}/index.html", dist_dir)))
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("GraphStudio server on http://localhost:{}", port);

    // Auto-resume CDC streams for cdc_pg Sources (handlers::sources). The
    // legacy query_sources auto-start was retired in Phase 4 of source-unification.
    let cdc_state_src = state.clone();
    tokio::spawn(async move {
        // Small delay to let server fully initialize
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        handlers::sources::cdc_auto_start_all(cdc_state_src).await;
    });

    // Eager-rebuild every graph from its stored TOML on boot. Without
    // this, the in-memory graph snapshots evaporate every restart and
    // any dataview / agent call that depends on them 404s until
    // someone POSTs /build manually — which has stalled multiple
    // dashboard sessions waiting for `dataview_read` to succeed.
    // Runs in the background so boot stays fast; logs per-graph
    // outcome so failures are visible without polling.
    let graph_boot_state = state.clone();
    tokio::spawn(async move {
        let rows = match service::graphs::list(&graph_boot_state).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "[graph-boot] list failed; no graphs will be auto-built");
                return;
            }
        };
        for row in rows {
            let id = match row.get("id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None    => continue,
            };
            match service::graphs::build_by_id(&graph_boot_state, &id).await {
                Ok(stats) => tracing::info!(
                    graph_id = %id,
                    nodes = stats.total_nodes,
                    elapsed_ms = stats.elapsed_ms,
                    "[graph-boot] built"
                ),
                Err(e) => tracing::warn!(graph_id = %id, error = %e, "[graph-boot] build failed"),
            }
        }
    });

    // Pipeline scheduler (Phase 2 of misty-hinton). Reads each pipeline's
    // `trigger` column and spawns per-pipeline subscriber tasks for
    // CDC/RclChange triggers. Waits internally for RCL to come online.
    services::pipeline_scheduler::start(state.clone());

    // PG connection pools — one per row in `connections`. Must run
    // before any pipeline is triggered, since `pg_extract` now obtains
    // pooled clients via `pg::get_named_connection(connection_ref)`.
    // `init_from_connections` is idempotent and skips rows with
    // incomplete configs, so a half-set-up tenant still boots.
    if let Err(e) = pg_pools::init_from_connections(state.clone()).await {
        tracing::warn!(error = %e, "[pg-pool] init returned error; boot continues");
    }

    // Phase 3 of misty-hinton: rehydrate the in-memory article_selection
    // store from the existing DuckDB table (no-op if the table doesn't
    // exist yet). Runs on a blocking thread so boot stays fast.
    let store_state = state.clone();
    tokio::task::spawn_blocking(move || {
        match article_selection::load_from_duckdb(&store_state.duckdb_path) {
            Ok(rows) => {
                let n = rows.len();
                store_state.article_selection_store.swap(rows);
                tracing::info!(rows = n, "[article_selection] in-memory store rehydrated on boot");
            }
            Err(e) => {
                tracing::warn!(error = %e, "[article_selection] boot rehydrate failed; starting empty");
            }
        }
    });

    // Boot-time UAM cold-load. Waits for the article_graph to be present
    // (the entitlement resolver intersects against graph article NodeIds)
    // and then runs the cold-load on a background task. Subsequent
    // refreshes happen automatically after every `pl_build_article_graph`
    // run via `pipeline_assemblies::run_build_article_graph`. Until CDC
    // (Phase B), this + the `/api/uam/refresh` endpoint are the only
    // ways to refresh entitlements.
    let uam_state = state.clone();
    tokio::spawn(async move {
        let dsn = match resolve_pg_dsn_for_uam(&uam_state) {
            Some(d) => d,
            None => {
                tracing::warn!(
                    "[uam] no default PG connection at boot — skipping cold-load (call /api/uam/refresh once a connection is configured)"
                );
                return;
            }
        };
        // Wait up to 60s for the graph to be present before bailing out.
        // pl_build_article_graph builds in ~12s on the bealls dataset; allow
        // headroom. If the graph never arrives (fresh tenant, no run
        // yet), we just skip — refresh on next pipeline run.
        for _ in 0..60 {
            if let Some(graph) = uam_state.legacy_graph.load_full() {
                let universe = graph.count_kind(graph::legacy::NodeKind::Article) as i64;
                if let Err(e) = uam_state.uam.cold_load(&dsn, graph).await {
                    tracing::warn!(error=%e, "[uam] boot cold-load failed");
                    return;
                }
                if let Err(e) = uam_state.uam.materialize_to_duckdb(&uam_state.duckdb_path, universe) {
                    tracing::warn!(error=%e, "[uam] materialize_to_duckdb failed");
                }
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        tracing::info!(
            "[uam] graph not built within 60s of boot — skipping cold-load (will refresh on next pl_build_article_graph run)"
        );
    });

    // Optional: start the RCL gRPC service in-process. Gated by [rcl].enabled
    // in environment.toml. The service subscribes to PG NOTIFY for rule changes
    // (or polls when LISTEN unavailable) and exposes 3 unary resolves +
    // a server-stream Subscribe.
    if resolved.config.rcl.enabled {
        let grpc_state = state.clone();
        let grpc_port = resolved.grpc_port;
        let port_override = resolved.config.rcl.port_override;
        tokio::spawn(async move {
            start_rcl_grpc(grpc_state, grpc_port, port_override).await;
        });
    } else {
        tracing::info!("[rcl] service disabled (set [rcl] enabled = true in environment.toml to enable)");
    }

    axum::serve(listener, app).await.unwrap();
}

/// Resolve the default Postgres DSN for the UAM cold-load. Mirrors
/// the same resolution used by article_selection / cross_filter
/// handlers — picks the `is_default = 1` row in `connections` of
/// type `pg`/`postgres`, falling back to the first PG connection.
fn resolve_pg_dsn_for_uam(state: &AppState) -> Option<String> {
    let sources = state.db.query("SELECT * FROM connections", &[]).ok()?;
    let is_pg = |c: &&serde_json::Value| {
        let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        t == "pg" || t == "postgres"
    };
    let is_default = |c: &&serde_json::Value| {
        c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1
    };
    let conn = sources
        .iter()
        .find(|c| is_pg(c) && is_default(c))
        .or_else(|| sources.iter().find(is_pg))?;
    query::pg_conn_str(conn.get("config")?)
}

/// Boot the RCL gRPC service. Looks up the default PG `data_source` from
/// SQLite, builds a `RuleStore`, and starts a Tonic server bound to
/// `grpc_port`. On any failure (no PG source, RCL tables absent, port in use)
/// logs an error and returns — the rest of the server keeps running.
async fn start_rcl_grpc(
    state: std::sync::Arc<AppState>,
    grpc_port: u16,
    port_override: Option<u16>,
) {
    use services::rcl_grpc::{RclGrpcService, RclServiceServer, build_rule_store};

    let mut dsn = match resolve_default_pg_dsn(&state) {
        Some(d) => d,
        None => {
            tracing::warn!("[rcl] no default PG data_source — skipping gRPC service start");
            return;
        }
    };
    if let Some(p) = port_override {
        dsn = rewrite_pg_port(&dsn, p);
        tracing::info!("[rcl] DSN port overridden to {}", p);
    }

    let store = match build_rule_store(dsn, false).await {
        Ok(s) => std::sync::Arc::new(s),
        Err(e) => {
            tracing::error!(error=%e, "[rcl] RuleStore initialization failed (likely RCL tables missing); gRPC service not started");
            return;
        }
    };

    // Publish the RuleStore on AppState so in-process consumers (article-selection
    // materializer, future dataviews) can clone the Arc and resolve locally.
    {
        let mut guard = state.rcl_store.write().await;
        *guard = Some(store.clone());
    }

    let svc = RclGrpcService::new(store);
    // Phase 3 of misty-hinton: register the article_selection read-side
    // gRPC alongside RCL on the same Tonic server. Reads from the
    // in-memory ArticleSelectionStore (rehydrated on boot + after each
    // run with placement = DuckDbAndInMemory).
    let article_selection_svc = services::article_selection_grpc::ArticleSelectionGrpcService::new(
        state.article_selection_store.clone(),
    );
    // Article-level graph read gRPC. Backs the SmartStudio "RCL
    // Explorer" tab and (future) entity-list capability. Currently
    // reads `state.legacy_graph`; returns FAILED_PRECONDITION until
    // `pl_build_article_graph` has run at least once.
    let graph_articles_svc =
        services::graph_articles_grpc::ArticleGraphGrpcService::new(state.clone());
    // Cross-filter v2 gRPC. Mirrors the inventory-smart-rust
    // POST /cross-filter-v2 contract; same logic as the HTTP handler
    // routes through the same `crate::cross_filter` resolver.
    let cross_filter_svc =
        services::cross_filter_grpc::CrossFilterGrpcService::new(state.clone());

    let addr = format!("0.0.0.0:{}", grpc_port);
    let socket: std::net::SocketAddr = match addr.parse() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error=%e, "[rcl] invalid grpc_port: {}", addr);
            return;
        }
    };
    tracing::info!(
        "[grpc] services on {} (rcl, article_selection, graph_articles, cross_filter)",
        socket
    );
    if let Err(e) = tonic::transport::Server::builder()
        .add_service(RclServiceServer::new(svc))
        .add_service(services::article_selection_grpc::ArticleSelectionServiceServer::new(article_selection_svc))
        .add_service(services::graph_articles_grpc::ArticleGraphServiceServer::new(graph_articles_svc))
        .add_service(services::cross_filter_grpc::CrossFilterServiceServer::new(cross_filter_svc))
        .serve(socket)
        .await
    {
        tracing::error!(error=%e, "[grpc] Tonic server exited");
    }
}

/// Rewrite the port in a libpq-style DSN string, matching V4's
/// `article_selection_v4::extractor::rewrite_port`.
fn rewrite_pg_port(dsn: &str, new_port: u16) -> String {
    if let Some(start) = dsn.find("port=") {
        let after = &dsn[start + 5..];
        let end = after.find(' ').unwrap_or(after.len());
        format!("{}{}{}", &dsn[..start + 5], new_port, &after[end..])
    } else {
        format!("{} port={}", dsn, new_port)
    }
}

/// Boot-time fetch of LLM API keys from GCP Secret Manager.
///
/// The named secret's payload is expected to be a JSON object — e.g.
/// `{ "OPENAI_API_KEY": "sk-...", "ANTHROPIC_API_KEY": "sk-ant-..." }`
/// — which `SecretManager::load_env` reads and exports as process env
/// vars. Rig's `openai::Client::from_env()` (and the future Anthropic
/// equivalent) then picks them up via the standard
/// `<PROVIDER>_API_KEY` env var convention.
///
/// Silently no-ops when `[agent].gcp_project_id` or
/// `[agent].llm_secret_name` is missing — useful for local dev where
/// the keys come from a shell `export` or `.env`. Warnings (not
/// hard errors) on failure so the server still boots and a clear log
/// line tells the operator what's wrong.
async fn load_llm_secrets(cfg: &instance_config::AgentConfig) {
    let (project, name) = match (&cfg.gcp_project_id, &cfg.llm_secret_name) {
        (Some(p), Some(n)) => (p.clone(), n.clone()),
        _ => {
            tracing::info!(
                "[agent secrets] [agent].gcp_project_id / llm_secret_name not set in environment.toml; \
                 relying on shell env for OPENAI_API_KEY / ANTHROPIC_API_KEY"
            );
            return;
        }
    };
    let params = secret_manager::SecretManagerParams {
        project_id: project.clone(),
        secret_name: name.clone(),
        version: cfg.llm_secret_version,
    };
    let sm = match secret_manager::SecretManager::new(params).await {
        Ok(sm) => sm,
        Err(e) => {
            tracing::warn!(error = %e, project = %project, secret = %name,
                "[agent secrets] SecretManager init failed; relying on shell env");
            return;
        }
    };
    match sm.load_env().await {
        Ok(()) => tracing::info!(
            project = %project, secret = %name,
            "[agent secrets] loaded LLM keys from GCP secret into process env"
        ),
        Err(e) => tracing::warn!(error = %e, project = %project, secret = %name,
            "[agent secrets] load_env failed; relying on shell env"),
    }
}

/// Find the default PG data_source DSN. Mirrors the convention used by
/// dataview_source.rs / pipeline_v2.rs.
fn resolve_default_pg_dsn(state: &AppState) -> Option<String> {
    let sources = state.db.query("SELECT * FROM connections", &[]).ok()?;
    let is_pg = |c: &&serde_json::Value| {
        let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        t == "pg" || t == "postgres"
    };
    let is_default = |c: &&serde_json::Value| {
        c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1
    };
    let conn = sources
        .iter()
        .find(|c| is_pg(c) && is_default(c))
        .or_else(|| sources.iter().find(is_pg))?;
    query::pg_conn_str(conn.get("config")?)
}

