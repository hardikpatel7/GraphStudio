# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is GraphStudio?

GraphStudio is a **metadata-driven platform** that configures and generates production-grade inventory management applications (InventorySmart) for retail clients. Each running instance is **one tenant** — one `(client, app_type, environment)` triple (e.g. `briscoes-inventorysmart-demo`). Identity is read from `environment.toml` at startup.

The platform captures app metadata (DataViews, Sources, Pipelines, Dimensions, Modules) via a web UI and generates Rust gRPC services + React frontends for the generated app.

## Commands

All commands run from `GraphStudio/` (the directory with `package.json`).

```bash
# Start frontend (Vite, port 5173) + Rust server (port 3001) together
npm run dev

# Frontend only
npm run dev:client

# Rust server only (single run)
npm run dev:server

# Rust server with file-watch rebuild
npm run dev:server:watch

# Production build (TypeScript check + Vite bundle)
npm run build

# Serve production build locally (port 4173)
npm run preview

# Lint
npm run lint

# Frontend + MCP tests (Vitest)
npm test

# Rust unit + integration tests
cd server && cargo test

# MCP server tests only
cd mcp-server && npm test
```

The Rust server (`server/`) is built via Cargo; `npm run dev:server` wraps `cargo run --manifest-path server/Cargo.toml`. First build takes several minutes due to DuckDB and Tonic compilation.

**MCP server** (separate package in `mcp-server/`):
```bash
cd mcp-server && npm run dev        # stdio transport
cd mcp-server && npm run dev:http   # HTTP transport
```

**Rust dependencies** pull from Bitbucket via SSH (`ssh://git@bitbucket.org:22/insideinsight/rust-shared-utils.git`, branch `develop/dev-v4`). SSH access to that repo is required to build. To use local overrides, uncomment the `[patch.]` block at the bottom of `server/Cargo.toml`.

## Architecture

### Two Processes, One Port in Dev

Vite (`:5173`) proxies `/api` to the Rust Axum server (`:3001`). The Rust server also serves the built `dist/` in production. There are two HTML entry points:
- `index.html` — main GraphStudio editor UI
- `agent.html` — AI agent UI (separate Zustand state tree in `src/agent/`)

### Frontend (`src/`)

Single-page app. Navigation is driven by `useWorkspaceStore` (Zustand) which tracks:
- `activeTab` — which section tab is selected (dataview, pipeline, source, graph, query, …)
- `selected` — the currently selected item within that tab

`App.tsx` renders `WorkspaceLayout` (shell + tabs) and a `WorkspaceContent` switcher that maps `(activeTab, selected.type)` → workspace component.

**Zustand stores** (`src/stores/`) — one per entity type (apps, dataviews, dimensions, filterConfigs, modules, submodules, components, codeGen). All stores use direct `api.` calls; there is no global fetch layer beyond `src/api/client.ts`.

**Path alias**: `@/` maps to `src/` (configured in `vite.config.ts` and `vitest.config.ts`).

**Note:** `vitest.config.ts` is a separate file from `vite.config.ts` due to a type conflict between vite v6 (rolldown) and vitest v3's bundled vite (rollup). The test config (`globals`, `environment`, `setupFiles`) lives in `vitest.config.ts`; build/dev config stays in `vite.config.ts`. `vitest.config.ts` is excluded from `tsconfig.node.json` and carries `// @ts-nocheck`.

### Backend (`server/src/`)

Rust Axum server. The crate is split into a **library** (`graphstudio_server`) and a **binary** (`main.rs`):
- `lib.rs` — re-exports all modules and `build_router()` / `AppState` for integration tests
- `app_state.rs` — `AppState` and `ActiveRun` structs (moved out of `main.rs`)
- `router.rs` — `pub fn build_router(state: Arc<AppState>) -> Router` (extracted from `main.rs`)
- `main.rs` — boot sequence only; calls `build_router()` and adds the static-file fallback

Key modules:

| Module | Role |
|---|---|
| `db` | SQLite wrapper (metadata: connections, sources, dataviews, pipelines, modules, …) |
| `trace_db` | Activity log in per-tenant DuckDB (`log.duckdb`) |
| `handlers/` | Axum route handlers (one file per resource) |
| `service/` | Business logic called by handlers |
| `graph/` | Two coexisting graph implementations: `graph::Graph` (metadata-driven, TOML spec) and `graph::legacy::ArticleGraph` (hand-coded; see `docs/v1-cleanup-todo.md`) |
| `pipeline/` | Pipeline execution (delegates to `pipeline` crate from rust-shared-utils) |
| `agent/` | AI agent subsystem (LLM routing via Rig, usage metering, workspace/session SQLite) |
| `services/` | Long-running background tasks: RCL gRPC (Tonic), pipeline scheduler, CDC auto-start, article-selection gRPC |

**AppState** (defined in `app_state.rs`) is the single `Arc<AppState>` threaded through every handler. It holds live state that can't live in SQLite: the running graph snapshots (`ArcSwapOption`), active pipeline run, CDC manager, agent state, UAM store, etc.

### Data Model (Tenant SQLite)

The tenant's `smartstudio.db` has these tables: `connections`, `sources`, `dataviews`, `pipelines`, `shared_pipeline_steps`, `modules`, `submodules`, `components`, `dimensions`, `filter_configs`, `templates`, `graphs`, `saved_queries`, `derived_tables`, `viewports`.

Runtime artifacts live at `<home_path>/smartstudio/<tenant_id>/data/`:
- `smartstudio.db` — all metadata
- `tenant_data.duckdb` — materialized data warehouse
- `log.duckdb` — activity/trace log
- `parquet/` — pipeline-produced parquet files

### Source Kinds (six)

| Kind | Read mode | Description |
|---|---|---|
| `pg_query` | live | SQL executed against PG on every read |
| `bq_query` | live | SQL executed against BigQuery |
| `duckdb_query` | live | SQL against tenant DuckDB |
| `parquet_glob` | static | `read_parquet()` at a path |
| `duckdb_table` | static | Existing DuckDB table; populated by a Pipeline |
| `cdc_pg` | streaming | PG WAL → DuckDB mirror; auto-resumes on boot |

A Source's **kind cannot be changed** after creation. Source deletion is blocked while DataViews are bound to it.

### Graph Module

Two graph implementations live side-by-side in `server/src/graph/`:
- `graph::Graph` — metadata-driven, built from a TOML spec stored in the `graphs` SQLite table. Loaded/rebuilt via `POST /api/graphs/:id/build`. Stored as `ArcSwapOption<graph::Graph>` in `AppState.graphs`.
- `graph::legacy::ArticleGraph` — hand-coded article-level graph (article → product_code → L0–L5 hierarchy, channel, store_code). Slated for removal; see `docs/v1-cleanup-todo.md`.

The `toml` crate is compiled with `preserve_order` feature because key declaration order in `[hierarchy.*]` and `[metrics.*]` blocks IS the data (top-to-leaf level ordering).

### Boot Sequence

On startup the Rust server:
1. Loads `environment.toml` (via `instance_config`)
2. Pulls LLM API keys from GCP Secret Manager (if `[agent]` section is configured)
3. Opens SQLite + DuckDB
4. Seeds DuckDB views from `data/duckdb_views/*.sql`
5. Seeds Sources from `data/sources/*.toml` and DataViews from `data/dataviews/*.toml`
6. Registers all Axum routes
7. Starts background tasks: graph auto-build, CDC auto-start, pipeline scheduler, UAM cold-load, article-selection rehydration
8. Optionally starts RCL gRPC service on `grpc_port` (default 50051)

### Configuration

`environment.toml` is the required boot config (one instance = one tenant). Key fields:
```toml
home_path   = "/path/to/home"
client      = "bealls"
app_type    = "inventorysmart"
environment = "dev"
is_new      = false   # set true on first boot of a NEW tenant — bootstraps empty SQLite schema; flip back to false after first successful start
[server]
port      = 3001
grpc_port = 50051
[rcl]
enabled = true
[graphs]
default_id = "bealls-inventory-graph"
[agent]
# gcp_project_id / llm_secret_name — omit for local dev; export API keys in shell instead
```

For local dev, LLM keys (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`) can be set via shell `export` rather than GCP Secret Manager.

## Key Design Decisions

1. **Field components must be defined outside parent components** — defining them inline causes React to unmount/remount inputs on every parent render, causing focus loss.
2. **No `useState` during render** — auto-selection logic (module/sub/component) must use `useEffect`, not `useState` initializer.
3. **Passwords are masked** in GET responses (`••••••••`) and preserved on save if the value is still the mask string.
4. **Pipeline serialization** — DuckDB doesn't allow concurrent write connections to the same file. `AppState.pipeline_run_lock` serializes all pipeline runs. Read-only queries (Live View, schema introspect) bypass this lock.
5. **Route order** in `router.rs` — generic parameterized routes like `/pipelines/{id}` must be registered AFTER specific sub-routes like `/pipelines/cancel` and `/pipelines/active`, or Axum will match the literal strings as IDs.
6. **`preserve_order` in TOML** — the `indexmap`-backed TOML deserializer is used specifically for graph specs so key insertion order is preserved (hierarchy level ordering, metric source ordering).

## MCP Server

The `mcp-server/` package exposes GraphStudio's data layer to Claude Code via MCP tools (graph queries, DuckDB queries, DataView reads, etc.). See `mcp-server/README.md` and `docs/smartstudio-mcp-user-guide.md`.

When answering a question using graphstudio MCP tools, review the tool trace before composing the reply. File `mcp__graphstudio__submit_feedback` if: a graph tool couldn't answer something it should have been able to; a graph tool returned 404/400; multiple tool calls assembled what should have been one answer; or a partial/estimated answer was returned. Include `example_question` on every feedback entry. File before responding.

## Further Reading

- **[how-to-use.md](how-to-use.md)** — first-time setup, blank-state walkthrough, tenant switching, troubleshooting
- **[README.md](README.md)** — product overview, capabilities, data flow, quick start
- **[docs/primer.md](docs/primer.md)** — deep-dive on every concept (Sources, Pipelines, DataViews, CDC, Graph, code generation)
