# GraphStudio

**Turn raw data into production-grade operational intelligence — without writing a line of app code.**

GraphStudio is a metadata-driven platform that lets data engineers and app builders connect data sources, define pipelines, model knowledge graphs, and generate fully functional Rust gRPC services and React frontends — all from a single web UI. One platform, one configuration file, one tenant. Deploy it, describe your data, and get a running app.

---

## Why GraphStudio?

Every operational app needs the same things: a way to model data, a way to transform it, and a way to configure business rules. GraphStudio gives you all three as configuration.

- **Define once, generate always.** Describe your data model in the UI. GraphStudio generates the backend services and frontend components that implement it.
- **Live or batch — your choice.** Query Postgres or BigQuery live on every read, materialise into DuckDB via pipelines, or stream WAL changes continuously. The data layer adapts to the source.
- **Graph-native.** Every hierarchy — product (L0 → L5), store (region → DC → store), channel — is a first-class graph node. Roll-ups, cross-filters, and exception rules operate on the graph, not hardcoded SQL.
- **AI-ready.** Built-in agent UI and MCP server let Claude Code read your data, traverse the graph, and answer inventory questions directly.

---

## What You Can Build

| Capability | What it does |
|---|---|
| **Operational dashboards** | Live data for any domain: inventory, orders, fulfilment, ratings — materialised or live-queried, by any hierarchy combination |
| **Configuration surfaces** | Rule-driven management screens backed by your own data, with dimension filters, cascading drill-downs, and editable targets |
| **Planning workspaces** | Multi-level merchandise plans surfaced through the same DataView → Component contract |
| **Cross-filter analysis** | Filter one view by another across shared hierarchy dimensions — instantly, in-process |
| **Exception management** | Flag, review, and resolve inventory exceptions at any graph level |
| **AI query interface** | Natural-language queries over your DuckDB warehouse via the built-in agent and Claude Code MCP tools |

---

## Example — Bolt Basket (quick-commerce grocery delivery)

Bolt Basket runs GraphStudio for their _Dark Store Inventory_ module with four sub-modules:
**Store inventory positions** (SKU-level on-hand per dark store),
**Distribution** (inbound replenishment orders),
**Exception handling** (stockouts, low-stock, freshness alerts), and
**Customer ratings** (per-store NPS and complaint tracking).
Bolt Basket replaced all InventorySmart-specific content with their own TOML templates
following the same Source → Pipeline → DataView → Module pattern.

---

## Core Concepts

### Tenant

Each running GraphStudio instance is one **tenant** — one `(client, app_type, environment)` triple (e.g. `boltbasket-darkstoredash-demo`). Identity is read from `environment.toml` at startup. All metadata and data for a tenant is fully isolated.

### Connection

A **Connection** is your credentials to an external data system — Postgres, BigQuery. Define it once, reuse it across many Sources and Pipelines.

### Source

A **Source** is the addressable data layer that your app binds to. Six kinds cover every pattern:

- **`pg_query` / `bq_query`** — live SQL executed on every read. Always fresh.
- **`duckdb_query`** — live SQL against the local DuckDB warehouse. Fast joins on materialised data.
- **`duckdb_table`** — a pre-populated table produced by a Pipeline. Millisecond reads.
- **`parquet_glob`** — read parquet files directly. Good for lake-style ingestion.
- **`cdc_pg`** — streaming Postgres WAL → DuckDB mirror. Changes land in seconds, no polling.

### Pipeline

A **Pipeline** is a batch multi-step DAG that moves and transforms data. A typical pipeline:

```
PG extract → parquet → load into DuckDB → transform/join → output table
```

Pipelines run on demand or on schedule. Every step is logged. A failed step preserves upstream outputs for debugging.

### Knowledge Graph

The **Graph** is how GraphStudio models your business hierarchy. Nodes are entities (products, stores, DCs, channels). Edges are relationships. Metrics roll up through the hierarchy automatically.

The graph powers:
- **Roll-up** — sum/max/min any metric from leaf to root
- **Cross-filter** — filter a DataView by selecting nodes in a related dimension graph
- **Exception rules** — flag anomalies at any node, inherit and override at any level
- **RCL (Rule Configuration Layer)** — pricing and allocation rules resolved against the live graph, streamed to generated services via gRPC

### DataView

A **DataView** is the contract between data and UI. It binds a Source, adds column metadata (display names, types, searchable/sortable/editable flags), attaches Dimension filters, and defines a default sort. DataViews are stable — swap the backing Source without touching any downstream app code.

### App Surface (Module → SubModule → Component)

Three-level hierarchy that maps to generated-app navigation:

```
Module         →  top-level nav section  (Dashboard, Configuration, VPA)
  SubModule    →  tab within a Module    (Inventory, Allocation, Exceptions)
    Component  →  panel bound to a DataView  (table, chart, editor)
```

Define the hierarchy here; GraphStudio generates the React frontend that implements it.

### Code Generation

Pick a DataView, click **Generate**. GraphStudio renders a Rust gRPC service (filter / sort / paginate / search RPCs) and a React component from language-pack templates. Preview in the UI or write to disk for the deployment workflow.

### AI Agent + MCP

The built-in **Agent UI** (`/agent`) lets you query your data in plain English. The **MCP server** (`mcp-server/`) exposes GraphStudio's data layer to Claude Code: graph traversal, DuckDB queries, DataView reads, and feedback submission — all as MCP tools.

---

## Data Flow

```
External systems                GraphStudio                    Generated app
──────────────────────────────────────────────────────────────────────────────
Postgres ──► pg_query Source ──────────────────────────────────► DataView read
BigQuery ──► bq_query Source ──────────────────────────────────► DataView read

Postgres ──► Pipeline ──► duckdb_table Source ─────────────────► DataView read
  (COPY → parquet → DuckDB load → transforms)

Postgres ──► cdc_pg Source ────────────────────────────────────► DataView read
  (WAL stream → DuckDB mirror, live)

DuckDB warehouse ──► duckdb_query Source ──────────────────────► DataView read

                     DataView ──► Component ──► SubModule ──► Module
                                                              ↓
                                                     Generated React UI

                     DataView ──► Generate ──► Rust gRPC service
                                                     ↓
                                          Tonic server (port 50051)

                     Knowledge Graph ──► Cross-filter / Roll-up / RCL
                                                     ↓
                                          In-process gRPC + agent tools
```

---

## Quick Start

**Prerequisites:** Rust toolchain, Node.js 20+, SSH access to the `rust-shared-utils` Bitbucket repo.

```bash
# Clone and install frontend dependencies
git clone <repo-url>
cd GraphStudio
npm install

# Configure your tenant (edit environment.toml)
# Set client, app_type, environment, is_new = true

# Start everything
npm run dev
# → Vite frontend at http://localhost:5173
# → Rust API server at http://localhost:3001
```

For a complete setup walkthrough — including starting from blank state, connecting data, and building your first DataView — see **[how-to-use.md](how-to-use.md)**.

---

## Repository Structure

```
GraphStudio/
├── src/                    # React frontend (Vite + Zustand + TanStack)
│   ├── agent/              # AI agent UI entry point
│   ├── api/                # API client
│   ├── components/         # Workspace components per resource type
│   ├── layouts/            # WorkspaceLayout shell
│   └── stores/             # Zustand stores (one per entity)
├── server/                 # Rust Axum backend
│   ├── src/
│   │   ├── handlers/       # Route handlers (one file per resource)
│   │   ├── graph/          # Knowledge graph engine
│   │   ├── pipeline/       # Pipeline execution engine
│   │   ├── agent/          # LLM agent subsystem
│   │   ├── services/       # Background tasks (RCL, CDC, scheduler)
│   │   └── db/             # SQLite metadata wrapper
│   └── templates/          # Tera code-generation templates
├── mcp-server/             # MCP server (Node.js, Claude Code integration)
├── docs/
│   ├── primer.md           # Deep-dive concept reference
│   └── configuration.md    # Full configuration options
├── environment.toml        # Tenant identity + boot config (required)
├── how-to-use.md           # Setup and usage guide ← start here
└── CLAUDE.md               # Architecture reference for Claude Code
```

---

## Development Commands

```bash
npm run dev                 # Start frontend + Rust server together
npm run dev:client          # Frontend only (Vite, :5173)
npm run dev:server          # Rust server only (single run, :3001)
npm run dev:server:watch    # Rust server with file-watch rebuild
npm run build               # TypeScript check + Vite production bundle
npm run preview             # Serve production build locally (:4173)
npm run lint                # ESLint
npm test                    # Vitest frontend + MCP tests

cd server && cargo test     # Rust unit + integration tests
cd mcp-server && npm test   # MCP canary test
```

> **First Rust build:** DuckDB and Tonic compile from source — expect 5–10 minutes. Incremental rebuilds are fast.

---

## Technical Setup (Vite + React + TypeScript)

This project uses **React 19 + TypeScript + Vite** for the frontend.

Two official React plugins are available:

- [`@vitejs/plugin-react`](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react) — uses [Oxc](https://oxc.rs) for fast transforms
- [`@vitejs/plugin-react-swc`](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react-swc) — uses SWC

### Expanding the ESLint configuration

For production use, enable type-aware lint rules:

```js
export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      tseslint.configs.recommendedTypeChecked,
      tseslint.configs.stylisticTypeChecked,
    ],
    languageOptions: {
      parserOptions: {
        project: ['./tsconfig.node.json', './tsconfig.app.json'],
        tsconfigRootDir: import.meta.dirname,
      },
    },
  },
])
```

You can also add React-specific lint rules with [`eslint-plugin-react-x`](https://github.com/Rel1cx/eslint-react/tree/main/packages/plugins/eslint-plugin-react-x) and [`eslint-plugin-react-dom`](https://github.com/Rel1cx/eslint-react/tree/main/packages/plugins/eslint-plugin-react-dom).

---

## Further Reading

| Document | What's in it |
|---|---|
| **[how-to-use.md](how-to-use.md)** | Step-by-step setup, blank-state walkthrough, environment switching, troubleshooting |
| **[docs/primer.md](docs/primer.md)** | Deep-dive on every concept: Sources, Pipelines, DataViews, CDC, Knowledge Graph, code generation |
| **[docs/configuration.md](docs/configuration.md)** | Full `environment.toml` reference |
| **[CLAUDE.md](CLAUDE.md)** | Architecture reference for Claude Code — modules, design decisions, boot sequence |
| **[mcp-server/README.md](mcp-server/README.md)** | MCP server setup and Claude Code integration |
