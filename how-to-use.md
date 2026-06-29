# GraphStudio — How to Use

A step-by-step guide to running GraphStudio, starting from a blank state, and building up to a fully connected app.

---

## Table of Contents

1. [What is GraphStudio?](#1-what-is-graphstudio)
2. [Understanding your tenant identity](#2-understanding-your-tenant-identity)
3. [Starting fresh (blank state)](#3-starting-fresh-blank-state)
4. [Running the app](#4-running-the-app)
5. [Connecting your data — step by step](#5-connecting-your-data--step-by-step)
   - 5.1 [Add a Connection](#51-add-a-connection)
   - 5.2 [Create a Source](#52-create-a-source)
   - 5.3 [Create a Pipeline (optional)](#53-create-a-pipeline-optional)
   - 5.4 [Create a DataView](#54-create-a-dataview)
   - 5.5 [Build the app surface (Modules → Components)](#55-build-the-app-surface-modules--components)
6. [Switching between tenants](#6-switching-between-tenants)
7. [Where your data lives](#7-where-your-data-lives)
8. [Running tests](#8-running-tests)
9. [Building for production](#9-building-for-production)
10. [Configuration reference (`environment.toml`)](#10-configuration-reference-environmenttoml)
11. [Troubleshooting](#11-troubleshooting)

---

## 1. What is GraphStudio?

GraphStudio is a **metadata-driven platform** that lets you define data connections, sources, pipelines, and application surfaces in a web UI — and then generates the production Rust gRPC services and React frontends that implement them.

Each running instance is **one tenant** — one `(client, app_type, environment)` triple. All configuration lives in `environment.toml`.

---

## 2. Understanding your tenant identity

GraphStudio derives a tenant ID from three fields in `environment.toml`:

```toml
client      = "briscoes"
app_type    = "inventorysmart"
environment = "demo"
# → tenant ID: briscoes-inventorysmart-demo
```

This ID determines:
- The **data directory**: `<home_path>/smartstudio/<tenant-id>/data/`
- The **display name** shown in the UI header
- The **SQLite metadata DB**: `smartstudio.db` inside that directory

The currently running tenant (`briscoes-inventorysmart-demo`) already has data. To start fresh, you create a new identity — existing data is never touched.

---

## 3. Starting fresh (blank state)

### Step 1 — Edit `environment.toml`

Open `environment.toml` in the repo root and change the three identity fields to whatever your new tenant should be:

```toml
home_path   = "/Users/yourname/Documents/graphstudio-data"
client      = "mycompany"
app_type    = "inventorysmart"
environment = "dev"
is_new      = true    # ← tells the server to bootstrap a fresh DB on first boot
```

Also comment out `default_id` under `[graphs]` (it references the old tenant's graph):

```toml
[graphs]
# default_id = "briscoes-inventory-graph"
```

And disable RCL if you don't have a live PG replication source:

```toml
[rcl]
enabled = false
```

### Step 2 — Start the server

```bash
npm run dev
```

On first boot with `is_new = true`, the server:
- Creates the directory `<home_path>/smartstudio/mycompany-inventorysmart-dev/data/`
- Bootstraps an empty `smartstudio.db` (all tables present, zero rows)
- Seeds any starter templates from `data/`

### Step 3 — Flip `is_new` back to `false`

After the first successful boot, set:

```toml
is_new = false
```

This prevents the bootstrap from running again on future restarts. The server will not touch existing data.

---

## 4. Running the app

All commands run from the repo root (where `package.json` lives).

| Command | What it does |
|---|---|
| `npm run dev` | Start Vite (`:5173`) + Rust server (`:3001`) together |
| `npm run dev:client` | Frontend only (Vite) |
| `npm run dev:server` | Rust server only (single run) |
| `npm run dev:server:watch` | Rust server with file-watch rebuild |
| `npm run build` | TypeScript check + Vite production bundle |
| `npm run preview` | Serve the production `dist/` build locally |

> **First build warning:** The Rust server takes several minutes to compile on first run (DuckDB + Tonic compilation). Subsequent builds are incremental and much faster.

The UI is at **http://localhost:5173** in dev mode, or **http://localhost:4173** when running `npm run preview`.

There are two HTML entry points:
- `index.html` → main GraphStudio editor UI
- `agent.html` → AI agent UI (`/agent`)

---

## 5. Connecting your data — step by step

### 5.1 Add a Connection

A **Connection** is your credentials to an external data system (PostgreSQL, BigQuery). You reuse it across many Sources.

1. Open the **Connections** tab in the left nav.
2. Click **New Connection**.
3. Fill in: type (`pg` or `bq`), host, port, database, username, password.
4. Optionally mark as **Default** — pipelines and sources without an explicit connection reference will use this one.
5. Save.

> Passwords are masked in GET responses (`••••••••`) and preserved on save if you don't change them.

### 5.2 Create a Source

A **Source** is the addressable data layer that DataViews bind to. Choose a kind based on where your data lives and how you want to read it:

| Kind | When to use |
|---|---|
| `pg_query` | Live SQL against Postgres — always fresh, executed on each read |
| `bq_query` | Live SQL against BigQuery |
| `duckdb_query` | Live SQL against the local DuckDB warehouse |
| `parquet_glob` | Read parquet files at a path |
| `duckdb_table` | A pre-populated DuckDB table produced by a Pipeline |
| `cdc_pg` | Streaming Postgres WAL mirror into DuckDB |

**To create a Source:**
1. Open the **Sources** tab.
2. Click **New Source**, pick a kind.
3. Fill in the kind-specific config:
   - `pg_query` / `bq_query`: select a Connection, write your SQL SELECT.
   - `duckdb_query`: write your SQL (no connection needed).
   - `duckdb_table`: enter the target DuckDB table name (the pipeline will create it).
   - `parquet_glob`: enter the file path (use `{PARQUET_HOME}` as a placeholder).
   - `cdc_pg`: select Connection, upstream table, primary key, target table name.
4. Save.

> A Source's **kind cannot be changed** after creation. To switch kinds, create a new Source and re-bind your DataViews to it.

### 5.3 Create a Pipeline (optional)

Pipelines are needed when you want to **materialise data into DuckDB** (kind `duckdb_table`). If you're using live Sources (`pg_query`, `bq_query`, `duckdb_query`) you can skip this step.

A Pipeline is a multi-step batch DAG. Typical pattern:

```
pg_extract  →  duckdb_load  →  duckdb_query (transforms)  →  duckdb_table
(PG → parquet)  (parquet → DuckDB)  (joins, aggregates)     (output Source)
```

**To create a Pipeline:**
1. Open the **Pipelines** tab.
2. Click **New Pipeline**, give it a name.
3. Add steps in order; each step references a Connection (for PG/BQ steps) and declares inputs/outputs.
4. The final step must reference a `duckdb_table` Source by id — this is how the pipeline knows where to write.
5. Save, then click **Run** to execute.

Pipeline runs are logged to the Activity panel in real time. A failed step stops the run; successful upstream outputs are preserved on disk for debugging.

### 5.4 Create a DataView

A **DataView** is the contract between your data and your app's UI. It adds presentation metadata on top of a Source.

1. Open the **DataViews** tab.
2. Click **New DataView**.
3. Select a **Source** as the backing data.
4. Configure **columns**: which columns are visible, their display names, types, and flags (sortable, searchable, editable).
5. Set a **default sort**.
6. Optionally attach **Dimensions** (for filter panels) and **FilterConfigs**.
7. Save.

DataViews are stable contracts. If you later swap the backing Source, your app-surface bindings (Modules, Components) don't change.

### 5.5 Build the app surface (Modules → Components)

The three-level hierarchy maps to generated-app navigation:

```
Module (top-level nav section)
  └── SubModule (tab within a Module)
        └── Component (panel, bound to a DataView)
```

**To build the surface:**
1. Open the **Modules** tab.
2. Create a **Module** — give it a name, route, icon, and sort order.
3. Inside the Module, add a **SubModule** — assign it to one or more DataViews, designate a primary one.
4. Inside the SubModule, add **Components** — bind each to a DataView, choose a component type (table, chart, etc.).

Once the surface is defined, use **Code Generation** (DataViews tab → Generate) to preview or write the Rust gRPC service and React frontend for a DataView.

---

## 6. Switching between tenants

GraphStudio has no multi-tenant UI — switching tenants means changing `environment.toml` and restarting.

**To switch to a different tenant:**

```toml
# environment.toml
client      = "acme"
app_type    = "inventorysmart"
environment = "staging"
is_new      = false   # false if the tenant already exists
```

Restart the server (`npm run dev:server`). Data for each tenant is fully isolated in its own subdirectory under `<home_path>/smartstudio/`.

**To create a new tenant while keeping the old one intact:** just change the three identity fields and set `is_new = true`. The previous tenant's directory is untouched.

---

## 7. Where your data lives

```
<home_path>/smartstudio/<client>-<app_type>-<environment>/data/
├── smartstudio.db          # All metadata: connections, sources, dataviews, pipelines, ...
├── smartstudio.db-shm      # SQLite shared memory
├── smartstudio.db-wal      # SQLite write-ahead log
├── tenant_data.duckdb      # Materialized data warehouse (pipeline outputs, CDC mirrors)
├── log.duckdb              # Activity + trace log
├── parquet/                # Pipeline-produced parquet files
├── sources/                # Seed source definitions (TOML)
├── dataviews/              # Seed dataview definitions (TOML)
├── duckdb_views/           # SQL files seeded as DuckDB views on boot
├── graphs/                 # Graph spec files
└── agent.db                # AI agent workspace + session SQLite
```

**To wipe a tenant's data completely** (destructive — no undo):

```bash
rm -rf "<home_path>/smartstudio/<tenant-id>"
```

Then set `is_new = true` in `environment.toml` and restart the server to recreate an empty DB.

---

## 8. Running tests

```bash
# Frontend + MCP tests
npm test

# Rust integration + unit tests
cd server && cargo test

# MCP server only
cd mcp-server && npm test
```

Expected results (post-rename):
- **Rust**: 98 pass / 2 fail (expected RED canaries) / 1 ignored
- **Frontend**: 3 pass / 2 fail (expected RED canaries)
- **MCP**: 1 pass

The 4 failing tests are intentional canaries that guard the rename — they assert old "SmartStudio" strings that no longer exist in the renamed codebase.

---

## 9. Building for production

```bash
npm run build
```

This runs:
1. `tsc -b` — TypeScript type check across app + node configs
2. `vite build` — bundles `index.html` (main UI) and `agent.html` (agent UI) into `dist/`

The Rust server (`server/`) is compiled separately by Cargo:

```bash
cd server && cargo build --release
```

In production, the Rust server serves the built `dist/` as static files in addition to the API routes.

---

## 10. Configuration reference (`environment.toml`)

```toml
# Required — tenant identity
home_path   = "/path/to/data/root"
client      = "mycompany"
app_type    = "inventorysmart"
environment = "dev"
is_new      = false   # set true only on first boot of a new tenant

[server]
port      = 3001    # HTTP API + static file server
grpc_port = 50051   # Tonic gRPC (RCL service)

[rcl]
enabled       = false          # set true if you have a live RCL PG source
port_override = 5432           # PG port for RCL LISTEN/NOTIFY

[graphs]
# Must match the `id` of a row in the `graphs` SQLite table.
# Comment out if starting fresh — no graph exists yet.
# default_id = "mycompany-inventory-graph"

[paths]
# Optional: override artifact locations per-type.
# Omitting any key falls back to <home_path>/smartstudio/<tenant-id>/data/...
# parquet_home = "/mnt/fast-disk/parquet"

[agent]
# GCP Secret Manager for LLM API keys (omit for local dev).
# For local dev, export OPENAI_API_KEY / ANTHROPIC_API_KEY in your shell.
# gcp_project_id     = "my-gcp-project"
# llm_secret_name    = "graphstudio_agent"
# llm_secret_version = 1
```

**Items that must NOT be renamed** (internal identifiers, not display names):
- `NAMESPACE_DIR = "smartstudio"` — data directory path segment
- `smartstudio.db` — SQLite filename
- `SMARTSTUDIO_PG_MAX_CONCURRENCY` — env var for PG pool tuning
- `SMARTSTUDIO_BEALLS_DUCKDB` — env var for bealls DuckDB path override
- `SMARTSTUDIO_URL` — env var read by the MCP server to locate the API

---

## 11. Troubleshooting

**Server won't start — "database is locked"**
Another process (or a crashed server) holds the SQLite WAL lock. Find and kill it:
```bash
lsof | grep smartstudio.db
```

**Blank UI / no data after switching tenants**
Check `environment.toml` — make sure `client`, `app_type`, `environment` match the tenant whose data you want. The server derives the data path from these three values at boot.

**`is_new = true` but database already exists**
The bootstrap is a no-op if `smartstudio.db` already exists — it won't overwrite existing data. To truly reset, delete the tenant data directory first (§7), then restart with `is_new = true`.

**RCL service fails to start**
Set `[rcl] enabled = false` if you don't have a Postgres instance with the RCL tables and `NOTIFY` triggers. The server starts cleanly without it; only RCL-dependent features (rule resolution) are unavailable.

**Rust compilation fails with SSH errors**
Dependencies pull from Bitbucket via SSH (`ssh://git@bitbucket.org:22/insideinsight/rust-shared-utils.git`). Ensure your SSH key is added to `ssh-agent`:
```bash
ssh-add ~/.ssh/id_rsa
```
Or uncomment the `[patch.]` block in `server/Cargo.toml` to use local overrides.

**Build error: `test does not exist in type UserConfigExport`**
This is a known type conflict between vite v6 (rolldown) and vitest v3's bundled vite (rollup). It is suppressed via `// @ts-nocheck` in `vitest.config.ts` and the test config has been split out of `vite.config.ts`. Run `npm run build` — it should pass cleanly.

---

## See also

- [`CLAUDE.md`](CLAUDE.md) — architecture reference, key design decisions, module overview
- [`docs/primer.md`](docs/primer.md) — deep dive into every concept (Sources, Pipelines, DataViews, CDC, code generation)
- [`docs/configuration.md`](docs/configuration.md) — detailed configuration options
- [`mcp-server/README.md`](mcp-server/README.md) — MCP server setup for Claude Code integration
