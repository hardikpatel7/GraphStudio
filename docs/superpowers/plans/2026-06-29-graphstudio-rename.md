# GraphStudio Rename — Test Suite + Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a complete test suite (Rust integration + React component + MCP canary) that is fully green, then rename all display strings from "SmartStudio" to "GraphStudio," confirmed by the canary tests going RED on the renamed items.

**Architecture:** A `src/lib.rs` is added to the Rust binary crate to expose modules for integration testing; the router and AppState are extracted into dedicated files. Frontend tests use Vitest + jsdom + @testing-library/react. MCP server gets a standalone Vitest canary.

**Tech Stack:** Rust/Axum 0.8, `axum-test 17`, `tempfile 3`, Vitest, @testing-library/react, jsdom, TypeScript

## Global Constraints

- All POST handlers return **201 Created** (not 200)
- Every resource `id` must be provided in the request body — the server does NOT auto-generate IDs
- `build_router(state)` returns the inner API `Router` (routes at `/health`, `/pipelines`, etc. — no `/api/` prefix, no static-file fallback); the outer nesting lives only in `main()`
- RCL gRPC service is disabled in tests: `[rcl] enabled = false` in the test `environment.toml`
- Phase 3 renames are **display strings only**: `NAMESPACE_DIR`, the SQLite filename `smartstudio.db`, env vars `SMARTSTUDIO_PG_MAX_CONCURRENCY` / `SMARTSTUDIO_BEALLS_DUCKDB` / `SMARTSTUDIO_URL`, the GCS bucket path `smartstudio-data`, and the bundle filename prefix `smartstudio-bundle-` are **not renamed**
- `src-old/` is dead code — ignore it entirely
- Vitest `globals: true`, environment `jsdom`

---

### Task 1: Add `lib.rs`, `app_state.rs`, `router.rs` — expose crate for integration tests

**Files:**
- Create: `server/src/lib.rs`
- Create: `server/src/app_state.rs`
- Create: `server/src/router.rs`
- Modify: `server/src/main.rs`
- Modify: `server/Cargo.toml`

**Interfaces:**
- Produces: `smartstudio_server::AppState`, `smartstudio_server::ActiveRun`, `smartstudio_server::build_router` — used by Tasks 2–8

- [ ] **Step 1: Add `[lib]` section to Cargo.toml**

  In `server/Cargo.toml`, insert after the `[package]` block (before `[profile.release]`):

  ```toml
  [lib]
  name = "smartstudio_server"
  path = "src/lib.rs"
  ```

- [ ] **Step 2: Create `server/src/lib.rs`**

  ```rust
  pub mod agent;
  pub mod app_state;
  pub mod article_selection;
  pub mod clickhouse;
  pub mod cross_filter;
  pub mod db;
  pub mod db_config;
  pub mod graph;
  pub mod handlers;
  pub mod instance_config;
  pub mod pg_pools;
  pub mod pipeline;
  pub mod pipeline_assemblies;
  pub mod query;
  pub mod router;
  pub mod seed;
  pub mod service;
  pub mod services;
  pub mod trace_db;
  pub mod uam;

  pub use app_state::{ActiveRun, AppState};
  pub use router::build_router;
  ```

- [ ] **Step 3: Create `server/src/app_state.rs`**

  Move `ActiveRun` and `AppState` verbatim from `main.rs` (the two structs and their doc comments, approximately lines 28–125). The file starts with:

  ```rust
  use std::sync::Arc;

  pub struct ActiveRun {
      pub pipeline_id: String,
      pub started_at: std::time::Instant,
      pub cancel: tokio_util::sync::CancellationToken,
  }

  pub struct AppState {
      pub db: crate::db::Database,
      pub parquet_home: String,
      pub traces: crate::trace_db::TraceManager,
      pub duckdb_path: String,
      pub data_dir: String,
      pub db_path: String,
      pub port: String,
      pub cdc_manager: cdc::CdcManager,
      pub pg_pool: tokio::sync::Mutex<std::collections::HashMap<String, tokio_postgres::Client>>,
      pub tenant_id: String,
      pub client: String,
      pub app_type: String,
      pub environment: String,
      pub pipeline_run_lock: Arc<tokio::sync::Mutex<()>>,
      pub active_run: Arc<tokio::sync::RwLock<Option<ActiveRun>>>,
      pub pipeline_progress_interval: Option<std::time::Duration>,
      pub rcl_store: tokio::sync::RwLock<Option<Arc<rcl::RuleStore>>>,
      pub cdc_change_tx: tokio::sync::broadcast::Sender<
          crate::services::pipeline_scheduler::CdcChangeEvent,
      >,
      pub article_selection_store: Arc<crate::article_selection::ArticleSelectionStore>,
      pub legacy_graph: Arc<arc_swap::ArcSwapOption<crate::graph::legacy::ArticleGraph>>,
      pub graphs: Arc<
          tokio::sync::RwLock<
              std::collections::HashMap<
                  String,
                  Arc<arc_swap::ArcSwapOption<crate::graph::Graph>>,
              >,
          >,
      >,
      pub default_graph_id: Option<String>,
      pub uam: Arc<crate::uam::UamStore>,
      pub agent: Arc<crate::agent::AgentState>,
  }
  ```

- [ ] **Step 4: Create `server/src/router.rs`**

  Move the `let api = Router::new() ... .with_state(state.clone());` block from `main.rs` (lines 302–506) into a standalone function. The resulting file:

  ```rust
  use std::sync::Arc;
  use axum::{Router, routing::{get, post, put, delete}};
  use crate::{handlers, agent};
  use crate::AppState;

  pub fn build_router(state: Arc<AppState>) -> Router {
      Router::new()
          .route("/health", get(handlers::health))
          .route("/identity", get(handlers::identity))
          // ── paste the full route chain from main.rs lines 307–504 here ──
          .merge(agent::router())
          .with_state(state)
  }
  ```

  The body is a verbatim paste of every `.route(...)` call from `main.rs` lines 307–504. Remove the `let api =` assignment and the trailing `; ` — the whole chain is the function body returned directly.

  Change the final `.with_state(state.clone())` to `.with_state(state)` since the function now owns the Arc.

- [ ] **Step 5: Slim down `server/src/main.rs`**

  a. Remove the 18 `mod` declarations at the top (lines 1–18) — they are now in `lib.rs`.

  b. Remove the `ActiveRun` and `AppState` struct definitions (everything between `#[derive(Clone)] pub struct ActiveRun` and the closing `}` of `AppState`).

  c. Replace the entire `let api = Router::new() ... .with_state(state.clone());` block with a single line:
  ```rust
  let api = smartstudio_server::build_router(state.clone());
  ```

  d. Add these imports after the existing `use axum::*` block:
  ```rust
  use smartstudio_server::{
      agent, article_selection, db, graph, handlers, instance_config,
      pg_pools, query, seed, service, services, trace_db, uam,
      AppState, ActiveRun,
  };
  ```

  e. Update the two `crate::query::pg_conn_str(...)` calls in the helper functions at the bottom of the file (lines 671 and 831) → `query::pg_conn_str(...)`.

  f. Update `crate::graph::legacy::NodeKind::Article` (line 619) → `graph::legacy::NodeKind::Article`.

- [ ] **Step 6: Verify it compiles**

  Run from `server/`:
  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && cargo build 2>&1 | tail -20
  ```
  Expected: `Compiling smartstudio-server ... Finished`. Zero errors.

- [ ] **Step 7: Commit**

  ```bash
  git add server/src/lib.rs server/src/app_state.rs server/src/router.rs \
          server/src/main.rs server/Cargo.toml
  git commit -m "refactor(server): add lib.rs, extract AppState + build_router for integration tests"
  ```

---

### Task 2: Add dev-dependencies + shared test helper

**Files:**
- Modify: `server/Cargo.toml`
- Create: `server/tests/common/mod.rs`

**Interfaces:**
- Produces: `common::setup_server() -> (TestServer, TempDir)` — used by Tasks 3–6

- [ ] **Step 1: Add dev-dependencies to `server/Cargo.toml`**

  Add after the `[dependencies]` block:

  ```toml
  [dev-dependencies]
  axum-test = "17"
  tempfile = "3"
  ```

- [ ] **Step 2: Create `server/tests/common/mod.rs`**

  ```rust
  use std::sync::Arc;
  use axum_test::TestServer;
  use tempfile::TempDir;
  use smartstudio_server::{
      agent, article_selection, db, graph, trace_db, uam,
      AppState, build_router,
  };

  pub async fn setup_server() -> (TestServer, TempDir) {
      let tmp = tempfile::tempdir().expect("tempdir");
      let home = tmp.path().to_str().unwrap().to_string();

      // Write a minimal environment.toml into the temp dir.
      // is_new=false: skip template-copy seed; ensure_ready still creates the DB schema.
      let toml = format!(
          r#"home_path = "{home}"
  client = "test"
  app_type = "test"
  environment = "test"
  is_new = false

  [server]
  port = "13001"
  grpc_port = 50052

  [rcl]
  enabled = false
  "#,
          home = home
      );
      let toml_path = tmp.path().join("environment.toml");
      std::fs::write(&toml_path, toml).unwrap();

      // Resolve config and create directory + SQLite schema.
      let cfg = smartstudio_server::instance_config::load(&toml_path)
          .expect("load config");
      let resolved = smartstudio_server::instance_config::resolve(cfg)
          .expect("resolve config");
      smartstudio_server::instance_config::ensure_ready(&resolved)
          .expect("ensure_ready");

      // Open databases.
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
  ```

- [ ] **Step 3: Verify the test helper compiles**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test --test common --no-run 2>&1 | tail -10
  ```
  Expected: `Finished test [unoptimized + debuginfo]`. Zero errors.

- [ ] **Step 4: Commit**

  ```bash
  git add server/Cargo.toml server/tests/common/mod.rs
  git commit -m "test(server): add axum-test dev-dep + shared setup_server() helper"
  ```

---

### Task 3: Health + identity integration tests

**Files:**
- Create: `server/tests/test_health.rs`

**Interfaces:**
- Consumes: `common::setup_server()`

- [ ] **Step 1: Write `server/tests/test_health.rs`**

  ```rust
  mod common;

  #[tokio::test]
  async fn health_returns_200() {
      let (server, _tmp) = common::setup_server().await;
      let resp = server.get("/health").await;
      resp.assert_status_ok();
  }

  #[tokio::test]
  async fn identity_returns_200_with_expected_shape() {
      let (server, _tmp) = common::setup_server().await;
      let resp = server.get("/identity").await;
      resp.assert_status_ok();
      let body: serde_json::Value = resp.json();
      assert!(body.get("id").and_then(|v| v.as_str()).is_some(), "missing id");
      assert!(body.get("client").and_then(|v| v.as_str()).is_some(), "missing client");
      assert!(body.get("app_type").and_then(|v| v.as_str()).is_some(), "missing app_type");
      assert!(body.get("environment").and_then(|v| v.as_str()).is_some(), "missing environment");
      assert!(body.get("display_name").and_then(|v| v.as_str()).is_some(), "missing display_name");
      // tenant_id = client-app_type-environment
      let id = body["id"].as_str().unwrap();
      assert_eq!(id, "test-test-test");
  }
  ```

- [ ] **Step 2: Run tests**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test --test test_health 2>&1 | tail -15
  ```
  Expected: `test health_returns_200 ... ok`, `test identity_returns_200_with_expected_shape ... ok`

- [ ] **Step 3: Commit**

  ```bash
  git add server/tests/test_health.rs
  git commit -m "test(server): health + identity integration tests"
  ```

---

### Task 4: CRUD integration tests (8 resources)

**Files:**
- Create: `server/tests/test_crud.rs`

**Interfaces:**
- Consumes: `common::setup_server()`

- [ ] **Step 1: Write `server/tests/test_crud.rs`**

  ```rust
  mod common;

  // ── connections ──────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn connections_create_and_read() {
      let (server, _tmp) = common::setup_server().await;
      let body = serde_json::json!({
          "id": "conn-test-1",
          "display_name": "Test Connection",
          "type": "pg",
          "config": { "host": "localhost", "port": 5432, "user": "u", "password": "p", "database": "d" }
      });
      let create = server.post("/connections").json(&body).await;
      create.assert_status(axum::http::StatusCode::CREATED);

      let get = server.get("/connections/conn-test-1").await;
      get.assert_status_ok();
      let resp: serde_json::Value = get.json();
      assert_eq!(resp["id"], "conn-test-1");
      assert_eq!(resp["display_name"], "Test Connection");
  }

  #[tokio::test]
  async fn connections_get_missing_returns_404() {
      let (server, _tmp) = common::setup_server().await;
      server.get("/connections/no-such-id").await.assert_status_not_found();
  }

  // ── sources ──────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn sources_create_and_read() {
      let (server, _tmp) = common::setup_server().await;
      let body = serde_json::json!({
          "id": "src-test-1",
          "display_name": "Test Source",
          "kind": "duckdb_table",
          "config": { "table_name": "test_table" }
      });
      let create = server.post("/sources").json(&body).await;
      create.assert_status(axum::http::StatusCode::CREATED);

      let get = server.get("/sources/src-test-1").await;
      get.assert_status_ok();
      let resp: serde_json::Value = get.json();
      assert_eq!(resp["id"], "src-test-1");
      assert_eq!(resp["kind"], "duckdb_table");
  }

  #[tokio::test]
  async fn sources_get_missing_returns_404() {
      let (server, _tmp) = common::setup_server().await;
      server.get("/sources/no-such-id").await.assert_status_not_found();
  }

  // ── dataviews ────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn dataviews_create_and_read() {
      let (server, _tmp) = common::setup_server().await;
      let body = serde_json::json!({
          "id": "dv-test-1",
          "display_name": "Test DataView"
      });
      let create = server.post("/dataviews").json(&body).await;
      create.assert_status(axum::http::StatusCode::CREATED);

      let get = server.get("/dataviews/dv-test-1").await;
      get.assert_status_ok();
      let resp: serde_json::Value = get.json();
      assert_eq!(resp["id"], "dv-test-1");
      assert_eq!(resp["display_name"], "Test DataView");
  }

  #[tokio::test]
  async fn dataviews_get_missing_returns_404() {
      let (server, _tmp) = common::setup_server().await;
      server.get("/dataviews/no-such-id").await.assert_status_not_found();
  }

  // ── pipelines ────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn pipelines_create_and_read() {
      let (server, _tmp) = common::setup_server().await;
      let body = serde_json::json!({
          "id": "pl-test-1",
          "display_name": "Test Pipeline"
      });
      let create = server.post("/pipelines").json(&body).await;
      create.assert_status(axum::http::StatusCode::CREATED);

      let get = server.get("/pipelines/pl-test-1").await;
      get.assert_status_ok();
      let resp: serde_json::Value = get.json();
      assert_eq!(resp["id"], "pl-test-1");
      assert_eq!(resp["display_name"], "Test Pipeline");
  }

  #[tokio::test]
  async fn pipelines_get_missing_returns_404() {
      let (server, _tmp) = common::setup_server().await;
      server.get("/pipelines/no-such-id").await.assert_status_not_found();
  }

  // ── modules ──────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn modules_create_and_read() {
      let (server, _tmp) = common::setup_server().await;
      let body = serde_json::json!({
          "id": "mod-test-1",
          "display_name": "Test Module",
          "route": "/test",
          "icon": "table",
          "permission_key": "test"
      });
      let create = server.post("/modules").json(&body).await;
      create.assert_status(axum::http::StatusCode::CREATED);

      let get = server.get("/modules").await;
      get.assert_status_ok();
      let list: Vec<serde_json::Value> = get.json();
      assert!(list.iter().any(|m| m["id"] == "mod-test-1"),
              "mod-test-1 not found in list");
  }

  #[tokio::test]
  async fn modules_get_missing_returns_404() {
      let (server, _tmp) = common::setup_server().await;
      server.get("/modules/no-such-id").await.assert_status_not_found();
  }

  // ── dimensions ───────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn dimensions_create_and_read() {
      let (server, _tmp) = common::setup_server().await;
      // dimensions need a connection reference; create one first
      server.post("/connections").json(&serde_json::json!({
          "id": "conn-for-dim", "display_name": "Dim Conn", "type": "pg", "config": {}
      })).await;
      let body = serde_json::json!({
          "id": "dim-test-1",
          "display_name": "Test Dimension",
          "master_table": "product_attributes",
          "datasource_ref": "conn-for-dim"
      });
      let create = server.post("/dimensions").json(&body).await;
      create.assert_status(axum::http::StatusCode::CREATED);

      let get = server.get("/dimensions").await;
      get.assert_status_ok();
      let list: Vec<serde_json::Value> = get.json();
      assert!(list.iter().any(|d| d["id"] == "dim-test-1"),
              "dim-test-1 not found in list");
  }

  // ── filter-configs ───────────────────────────────────────────────────────────

  #[tokio::test]
  async fn filter_configs_create_and_read() {
      let (server, _tmp) = common::setup_server().await;
      let body = serde_json::json!({
          "id": "fc-test-1",
          "display_name": "Test Filter Config",
          "dimension_ref": "dim-product"
      });
      let create = server.post("/filter-configs").json(&body).await;
      create.assert_status(axum::http::StatusCode::CREATED);

      let get = server.get("/filter-configs/fc-test-1").await;
      get.assert_status_ok();
      let resp: serde_json::Value = get.json();
      assert_eq!(resp["id"], "fc-test-1");
      assert_eq!(resp["dimension_ref"], "dim-product");
  }

  #[tokio::test]
  async fn filter_configs_get_missing_returns_404() {
      let (server, _tmp) = common::setup_server().await;
      server.get("/filter-configs/no-such-id").await.assert_status_not_found();
  }

  // ── templates ────────────────────────────────────────────────────────────────

  #[tokio::test]
  async fn templates_create_and_list() {
      let (server, _tmp) = common::setup_server().await;
      let body = serde_json::json!({
          "id": "tmpl-test-1",
          "display_name": "Test Template",
          "description": "A test template"
      });
      let create = server.post("/templates").json(&body).await;
      create.assert_status(axum::http::StatusCode::CREATED);

      let get = server.get("/templates").await;
      get.assert_status_ok();
      let list: Vec<serde_json::Value> = get.json();
      assert!(list.iter().any(|t| t["id"] == "tmpl-test-1"),
              "tmpl-test-1 not found in list");
  }
  ```

- [ ] **Step 2: Run tests**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test --test test_crud 2>&1 | tail -25
  ```
  Expected: all 16 tests pass (`ok`). If any handler returns a different status code or field name, adjust the test body to match — the handler is the source of truth; the test must adapt.

- [ ] **Step 3: Commit**

  ```bash
  git add server/tests/test_crud.rs
  git commit -m "test(server): CRUD integration tests for 8 resources"
  ```

---

### Task 5: Code generation integration tests

**Files:**
- Create: `server/tests/test_generate.rs`

**Interfaces:**
- Consumes: `common::setup_server()`

- [ ] **Step 1: Write `server/tests/test_generate.rs`**

  ```rust
  mod common;

  async fn create_minimal_dataview(server: &axum_test::TestServer) -> String {
      let dv_id = "dv-gen-test-1";
      server.post("/dataviews").json(&serde_json::json!({
          "id": dv_id,
          "display_name": "Gen Test View",
          "columns": [{"name": "id", "type": "VARCHAR", "visible": true}],
          "contract": {
              "grpc_service": "gen_test",
              "grpc_method": "list_gen_test"
          }
      })).await;
      dv_id.to_string()
  }

  #[tokio::test]
  async fn generate_preview_returns_six_expected_file_keys() {
      let (server, _tmp) = common::setup_server().await;
      let dv_id = create_minimal_dataview(&server).await;

      let resp = server
          .post(&format!("/generate/dataview/{dv_id}/preview"))
          .await;
      resp.assert_status_ok();

      let body: serde_json::Value = resp.json();
      let files = body.get("files")
          .expect("response missing 'files' key")
          .as_object()
          .expect("'files' is not an object");

      // The six required keys — key names depend on the DataView's grpc_service
      assert!(files.keys().any(|k| k.ends_with(".proto")),
              "no .proto key in files: {:?}", files.keys().collect::<Vec<_>>());
      assert!(files.contains_key("Cargo.toml"),      "missing Cargo.toml");
      assert!(files.contains_key("build.rs"),        "missing build.rs");
      assert!(files.contains_key("src/main.rs"),     "missing src/main.rs");
      assert!(files.contains_key("src/service.rs"),  "missing src/service.rs");
      assert!(files.contains_key("src/rest.rs"),     "missing src/rest.rs");
      assert_eq!(files.len(), 6, "expected exactly 6 file keys, got {}", files.len());
  }

  #[tokio::test]
  async fn generate_write_creates_files_on_disk() {
      let (server, _tmp) = common::setup_server().await;
      let dv_id = create_minimal_dataview(&server).await;

      let resp = server
          .post(&format!("/generate/dataview/{dv_id}/write"))
          .await;
      resp.assert_status_ok();

      let body: serde_json::Value = resp.json();
      let files_written = body.get("files_written")
          .expect("missing 'files_written'")
          .as_array()
          .expect("'files_written' is not an array");

      assert!(!files_written.is_empty(), "files_written is empty");
      for path_val in files_written {
          let path = path_val.as_str().expect("path is not a string");
          assert!(
              std::path::Path::new(path).exists(),
              "generated file does not exist on disk: {path}"
          );
      }
  }
  ```

- [ ] **Step 2: Run tests**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test --test test_generate 2>&1 | tail -15
  ```
  Expected: both tests pass.

- [ ] **Step 3: Commit**

  ```bash
  git add server/tests/test_generate.rs
  git commit -m "test(server): generate preview + write integration tests"
  ```

---

### Task 6: Integration canary tests (bundle, db_path, templates)

**Files:**
- Create: `server/tests/test_canaries.rs`
- Create: `server/tests/test_template_canaries.rs`

**Interfaces:**
- Consumes: `common::setup_server()`
- These tests are GREEN now; `test_canaries.rs` stays GREEN after rename; `test_template_canaries.rs` turns RED after the Tera templates are updated in Task 13.

- [ ] **Step 1: Write `server/tests/test_canaries.rs`**

  ```rust
  mod common;

  /// Canary: bundle export filename contains "smartstudio-bundle-".
  /// Stays GREEN — bundle filename is NOT renamed.
  #[tokio::test]
  async fn bundle_export_content_disposition_contains_smartstudio_bundle() {
      let (server, _tmp) = common::setup_server().await;
      // POST with an empty selection to trigger the export path
      let resp = server
          .post("/bundle/export")
          .json(&serde_json::json!({ "kinds": {} }))
          .await;
      resp.assert_status_ok();
      let cd = resp
          .headers()
          .get("content-disposition")
          .expect("missing Content-Disposition header")
          .to_str()
          .expect("non-ASCII Content-Disposition");
      assert!(
          cd.contains("smartstudio-bundle-"),
          "Content-Disposition does not contain 'smartstudio-bundle-': {cd}"
      );
  }

  /// Canary: resolved db_path ends with "smartstudio.db".
  /// Turns RED when the SQLite filename is renamed.
  #[tokio::test]
  async fn sqlite_filename_is_smartstudio_db() {
      let (_server, _tmp) = common::setup_server().await;
      // We need the db_path from AppState. Re-derive it via the config chain.
      let tmp2 = tempfile::tempdir().unwrap();
      let home = tmp2.path().to_str().unwrap();
      let toml = format!(
          r#"home_path = "{home}"
  client = "test"
  app_type = "test"
  environment = "test"
  is_new = false
  [server]
  port = "13002"
  grpc_port = 50053
  [rcl]
  enabled = false
  "#
      );
      let toml_path = tmp2.path().join("environment.toml");
      std::fs::write(&toml_path, toml).unwrap();
      let cfg = smartstudio_server::instance_config::load(&toml_path).unwrap();
      let resolved = smartstudio_server::instance_config::resolve(cfg).unwrap();
      assert!(
          resolved.db_path.ends_with("smartstudio.db"),
          "db_path '{}' does not end with 'smartstudio.db'",
          resolved.db_path
      );
  }
  ```

- [ ] **Step 2: Write `server/tests/test_template_canaries.rs`**

  ```rust
  mod common;

  async fn create_dv_for_templates(server: &axum_test::TestServer) -> String {
      let dv_id = "dv-tpl-canary";
      server.post("/dataviews").json(&serde_json::json!({
          "id": dv_id,
          "display_name": "Template Canary View",
          "contract": { "grpc_service": "tpl_canary", "grpc_method": "list_tpl_canary" }
      })).await;
      dv_id.to_string()
  }

  /// Canary: proto file comment contains "Generated from SmartStudio".
  /// Turns RED after Tera templates are updated in Task 13.
  #[tokio::test]
  async fn generated_proto_contains_smartstudio_comment() {
      let (server, _tmp) = common::setup_server().await;
      let dv_id = create_dv_for_templates(&server).await;

      let resp = server.post(&format!("/generate/dataview/{dv_id}/preview")).await;
      resp.assert_status_ok();
      let body: serde_json::Value = resp.json();
      let files = body["files"].as_object().unwrap();
      let proto_content = files
          .iter()
          .find(|(k, _)| k.ends_with(".proto"))
          .map(|(_, v)| v.as_str().unwrap_or(""))
          .expect("no .proto file in preview response");

      assert!(
          proto_content.contains("Generated from SmartStudio"),
          "proto file does not contain 'Generated from SmartStudio'"
      );
  }

  /// Canary: service.rs comment contains "Generated by SmartStudio".
  /// Turns RED after Tera templates are updated in Task 13.
  #[tokio::test]
  async fn generated_service_rs_contains_smartstudio_comment() {
      let (server, _tmp) = common::setup_server().await;
      let dv_id = create_dv_for_templates(&server).await;

      let resp = server.post(&format!("/generate/dataview/{dv_id}/preview")).await;
      resp.assert_status_ok();
      let body: serde_json::Value = resp.json();
      let service_content = body["files"]["src/service.rs"]
          .as_str()
          .expect("src/service.rs not in preview response");

      assert!(
          service_content.contains("Generated by SmartStudio"),
          "src/service.rs does not contain 'Generated by SmartStudio'"
      );
  }
  ```

- [ ] **Step 3: Run both canary test files**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test --test test_canaries --test test_template_canaries 2>&1 | tail -15
  ```
  Expected: all 4 tests pass (GREEN).

- [ ] **Step 4: Commit**

  ```bash
  git add server/tests/test_canaries.rs server/tests/test_template_canaries.rs
  git commit -m "test(server): integration canary tests — bundle filename, db_path, template strings"
  ```

---

### Task 7: Inline unit canaries (constants + env var names)

**Files:**
- Modify: `server/src/instance_config.rs`
- Modify: `server/src/pg_pools.rs`
- Modify: `server/src/graph/parity.rs`

**Interfaces:**
- These tests live inside `#[cfg(test)]` blocks in their source files, accessing private items. GREEN now; the `instance_config` canary turns RED when `NAMESPACE_DIR` is renamed.

- [ ] **Step 1: Add canary to `server/src/instance_config.rs`**

  Append to the end of the file:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      /// Canary: NAMESPACE_DIR == "smartstudio".
      /// Turns RED when the data directory path segment is renamed.
      #[test]
      fn namespace_dir_is_smartstudio() {
          assert_eq!(NAMESPACE_DIR, "smartstudio");
      }
  }
  ```

- [ ] **Step 2: Add canary to `server/src/pg_pools.rs`**

  Append to the end of the file:

  ```rust
  #[cfg(test)]
  mod tests {
      /// Canary: pg pool reads SMARTSTUDIO_PG_MAX_CONCURRENCY.
      /// Stays GREEN — this env var is NOT renamed.
      #[test]
      fn pg_max_concurrency_env_var_name() {
          // Set the env var and verify default_max_size() picks it up.
          std::env::set_var("SMARTSTUDIO_PG_MAX_CONCURRENCY", "7");
          let result = super::default_max_size();
          std::env::remove_var("SMARTSTUDIO_PG_MAX_CONCURRENCY");
          assert_eq!(result, 7, "default_max_size() did not read SMARTSTUDIO_PG_MAX_CONCURRENCY");
      }
  }
  ```

  If `default_max_size` is private, make it `pub(crate)`:
  ```rust
  pub(crate) fn default_max_size() -> usize {
  ```

- [ ] **Step 3: Add canary to `server/src/graph/parity.rs`**

  Append to the end of the file (in addition to the existing `#[ignore]` test):

  ```rust
  #[cfg(test)]
  mod canary_tests {
      /// Canary: env_duckdb_path reads SMARTSTUDIO_BEALLS_DUCKDB.
      /// Stays GREEN — this env var is NOT renamed.
      #[test]
      fn bealls_duckdb_env_var_name() {
          std::env::set_var("SMARTSTUDIO_BEALLS_DUCKDB", "/tmp/canary_test.duckdb");
          let result = super::env_duckdb_path();
          std::env::remove_var("SMARTSTUDIO_BEALLS_DUCKDB");
          assert_eq!(
              result,
              Some("/tmp/canary_test.duckdb".to_string()),
              "env_duckdb_path() did not read SMARTSTUDIO_BEALLS_DUCKDB"
          );
      }
  }
  ```

  If `env_duckdb_path` is private, make it `pub(crate)`:
  ```rust
  pub(crate) fn env_duckdb_path() -> Option<String> {
  ```

- [ ] **Step 4: Run the unit canaries**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test namespace_dir_is_smartstudio pg_max_concurrency_env_var_name bealls_duckdb_env_var_name \
    2>&1 | tail -10
  ```
  Expected: all 3 pass (GREEN).

- [ ] **Step 5: Commit**

  ```bash
  git add server/src/instance_config.rs server/src/pg_pools.rs server/src/graph/parity.rs
  git commit -m "test(server): inline unit canaries — NAMESPACE_DIR, PG env var, parity env var"
  ```

---

### Task 8: Run full Rust test suite — fix until green

**Files:** None new; fix compilation errors if any.

- [ ] **Step 1: Run all Rust tests**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test 2>&1 | tail -30
  ```

- [ ] **Step 2: Interpret output**

  All tests should pass. Common failures and fixes:
  - **Compilation error in `common/mod.rs`**: a field name mismatch in `AppState` construction → check `main.rs`'s original AppState field names and update `common/mod.rs`.
  - **Test returns wrong status**: a handler uses 200 instead of 201 → update the `assert_status` call in `test_crud.rs` to match.
  - **`env_duckdb_path` or `default_max_size` is private**: add `pub(crate)` as noted in Task 7.
  - **`generate_preview` test fails on file key names**: the exact proto key name is `proto/<grpc_service>.proto` (e.g., `proto/tpl_canary.proto`) — the test uses `.ends_with(".proto")` which handles this.

- [ ] **Step 3: Commit once green**

  ```bash
  git add -A
  git commit -m "test(server): full Rust test suite green"
  ```

---

### Task 9: Vitest infrastructure

**Files:**
- Modify: `package.json`
- Modify: `vite.config.ts`
- Create: `src/setupTests.ts`

- [ ] **Step 1: Add Vitest dev-dependencies**

  In `package.json` (root), add to `"devDependencies"`:
  ```json
  "vitest": "^3.0.0",
  "@vitest/coverage-v8": "^3.0.0",
  "@testing-library/react": "^16.0.0",
  "@testing-library/jest-dom": "^6.0.0",
  "@testing-library/user-event": "^14.0.0",
  "jsdom": "^26.0.0",
  "@types/jsdom": "^21.0.0"
  ```

  Add to `"scripts"`:
  ```json
  "test": "vitest run"
  ```

  Install:
  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio" && npm install
  ```

- [ ] **Step 2: Add test config block to `vite.config.ts`**

  The file currently exports `defineConfig({ plugins, resolve, build, server })`. Add a `test` block:

  ```ts
  import { defineConfig } from 'vite'
  import react from '@vitejs/plugin-react'
  import tailwindcss from '@tailwindcss/vite'
  import path from 'path'

  export default defineConfig({
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: {
        '@': path.resolve(__dirname, './src'),
      },
    },
    build: {
      rollupOptions: {
        input: {
          main: path.resolve(__dirname, 'index.html'),
          agent: path.resolve(__dirname, 'agent.html'),
        },
      },
    },
    server: {
      proxy: {
        '/api': {
          target: 'http://localhost:3001',
          changeOrigin: true,
        },
      },
    },
    test: {
      globals: true,
      environment: 'jsdom',
      setupFiles: ['./src/setupTests.ts'],
    },
  })
  ```

- [ ] **Step 3: Create `src/setupTests.ts`**

  ```ts
  import '@testing-library/jest-dom'
  ```

- [ ] **Step 4: Verify config is valid**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio" && npx vitest run --reporter=verbose 2>&1 | head -20
  ```
  Expected: "No test files found" (zero tests yet, but no config error).

- [ ] **Step 5: Commit**

  ```bash
  git add package.json vite.config.ts src/setupTests.ts package-lock.json
  git commit -m "test(frontend): add Vitest + @testing-library/react infrastructure"
  ```

---

### Task 10: Frontend tests (smoke + 3 component canaries)

**Files:**
- Create: `src/__tests__/App.smoke.test.tsx`
- Create: `src/__tests__/WorkspaceLayout.canary.test.tsx`
- Create: `src/__tests__/AgentApp.canary.test.tsx`
- Create: `src/__tests__/CoreServiceWorkspace.canary.test.tsx`

- [ ] **Step 1: Create `src/__tests__/App.smoke.test.tsx`**

  ```tsx
  import { render, screen, waitFor } from '@testing-library/react'
  import { vi } from 'vitest'

  vi.mock('@/api/client', () => ({
    api: {
      getIdentity: vi.fn().mockResolvedValue({
        id: 'test-tenant',
        client: 'test',
        app_type: 'test',
        environment: 'test',
        display_name: 'Test',
      }),
    },
  }))

  // Stub the activity SSE hook so it doesn't open a real EventSource.
  vi.mock('@/hooks/useActivitySSE', () => ({
    useActivitySSE: () => ({
      notifications: [],
      setNotifications: vi.fn(),
      readIdsRef: { current: new Set() },
      saveReadIds: vi.fn(),
      reload: vi.fn(),
    }),
  }))

  import App from '@/App'

  test('App renders without crashing and shows section tabs', async () => {
    render(<App />)
    // Tabs appear once identity resolves.
    await waitFor(() => {
      expect(screen.getByText('DataViews')).toBeInTheDocument()
    })
  })
  ```

- [ ] **Step 2: Create `src/__tests__/WorkspaceLayout.canary.test.tsx`**

  ```tsx
  import { render, screen } from '@testing-library/react'
  import { vi } from 'vitest'

  vi.mock('@/hooks/useActivitySSE', () => ({
    useActivitySSE: () => ({
      notifications: [],
      setNotifications: vi.fn(),
      readIdsRef: { current: new Set() },
      saveReadIds: vi.fn(),
      reload: vi.fn(),
    }),
  }))

  vi.mock('@/components/Sidebar', () => ({ default: () => <div /> }))

  import { WorkspaceLayout } from '@/layouts/WorkspaceLayout'

  const identity = {
    id: 'test-tenant',
    client: 'test',
    app_type: 'test',
    environment: 'test',
    display_name: 'Test',
  }

  /** Canary: brand label renders "SmartStudio".
   *  Turns RED after WorkspaceLayout.tsx is updated in Task 14. */
  test('sidebar brand label renders SmartStudio', () => {
    render(<WorkspaceLayout tenantId="test-tenant" identity={identity} workspace={<div />} />)
    expect(screen.getByText('SmartStudio')).toBeInTheDocument()
  })
  ```

- [ ] **Step 3: Create `src/__tests__/AgentApp.canary.test.tsx`**

  ```tsx
  import { readFileSync } from 'fs'
  import { resolve } from 'path'

  /** Canary: agent/App.tsx heading contains "SmartStudio Agent".
   *  Uses source-file check to avoid mounting the complex agent component tree.
   *  Turns RED after agent/App.tsx is updated in Task 14. */
  test('agent App heading contains SmartStudio Agent', () => {
    const src = readFileSync(resolve(__dirname, '../../agent/App.tsx'), 'utf8')
    expect(src).toContain('SmartStudio Agent')
  })
  ```

- [ ] **Step 4: Create `src/__tests__/CoreServiceWorkspace.canary.test.tsx`**

  ```tsx
  import { readFileSync } from 'fs'
  import { resolve } from 'path'

  /** Canary: CoreServiceWorkspace default GCS placeholder contains "smartstudio-data".
   *  Stays GREEN — this GCS path is NOT renamed.  */
  test('CoreServiceWorkspace GCS placeholder contains smartstudio-data', () => {
    const src = readFileSync(
      resolve(__dirname, '../../components/workspace/CoreServiceWorkspace.tsx'),
      'utf8'
    )
    expect(src).toContain('smartstudio-data')
  })
  ```

- [ ] **Step 5: Run frontend tests**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio" && npm test 2>&1 | tail -20
  ```
  Expected: 4 tests pass. Common fixes:
  - `WorkspaceLayout` imports a Zustand store that calls hooks at module level → mock `@/stores/workspace` with `vi.mock('@/stores/workspace', () => ({ useWorkspaceStore: (fn: any) => fn({ activeTab: 'dataview', setActiveTab: vi.fn(), selected: null, inspectorOpen: false, activityOpen: false, unreadCount: 0, sidebarSearch: '' }) }))`.
  - If `BundleModal` or `SettingsModal` imports break render → add simple stubs: `vi.mock('@/components/BundleModal', () => ({ BundleModal: () => null }))`.

- [ ] **Step 6: Commit**

  ```bash
  git add src/__tests__/ src/setupTests.ts
  git commit -m "test(frontend): smoke test + 3 component canary tests"
  ```

---

### Task 11: MCP server canary test

**Files:**
- Create: `mcp-server/vitest.config.ts`
- Create: `mcp-server/src/__tests__/canary.test.ts`
- Modify: `mcp-server/package.json`

- [ ] **Step 1: Create `mcp-server/vitest.config.ts`**

  ```ts
  import { defineConfig } from 'vitest/config'

  export default defineConfig({
    test: {
      globals: true,
      environment: 'node',
    },
  })
  ```

- [ ] **Step 2: Install vitest in mcp-server**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/mcp-server" && \
    npm install --save-dev vitest
  ```

- [ ] **Step 3: Add test script to `mcp-server/package.json`**

  Add to `"scripts"`:
  ```json
  "test": "vitest run"
  ```

- [ ] **Step 4: Create `mcp-server/src/__tests__/canary.test.ts`**

  ```ts
  import { readFileSync } from 'fs'
  import { resolve } from 'path'

  /** Canary: mcp-server reads SMARTSTUDIO_URL to locate the backend.
   *  Stays GREEN — this env var is NOT renamed. */
  test('http.ts reads SMARTSTUDIO_URL env var', () => {
    const src = readFileSync(resolve(__dirname, '../../src/http.ts'), 'utf8')
    expect(src).toContain('SMARTSTUDIO_URL')
  })
  ```

- [ ] **Step 5: Run MCP canary**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/mcp-server" && \
    npm test 2>&1 | tail -10
  ```
  Expected: 1 test passes (GREEN).

- [ ] **Step 6: Commit**

  ```bash
  git add mcp-server/vitest.config.ts mcp-server/src/__tests__/canary.test.ts \
          mcp-server/package.json mcp-server/package-lock.json
  git commit -m "test(mcp): SMARTSTUDIO_URL canary test"
  ```

---

### Task 12: Run full suite — confirm all green

- [ ] **Step 1: Run Rust suite**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test 2>&1 | grep -E "test .* (ok|FAILED|ignored)"
  ```
  Expected: all tests `ok`. Zero `FAILED`.

- [ ] **Step 2: Run frontend suite**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio" && npm test 2>&1 | tail -15
  ```
  Expected: all 4 tests pass.

- [ ] **Step 3: Run MCP suite**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/mcp-server" && \
    npm test 2>&1 | tail -5
  ```
  Expected: 1 test passes.

- [ ] **Step 4: Fix any remaining failures, commit**

  Do not proceed to Task 13 until all three suites are fully green.

---

### Task 13: Rename — Rust backend display strings

**Files:**
- Modify: `server/Cargo.toml`
- Modify: `server/src/main.rs`
- Modify: `server/src/agent/llm.rs`
- Modify: `server/src/agent/tools.rs`
- Modify: `server/templates/grpc-service/proto.tera`
- Modify: `server/templates/grpc-service/cargo_toml.tera`
- Modify: `server/templates/grpc-service/service_rs.tera`
- Modify: `server/templates/grpc-service/main_rs.tera`
- Modify: `server/templates/grpc-service/rest_rs.tera`

**Note:** `handlers/generate.rs`, `handlers/pipeline_v2.rs`, `services/article_selection_grpc.rs`, and `agent/db.rs` contain only internal code comments that reference `smartstudio` — not display strings or API response labels. Leave those files unchanged.

- [ ] **Step 1: Rename package in `server/Cargo.toml`**

  Change:
  ```toml
  name = "smartstudio-server"
  ```
  To:
  ```toml
  name = "graphstudio-server"
  ```

  Also update the `[lib]` section added in Task 1:
  ```toml
  [lib]
  name = "graphstudio_server"
  path = "src/lib.rs"
  ```

- [ ] **Step 2: Update tracing filter in `server/src/main.rs` (line 131)**

  Change:
  ```rust
  .add_directive("smartstudio_server=info".parse().unwrap())
  ```
  To:
  ```rust
  .add_directive("graphstudio_server=info".parse().unwrap())
  ```

  Also update the `use smartstudio_server::*` (or named imports) added in Task 1 to `use graphstudio_server::*`.

  Also update the log line near line 520:
  ```rust
  tracing::info!("SmartStudio server on http://localhost:{}", port);
  ```
  To:
  ```rust
  tracing::info!("GraphStudio server on http://localhost:{}", port);
  ```

- [ ] **Step 3: Update system prompt in `server/src/agent/llm.rs` (line 117)**

  Change:
  ```
  You are a SmartStudio retail-inventory planning assistant.
  ```
  To:
  ```
  You are a GraphStudio retail-inventory planning assistant.
  ```

- [ ] **Step 4: Update tool description in `server/src/agent/tools.rs` (line 322)**

  Change:
  ```rust
  description: "List all DataViews in this SmartStudio tenant. Returns id, display_name and metadata for each. Use this first when the user asks about available data.".into(),
  ```
  To:
  ```rust
  description: "List all DataViews in this GraphStudio tenant. Returns id, display_name and metadata for each. Use this first when the user asks about available data.".into(),
  ```

- [ ] **Step 5: Update Tera templates (5 files)**

  **`server/templates/grpc-service/proto.tera`** — change:
  ```
  // Generated from SmartStudio DataView: {{ dataview_id }}
  ```
  to:
  ```
  // Generated from GraphStudio DataView: {{ dataview_id }}
  ```

  **`server/templates/grpc-service/cargo_toml.tera`** — two changes:
  - `# Generated by SmartStudio from DataView` → `# Generated by GraphStudio from DataView`
  - `# REST (for SmartStudio test harness)` → `# REST (for GraphStudio test harness)`

  **`server/templates/grpc-service/service_rs.tera`** — change:
  ```
  //! Generated by SmartStudio — reads parquet via DuckDB.
  ```
  to:
  ```
  //! Generated by GraphStudio — reads parquet via DuckDB.
  ```

  **`server/templates/grpc-service/main_rs.tera`** — three changes:
  - `//! Generated by SmartStudio from DataView:` → `//! Generated by GraphStudio from DataView:`
  - `//! - REST on :50053 (for SmartStudio test harness)` → `//! - REST on :50053 (for GraphStudio test harness)`
  - `// REST server (for SmartStudio)` → `// REST server (for GraphStudio)`

  **`server/templates/grpc-service/rest_rs.tera`** — change:
  ```
  //! REST endpoints for SmartStudio test harness.
  ```
  to:
  ```
  //! REST endpoints for GraphStudio test harness.
  ```

- [ ] **Step 6: Build to confirm no regressions**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo build 2>&1 | tail -5
  ```
  Expected: `Finished`.

- [ ] **Step 7: Run Rust test suite — note which canaries turn RED**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test 2>&1 | grep -E "(ok|FAILED)"
  ```
  Expected RED (FAILED):
  - `test_template_canaries::generated_proto_contains_smartstudio_comment`
  - `test_template_canaries::generated_service_rs_contains_smartstudio_comment`

  Expected GREEN (ok): everything else, including `namespace_dir_is_smartstudio`, `pg_max_concurrency_env_var_name`, `bealls_duckdb_env_var_name`, `bundle_export_content_disposition_contains_smartstudio_bundle`, `sqlite_filename_is_smartstudio_db`.

- [ ] **Step 8: Commit**

  ```bash
  git add server/Cargo.toml server/src/main.rs server/src/agent/llm.rs \
          server/src/agent/tools.rs server/templates/
  git commit -m "rename(server): SmartStudio → GraphStudio display strings + package name"
  ```

---

### Task 14: Rename — React frontend display strings

**Files:**
- Modify: `src/layouts/WorkspaceLayout.tsx`
- Modify: `src/agent/App.tsx`
- Modify: `src/components/workspace/FilterConfigWorkspace.tsx`
- Modify: `src/components/workspace/GraphDesigner/HierarchyInspector.tsx`
- Modify: `index.html`

**Note:** `src/components/workspace/CoreServiceWorkspace.tsx` is explicitly NOT changed — the GCS path `"smartstudio-data"` is deferred.

- [ ] **Step 1: Update `src/layouts/WorkspaceLayout.tsx` (line 125)**

  Change:
  ```tsx
  <span className="text-blue-500 font-semibold text-sm tracking-wide">
    SmartStudio
  </span>
  ```
  To:
  ```tsx
  <span className="text-blue-500 font-semibold text-sm tracking-wide">
    GraphStudio
  </span>
  ```

- [ ] **Step 2: Update `src/agent/App.tsx` (line 264)**

  Change:
  ```tsx
  <h1 className="text-base font-semibold tracking-tight text-slate-900">SmartStudio Agent</h1>
  ```
  To:
  ```tsx
  <h1 className="text-base font-semibold tracking-tight text-slate-900">GraphStudio Agent</h1>
  ```

- [ ] **Step 3: Update `src/components/workspace/FilterConfigWorkspace.tsx` (line 16)**

  Change:
  ```
  from the SmartStudio `dimensions` table
  ```
  To:
  ```
  from the GraphStudio `dimensions` table
  ```

- [ ] **Step 4: Update `src/components/workspace/GraphDesigner/HierarchyInspector.tsx` (line 106)**

  Change:
  ```tsx
  hint="The TOML key for this hierarchy (e.g. `product`, `store`). Must match a SmartStudio dimension."
  ```
  To:
  ```tsx
  hint="The TOML key for this hierarchy (e.g. `product`, `store`). Must match a GraphStudio dimension."
  ```

- [ ] **Step 5: Update `index.html` (line 7)**

  Change:
  ```html
  <title>smartstudio</title>
  ```
  To:
  ```html
  <title>graphstudio</title>
  ```

- [ ] **Step 6: Run frontend tests — note which canaries turn RED**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio" && npm test 2>&1 | tail -20
  ```
  Expected RED (FAILED):
  - `WorkspaceLayout.canary.test.tsx` — `brand label renders SmartStudio`
  - `AgentApp.canary.test.tsx` — `agent App heading contains SmartStudio Agent`

  Expected GREEN (ok):
  - `App.smoke.test.tsx` — still renders without crash
  - `CoreServiceWorkspace.canary.test.tsx` — `smartstudio-data` still present

- [ ] **Step 7: Commit**

  ```bash
  git add src/layouts/WorkspaceLayout.tsx src/agent/App.tsx \
          src/components/workspace/FilterConfigWorkspace.tsx \
          src/components/workspace/GraphDesigner/HierarchyInspector.tsx \
          index.html
  git commit -m "rename(frontend): SmartStudio → GraphStudio display strings"
  ```

---

### Task 15: Rename — MCP server display strings

**Files:**
- Modify: `mcp-server/package.json`
- Modify: `mcp-server/src/server.ts`
- Modify: `mcp-server/src/index.ts`
- Modify: `mcp-server/src/index-http.ts`
- Modify: `mcp-server/src/tools/clickhouse_query.ts`
- Modify: `mcp-server/src/tools/describe_source.ts`
- Modify: `mcp-server/src/tools/list_sources.ts`
- Modify: `mcp-server/src/tools/materialize.ts`
- Modify: `mcp-server/src/tools/list_graphs.ts`
- Modify: `mcp-server/src/tools/describe_dataview.ts`
- Modify: `mcp-server/src/tools/graph_node.ts`
- Modify: `mcp-server/src/tools/feedback.ts`
- Modify: `mcp-server/src/tools/resolve_filter_values.ts`
- Modify: `mcp-server/src/tools/list_dataviews.ts`

**Do NOT change:** `mcp-server/src/http.ts` (`SMARTSTUDIO_URL` env var — deferred), `mcp-server/src/schema.ts` (lines 3–5 reference `smartstudio.db` — the actual DB filename, not renamed).

- [ ] **Step 1: `mcp-server/package.json`**

  Change `"name": "smartstudio-mcp"` → `"name": "graphstudio-mcp"`.

- [ ] **Step 2: `mcp-server/src/server.ts` (line 16)**

  Change `name: "smartstudio-mcp"` → `name: "graphstudio-mcp"`.

- [ ] **Step 3: `mcp-server/src/index.ts`**

  Change all occurrences of `[smartstudio-mcp]` → `[graphstudio-mcp]`.

- [ ] **Step 4: `mcp-server/src/index-http.ts`**

  - `[smartstudio-mcp]` → `[graphstudio-mcp]` (lines 16, 89, 94)
  - `Reports SmartStudio reachability` → `Reports GraphStudio reachability` (line 37)
  - `smartstudio: ss` → `graphstudio: ss` (line 47) — this is the JSON health-check key visible to Claude

  Do NOT change `SMARTSTUDIO_URL=` in the log line (line 89) — it's the env var name, not a display string.

- [ ] **Step 5: `mcp-server/src/tools/clickhouse_query.ts`**

  Change all occurrences of `smartstudio-side` → `graphstudio-side` (lines 32, 35, 102, 106).

- [ ] **Step 6: Remaining tool files — replace `SmartStudio` → `GraphStudio`**

  Run this sed command for all tool files:
  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/mcp-server/src/tools"
  for f in describe_source.ts list_sources.ts materialize.ts list_graphs.ts \
            describe_dataview.ts graph_node.ts feedback.ts resolve_filter_values.ts \
            list_dataviews.ts; do
    sed -i '' 's/SmartStudio/GraphStudio/g' "$f"
  done
  ```

  Verify no unintended changes:
  ```bash
  grep -l "smartstudio\|SmartStudio" \
    describe_source.ts list_sources.ts materialize.ts list_graphs.ts \
    describe_dataview.ts graph_node.ts feedback.ts resolve_filter_values.ts \
    list_dataviews.ts 2>/dev/null
  ```
  Expected: no output (all replaced).

- [ ] **Step 7: Run MCP canary — confirm it stays GREEN**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/mcp-server" && \
    npm test 2>&1 | tail -5
  ```
  Expected: `SMARTSTUDIO_URL canary` still passes (GREEN) — we did not touch `http.ts`.

- [ ] **Step 8: Commit**

  ```bash
  git add mcp-server/
  git commit -m "rename(mcp): SmartStudio → GraphStudio display strings + package name"
  ```

---

### Task 16: Run full suite + document RED/GREEN results

- [ ] **Step 1: Run Rust suite**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/server" && \
    cargo test 2>&1 | grep -E "test .* \.\.\. (ok|FAILED|ignored)"
  ```

- [ ] **Step 2: Run frontend suite**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio" && npm test -- --reporter=verbose 2>&1 | grep -E "✓|✗|×|PASS|FAIL"
  ```

- [ ] **Step 3: Run MCP suite**

  ```bash
  cd "/Users/hardiksavaliya/Documents/windsurf projects /GraphStudio/GraphStudio/mcp-server" && \
    npm test 2>&1 | tail -5
  ```

- [ ] **Step 4: Print final summary**

  Expected results:

  | Test | File | Result | Guards |
  |---|---|---|---|
  | `health_returns_200` | `test_health.rs` | GREEN | — |
  | `identity_returns_200_with_expected_shape` | `test_health.rs` | GREEN | — |
  | All 16 CRUD tests | `test_crud.rs` | GREEN | — |
  | `generate_preview_returns_six_expected_file_keys` | `test_generate.rs` | GREEN | — |
  | `generate_write_creates_files_on_disk` | `test_generate.rs` | GREEN | — |
  | `bundle_export_content_disposition_contains_smartstudio_bundle` | `test_canaries.rs` | **GREEN** | Bundle filename not renamed |
  | `sqlite_filename_is_smartstudio_db` | `test_canaries.rs` | **GREEN** | SQLite filename not renamed |
  | `generated_proto_contains_smartstudio_comment` | `test_template_canaries.rs` | **RED** | ✓ Templates renamed |
  | `generated_service_rs_contains_smartstudio_comment` | `test_template_canaries.rs` | **RED** | ✓ Templates renamed |
  | `namespace_dir_is_smartstudio` | `instance_config.rs` | **GREEN** | NAMESPACE_DIR not renamed |
  | `pg_max_concurrency_env_var_name` | `pg_pools.rs` | **GREEN** | Env var not renamed |
  | `bealls_duckdb_env_var_name` | `parity.rs` | **GREEN** | Env var not renamed |
  | `App renders without crashing` | `App.smoke.test.tsx` | GREEN | — |
  | `sidebar brand label renders SmartStudio` | `WorkspaceLayout.canary.test.tsx` | **RED** | ✓ Label renamed |
  | `agent App heading contains SmartStudio Agent` | `AgentApp.canary.test.tsx` | **RED** | ✓ Heading renamed |
  | `GCS placeholder contains smartstudio-data` | `CoreServiceWorkspace.canary.test.tsx` | **GREEN** | GCS path not renamed |
  | `http.ts reads SMARTSTUDIO_URL env var` | `mcp-server/canary.test.ts` | **GREEN** | Env var not renamed |

  **RED tests = rename confirmed.** The 4 RED canaries (2 Tera template + 1 WorkspaceLayout + 1 AgentApp) are expected and correct.

  **GREEN deferred canaries** = the 5 items below are explicitly NOT yet renamed and are guarded for a future migration:
  - `NAMESPACE_DIR` → `"smartstudio"` data directory path
  - SQLite filename → `smartstudio.db`
  - `SMARTSTUDIO_PG_MAX_CONCURRENCY` env var
  - `SMARTSTUDIO_BEALLS_DUCKDB` env var
  - `SMARTSTUDIO_URL` env var
  - `smartstudio-data` GCS bucket prefix
  - Bundle export filename prefix `smartstudio-bundle-`

- [ ] **Step 5: Final commit**

  ```bash
  git add -A
  git commit -m "docs: final rename summary — all suites run, canary RED/GREEN documented"
  ```
