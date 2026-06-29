# SmartStudio — A Primer

**SmartStudio is a metadata-driven app generation platform.** A single SmartStudio instance hosts one tenant — one client × one app type × one environment (e.g. "bealls × inventorysmart × dev"). The platform captures the metadata that describes a tenant's data feeds, application surface, and contracts, and generates the runtime artifacts (Rust gRPC services, React frontends, supporting code) that implement them.

This document is a top-to-bottom tour of the concepts you'll encounter while using or extending the platform.

---

## 1. The mental model in one picture

```
              ┌─────────────────────────────────────────────┐
              │              Generated App                  │
              │   (React frontend + Rust gRPC services)     │
              └──────────────────────┬──────────────────────┘
                                     │ consumes
                                     ▼
              ┌─────────────────────────────────────────────┐
              │       Application Surface (UI metadata)     │
              │                                             │
              │   Modules → SubModules → Components         │
              │   Component ──▶ DataView                    │
              │   DataView ──▶ Source                       │
              │   Filters ◀── Dimensions, FilterConfigs     │
              │   Live state ◀── ViewPorts                  │
              └──────────────────────┬──────────────────────┘
                                     │ binds to
                                     ▼
              ┌─────────────────────────────────────────────┐
              │                Data Layer                   │
              │                                             │
              │   Connection ──▶ Source                     │
              │   Pipeline ──▶ Source (kind=duckdb_table)   │
              │                                             │
              │   Source kinds: pg_query, bq_query,         │
              │     duckdb_query, parquet_glob,             │
              │     duckdb_table, cdc_pg                    │
              └─────────────────────────────────────────────┘
```

Three layers, with a small number of concepts each. The split is deliberate: data engineering concerns (down) stay separate from app-builder concerns (up). One Source can back many DataViews; one DataView is consumed by many Components.

---

## 2. Tenant & deployment

SmartStudio ships as a single binary plus a `dist/` frontend. **Each running instance is one tenant.** Identity is read from a required `environment.toml` at startup:

```toml
home_path   = "/home/karthick"
client      = "bealls"
app_type    = "inventorysmart"
environment = "dev"
[server]
port      = 3001
grpc_port = 50051
[rcl]
enabled       = true
port_override = 5433
```

The tenant id is `{client}-{app_type}-{environment}` (e.g., `bealls-inventorysmart-dev`). The instance's data lives at `<home_path>/smartstudio/<tenant_id>/data/`:

| File / Dir | Role |
|---|---|
| `smartstudio.db` | SQLite holding all tenant metadata (connections, sources, pipelines, dataviews, modules, …) |
| `tenant_data.duckdb` | DuckDB warehouse for materialized data |
| `parquet/` | Parquet artifacts (some pipeline outputs) |
| `traces/log.duckdb` | Activity / audit log |

There is no cross-tenant data sharing inside one binary. Multi-tenant deployments run multiple binaries.

---

## 3. Data layer

### 3.1 Connection

A **Connection** is credentials/endpoint to an external data system. Reusable across many Sources and Pipelines.

Stored in the `connections` SQLite table. Fields: `id, display_name, type (pg | bq | …), is_default, config JSON`. The config holds host, port, user, password, database, etc.

Marking a Connection as **default** lets Pipelines and Sources pick "the obvious choice" when they don't specify a `connection_ref`. At most one default per type.

### 3.2 Source

**A Source is the addressable data layer for DataViews.** Every Source is an explicit row in the `sources` table — there is no implicit/auto-discovery; if it's not a row, it's not a Source. A Source row carries a `kind` discriminator and kind-specific config.

| Kind | Read mode | What it is |
|---|---|---|
| `pg_query` | live | SQL against PG, executed on each DataView read |
| `bq_query` | live | SQL against BigQuery, executed on read |
| `duckdb_query` | live | SQL against tenant DuckDB, executed on read |
| `parquet_glob` | static | Read parquet files at a path |
| `duckdb_table` | static | Read an existing DuckDB table; **populated by a Pipeline** |
| `cdc_pg` | streaming | Self-managing PG WAL → DuckDB mirror (see §3.4) |

DataViews bind to a Source by id: `source = { type:'source', config:{ source_id, output? } }`. The read path looks up the Source and dispatches by kind.

**Source deletion is blocked when DataViews are bound.** Users must rewire or delete the bound DataViews first — symmetric across all kinds.

#### 3.2.1 Per-kind detail

**`pg_query`** — live Postgres SQL.
- Config: `connection_ref` (which Connection), `sql` (the SELECT to execute).
- DataView reads connect to PG, execute the SQL, return rows. No caching.
- Best for: low-volume, always-fresh data; admin lookups; small dimension tables.
- Cost per read = PG roundtrip + query time.

**`bq_query`** — live BigQuery SQL.
- Config: `connection_ref`, `sql`.
- Federated query against BQ. Same shape as `pg_query` but BQ-backed.
- Best for: data warehoused in BQ that doesn't need to live in DuckDB.

**`duckdb_query`** — live DuckDB SQL.
- Config: `sql` (typically referencing other Sources / tables in `tenant_data.duckdb`).
- Best for: composing multiple DuckDB tables on read (joins, aggregates) without materializing a new table.
- Cost per read = DuckDB query time. Generally fast for small joins.

**`parquet_glob`** — read parquet files at a path.
- Config: `path` (filesystem path; may use `{PARQUET_HOME}` placeholder), `hive_partitioning` (bool).
- Best for: external parquet artifacts (GCS-mirrored data, exports from other systems).
- Reads via DuckDB's `read_parquet()` with optional hive partitioning.

**`duckdb_table`** — existing DuckDB table.
- Config: `table_name`.
- Created **before** any pipeline populates it. The Source row is a deliberate placeholder. Pipelines target the Source by id; the underlying table comes into existence the first time a pipeline runs against it.
- Source row carries `last_populated_at`, `producing_pipeline_ids[]` (audit lineage).

**`cdc_pg`** — streaming PG mirror.
- Config: `connection_ref`, `upstream_table` (e.g., `inventory_smart.orders`), `target_table` (DuckDB table name), `primary_key`, `cdc_enabled` flag.
- On creation: smartstudio does an initial `COPY` from PG into the target table, then opens a logical-replication stream to apply WAL events live.
- Stays running as a long-lived task. Restartable. Auto-resumes on server boot.
- Full details in §3.4.

#### 3.2.2 Read-time dispatch

When a DataView is read (`POST /api/dataviews/{id}/data`), smartstudio:
1. Loads the DataView row, reads its `source.config.source_id`.
2. Loads the Source by id; switches on `kind`.
3. Dispatches to the kind-specific executor:
   - `pg_query` / `bq_query` → tokio_postgres / BigQuery client + the referenced Connection.
   - `duckdb_query` / `duckdb_table` / `parquet_glob` / `cdc_pg` → DuckDB connection.
4. Wraps the kind-specific query with the DataView's filters, sort, and pagination clauses, then returns rows + columns + total to the frontend.

#### 3.2.3 Edit semantics

- A Source's **kind cannot be changed** after creation. To switch kinds, create a new Source and re-bind DataViews.
- A Source's **config can be edited** (SQL, table_name, etc.); changes take effect on the next read.
- For `cdc_pg`, editing `upstream_table` or `primary_key` requires the CDC stream to be stopped first; the UI enforces this.
- Renaming a `target_table` on a `cdc_pg` Source is destructive (drops the old DuckDB table, restarts CDC against a new target); confirm prompt required.

### 3.3 Pipeline

**A Pipeline is a batch multi-step DAG that produces data.** Pipelines are independent — peer to Sources, not a kind of Source.

A Pipeline declares an ordered list of **steps**. The catalog of step kinds lives in the `pipeline` crate (`rust-shared-utils/pipeline`); each step has its own config schema, and the executor dispatches based on the variant.

#### 3.3.1 Step kinds

| Step kind | Reads from | Writes to | What it does |
|---|---|---|---|
| `pg_extract` | A `connection_ref` PG | Parquet on disk | Runs `COPY (query) TO STDOUT FORMAT CSV`, converts to parquet via DuckDB. Hash-partitioned parallel COPY for simple table scans. |
| `duckdb_load` | Parquet | DuckDB table | Loads a parquet glob into a named DuckDB table (typically the output of an upstream `pg_extract`). |
| `duckdb_query` | DuckDB | DuckDB | Executes arbitrary SQL on DuckDB. Used for joins, aggregates, transforms, `CREATE TABLE AS`, indexes, etc. |
| `duckdb_table` | DuckDB table | (downstream) | Declares the step's output table. Participates in dependency tracking and is the target reference for pipeline `duckdb_table` Source bindings. |

A typical Pipeline is a sequence:
`pg_extract` (PG → parquet) → `duckdb_load` (parquet → DuckDB) → one or more `duckdb_query` (transforms / joins) → final `duckdb_table` declaration that participates as the targeted Source's contents.

#### 3.3.2 Execution model

- Pipelines run **manually** (button click, `POST /api/pipelines/{id}/run`, or scoped re-run for changed inputs).
- The executor processes steps in declared order. Steps without mutual dependencies can run in parallel — the `pipeline` crate's executor coordinates a `JoinSet` for concurrent step branches when the DAG permits.
- `pg_extract` itself uses parallel hash-partitioned COPY internally for table scans (independent of cross-step parallelism).
- Each step run is logged to the activity log with start time, end time, duration, error (if any), and rows-affected metrics.

#### 3.3.3 Failure handling

- A step failure stops the pipeline run; subsequent steps are not attempted.
- Partial state is preserved — the parquet files / DuckDB tables produced by successful upstream steps remain on disk.
- The activity log records the failed step's error message + the list of steps that didn't run.
- The user retries by re-clicking Run; smartstudio re-runs the pipeline from the start. Resume-from-failed-step is future work.

#### 3.3.4 Targeting Sources

A pipeline step that produces data **declares its target as an existing `duckdb_table` Source by id.** The Source row exists first (a placeholder for the eventual table). The pipeline step references it. Running the pipeline writes the table.

Multiple pipelines may target the same Source — for example, one daily-refresh pipeline plus one on-demand reseed pipeline. Last writer wins. The Source row records the producing pipelines as a many-to-many relationship for lineage navigation.

#### 3.3.5 Source state during pipeline runs

A `duckdb_table` Source has lifecycle states surfaced in the UI:

| State | Meaning |
|---|---|
| `not_yet_populated` | Source row created, no pipeline has produced the underlying table yet. |
| `populating` | A pipeline run targeting this Source is in progress. |
| `populated` | Table exists and has rows. Last pipeline run completed successfully. |
| `failed` | Last pipeline run targeting this Source ended in error; the table may be partial or unchanged from the previous successful run. |

DataViews bound to a `not_yet_populated` Source render the friendly empty state with hints to run the populating pipelines (§4.1 / DataView read-time dispatch).

#### 3.3.6 Lineage

The Source row maintains a small history of producing-pipeline runs: which pipeline last populated it, when, with what row count. The Sources tab can show this for any `duckdb_table` Source. Useful when debugging "where did this data come from."

### 3.4 CDC streaming (`cdc_pg` Source)

CDC is **streaming ingestion from PG into DuckDB, kept live via WAL**. It exists as a single Source kind, `cdc_pg`. A `cdc_pg` Source is **self-contained**: it owns its DuckDB mirror table, manages its own long-lived runtime, and handles initial seeding plus continuous WAL streaming.

Key properties:

- **Not a pipeline step.** Lives in its own crate (`cdc` in `rust-shared-utils`, parallel to `pipeline` and `rcl`). Smartstudio launches one streaming task per `cdc_pg` Source.
- **One row per mirror.** The Source row holds `connection_ref`, `upstream_table`, `target_table`, `primary_key`, `cdc_enabled`.
- **Auto-resume on boot.** Every `cdc_pg` Source with `cdc_enabled=true` whose target table exists in DuckDB is restarted automatically. Operator doesn't re-click "Start CDC" after every deploy.
- **Backend scope:** PG only today. BQ/MySQL CDC, if introduced later, would be new kinds (`cdc_mysql`, etc.).

### 3.5 The flow, summarized

```
Connection ──▶ pg_query / bq_query Source       ─┐
              ─▶ duckdb_query / parquet_glob ────┤── DataView read = live execute
                                                  │
Connection ──▶ cdc_pg Source ───────────────────  ── DataView read = read from mirror
              (owns DuckDB target table)

Connection ──▶ Pipeline ──▶ duckdb_table Source ── DataView read = SELECT from table
                          (Pipeline populates;
                           Source persists)
```

---

## 4. Application surface

### 4.1 DataView

A **DataView is the rectangle the app shows.** It picks a Source as backing data and adds presentation:

- **`columns`** — projection metadata (visible/hidden, display names, types, sortable / searchable / editable flags, group hints).
- **`sort`** — default sort.
- **`contract`** — gRPC service binding (which engine, cache strategy, supported operations).
- **`dimensions`** — Dimensions that filter this view.
- **`cascading_filters`** — filter cascade rules.
- **`source`** — `{ type:'source', config:{ source_id, output? } }`.

DataViews are the **app's contract surface**. Modules, SubModules, and Components reference DataViews. The DataView definition is stable; its backing Source can change without touching app code generation.

### 4.2 Module → SubModule → Component

Three-level UI hierarchy mirroring the generated app's navigation:

- **Module** — a top-level section in the left nav (e.g., "Dashboard", "Configuration", "VPA"). Has route, icon, permission key, sort order.
- **SubModule** — a tab within a Module. References one or more DataViews; designates a primary one.
- **Component** — a panel within a SubModule. Bound to a DataView; carries component-specific config (chart type, table layout, etc.).

A typical generated app has many Modules, each containing several SubModules with multiple Components.

### 4.3 Dimension

A **Dimension is a filter-dimension definition.** Examples: product (with levels `l0_name → l5_name`, brand), store (region → district → store), DC.

Each Dimension stores:
- `master_table` — the underlying lookup table (e.g., `product_attributes_filter`).
- `levels` — ordered attribute columns.
- `additional_filter_cols` — extra columns usable for filtering.

A DataView lists the Dimensions it supports; the generated UI renders dimension-specific filter panels.

### 4.4 FilterConfig

A **FilterConfig is a reusable, named filter configuration for a Dimension.** Independent of DataViews — multiple DataViews can pick the same FilterConfig.

Stores: which columns are filterable, mandatory columns, cascading rules, optional row-count thresholds. Lets data engineers and app builders curate "good filter UX for this dimension" once and share it across views.

### 4.5 ViewPort

A **ViewPort is a stateful, server-side filtered window into a DataView.** When a user selects dimension filters in the generated app, the app calls `CreateViewPort(dataview_id, filters)`. The server reads the DataView's Source, applies the filters, materializes the result, and caches it on a remote-cache keyed by `(user, dataview, filters_hash)`. Subsequent sort/search/paginate operations target the cached materialized result — no recomputation.

A filter change creates a new ViewPort (or, when the underlying Source is `cdc_pg`, applies a delta refresh).

> **Note:** ViewPort, Filter, and DataView refinement are deferred to a separate planning session — only the binding shape changes in the current Source-unification effort.

---

## 5. Code generation

SmartStudio generates **Rust gRPC services + React frontends + supporting artifacts** from the metadata described above. Generation is template-driven; templates live in **language packs**.

A **Language Pack** is a directory of [Eta](https://eta.js.org/) templates targeting a specific stack. Today: `rust-backend` (Rust + Tonic), `react-frontend` (React 19 + TanStack Table / Router). `golang-backend` is planned.

Generation flow:
1. Pick a language pack and a target dataview / module.
2. Smartstudio renders templates with the metadata as input.
3. Output goes to a preview (UI) or written to disk (Bitbucket workflow).

Generated services include the per-DataView server (filter / sort / paginate / search RPCs) plus shared services:

- **RCL Resolution** — rule-configuration resolver for inventory pricing/allocation rules. Lives in the `rcl` crate (rust-shared-utils). Three unary RPCs (`ResolveDcPolicy`, `ResolveConstraints`, `ResolvePsm`) plus a server-streaming `Subscribe` that pushes fresh rule corpora when the underlying PG tables change. PG `LISTEN`/`NOTIFY` triggers feed the change detection.
- **Cross-Filter** — filter-cascade coordination across DataViews (planned).
- **Article Selection** — bealls-specific in-process Rust materializer for the inventory-smart article selection table. Not a pipeline; a custom runner that uses RCL resolution + parallel PG COPY + rayon assembly to produce one ~43K-row DuckDB table.

---

## 6. Cross-cutting

### 6.1 Activity / traces

SmartStudio logs **all metadata mutations** (create dataview, save source, run pipeline, etc.) and **all pipeline execution events** (step start / end / error) to a per-tenant DuckDB at `<tenant_data_dir>/log.duckdb`. The Activity panel in the UI streams events live via SSE.

### 6.2 In-process gRPC

Smartstudio runs a Tonic gRPC server alongside the Axum HTTP server, on a separate port (default 50051). Currently hosts the RCL service. Other in-process gRPC services (cross-filter, dataview-specific) can be added without spawning new processes.

### 6.3 Query workspace

A free-form SQL editor against the tenant's `tenant_data.duckdb`. Three side tabs:
- **Tables** — `SHOW TABLES` against the warehouse; click a table to load `SELECT * FROM "<name>" LIMIT 100` into the editor.
- **Saved** — named query templates persisted in SQLite.
- **History** — last 50 queries this session (in-memory; cleared on refresh).

Used for ad-hoc inspection — not part of the generated app.

### 6.4 Theme

Light + dark themes selectable from the top-right of the app. The active theme is persisted in `localStorage`. Default is dark.

---

## 7. Glossary

| Term | One-liner |
|---|---|
| **Tenant** | A `(client, app_type, environment)` triple; one per running binary |
| **Connection** | Credentials to an external data system; reusable |
| **Source** | Addressable data definition that DataViews bind to; six kinds |
| **Pipeline** | Batch multi-step DAG that populates `duckdb_table` Sources |
| **CDC Source (`cdc_pg`)** | Streaming PG → DuckDB mirror with its own runtime |
| **DataView** | App-surface contract — columns, sort, dimensions, gRPC binding |
| **Module / SubModule / Component** | 3-level generated-UI hierarchy |
| **Dimension** | Filter-dimension definition (levels + master table) |
| **FilterConfig** | Reusable named filter spec for a Dimension |
| **ViewPort** | Server-side filtered + cached window into a DataView |
| **Language Pack** | Set of Eta templates for one target stack |
| **`tenant_data.duckdb`** | Tenant warehouse — most data lives here |
| **`smartstudio.db`** | Tenant metadata SQLite |
| **`environment.toml`** | Required boot config: tenant id, ports, feature flags |

---

## 8. Out of scope here (deferred to separate documentation)

This primer covers the **platform's concept layer**. The following are intentionally out of scope and deserve their own dedicated documents:

- **Article Selection** — the bealls-specific inventory-smart article selection pipeline (multi-step extract from `mv_asv2_*` materialized views + RCL resolution + rayon assembly) and its dedicated DataView. It uses smartstudio's primitives (Connection, RCL service, DuckDB warehouse) but encodes tenant-specific business logic that doesn't belong in a platform primer. Future doc: `docs/article-selection.md`.
- **DataView + Filter + ViewPort refinement** — the next planning session covers refinements to dimension filtering, viewport semantics, and DataView contract. The current primer reflects today's shape; the refinement may shift this.

---

## See also

- [Source Unification Plan](./plans/source-unification.md) — strangler-style migration plan that brings today's three concepts (`data_sources`, `query_sources`, `shared_pipelines`) into the unified Source + Pipeline model described in this primer.
