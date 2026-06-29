use std::sync::Arc;
use axum_test::TestServer;
use tempfile::TempDir;
use graphstudio_server::{
    agent, article_selection, db, trace_db, uam,
    AppState, build_router,
};

pub async fn setup_server() -> (TestServer, TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().to_str().unwrap().to_string();

    let toml = format!(
        r#"home_path = "{home}"
client = "test"
app_type = "test"
environment = "test"
is_new = true

[server]
port = 13001
grpc_port = 50052

[rcl]
enabled = false
"#,
        home = home
    );
    let toml_path = tmp.path().join("environment.toml");
    std::fs::write(&toml_path, toml).unwrap();

    let cfg = graphstudio_server::instance_config::load(&toml_path).expect("load config");
    let resolved = graphstudio_server::instance_config::resolve(cfg).expect("resolve config");
    graphstudio_server::instance_config::ensure_ready(&resolved).expect("ensure_ready");

    let database = db::Database::open(&resolved.db_path).expect("open db");
    let traces = trace_db::TraceManager::new(&resolved.log_db_path);
    let cdc_manager = cdc::CdcManager::new();

    let agent_db_path = format!("{}/agent.db", resolved.data_dir);
    let agent_db = Arc::new(
        agent::db::AgentDb::open(&agent_db_path).expect("open agent.db"),
    );
    agent::config::seed_pricing_config(&agent_db).ok();
    agent::config::seed_model_allowlist(&agent_db).ok();
    agent::config::seed_workspaces(&agent_db).ok();
    agent::config::seed_workspace_kind_tools(&agent_db).ok();
    let meter_tx = agent::meter::writer::spawn(agent_db.clone());
    let agent_state = Arc::new(agent::AgentState::new(
        agent_db,
        Arc::new(agent::cache::ToolCache::new()),
        meter_tx,
    ));

    let (cdc_change_tx, _) = tokio::sync::broadcast::channel(256);

    let state = Arc::new(AppState {
        db: database,
        parquet_home: resolved.parquet_home.clone(),
        traces,
        duckdb_path: resolved.duckdb_path.clone(),
        data_dir: resolved.data_dir.clone(),
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
        pipeline_progress_interval: None,
        rcl_store: tokio::sync::RwLock::new(None),
        cdc_change_tx,
        article_selection_store: Arc::new(
            article_selection::ArticleSelectionStore::new(),
        ),
        legacy_graph: Arc::new(arc_swap::ArcSwapOption::from(None)),
        graphs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        default_graph_id: None,
        uam: Arc::new(uam::UamStore::new()),
        agent: agent_state,
    });

    let router = build_router(state);
    let server = TestServer::new(router).expect("TestServer::new");
    (server, tmp)
}
