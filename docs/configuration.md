# Configuration

SmartStudio is configured by a single `environment.toml` file. There are no
`.env` files. All non-secret configuration — tenant identity, port numbers,
filesystem layout, optional services — lives in this one TOML.

> Connection secrets (PG/BQ/DuckDB hosts, ports, users, passwords) are **not**
> in the TOML. They live in the SQLite metadata DB's `connections` table,
> created via the UI ("Connections" tab) or seeded by an operator. See
> [§ 7. Connections (secrets)](#7-connections-secrets).

---

## 1. File location and discovery

The server searches for `environment.toml` in this order at startup:

1. `<exe>/../../..` (repo root when running from `target/{debug,release}/`)
2. `<exe>/` (alongside the binary)
3. `<cwd>/` (the working directory the server was launched from)

First hit wins. **The file is required** — if none of the candidates exists,
the server logs the paths it tried and exits with status 1.

Code: `server/src/instance_config.rs::discover()`.

---

## 2. Minimum viable config

```toml
home_path   = "/srv"               # absolute path; tenant_root resolves under here
environment = "prod"
client      = "boltbasket"
app_type    = "darkstoredash"
```

That's it. Server picks defaults for everything else (HTTP port 3001, no gRPC
services, paths under `home_path/smartstudio/{tenant_id}/data/`).

---

## 3. Field reference

### Top-level (required)

| Field | Type | Description |
|---|---|---|
| `home_path` | string | **Absolute path** to the directory under which the `smartstudio/{tenant_id}/data/` tree lives by default. Also the search anchor for relative overrides in `[paths]`. Must exist; the server will not create it. |
| `client` | string | Tenant client name (e.g. `boltbasket`). Part of `tenant_id`. |
| `app_type` | string | App family (e.g. `darkstoredash`). Part of `tenant_id`. |
| `environment` | string | Environment label (`dev`, `uat`, `prod`, …). Part of `tenant_id`. |

`tenant_id` is computed as `{client}-{app_type}-{environment}`.

### Top-level (optional)

| Field | Type | Default | Description |
|---|---|---|---|
| `is_new` | bool | `false` | Bootstrap flag for the **first ever** start of a tenant. See [§ 5. Bootstrap mode](#5-bootstrap-mode-is_new). Remove or set to `false` after the first successful boot. |

### `[server]`

| Field | Type | Default | Description |
|---|---|---|---|
| `port` | u16 | `3001` | HTTP listen port (Axum). The Rust server serves both the API and the static SPA on this port. |
| `grpc_port` | u16 | `50051` | gRPC listen port (Tonic). Used **only** when `[rcl].enabled = true`. Hosts four services on the same port: `rcl`, `article_selection`, `article_graph`, `cross_filter`. |

### `[rcl]`

Gates the in-process RCL service. When disabled (the default), all four
gRPC services are skipped — only the HTTP API runs.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Set to `true` to start the RCL gRPC service at boot. The service subscribes to PG `NOTIFY rcl_changed` (or polls when LISTEN is unavailable) and exposes resolve + Subscribe RPCs. |
| `port_override` | u16 | unset | Optional. Rewrites the resolved default-PG DSN's `port=…` value before handing it to the `RuleStore`. Use when RCL tables (and the trigger migration) live on a separate PG instance — typically a read-replica/MV server on the same host. |

### `[pipeline]`

| Field | Type | Default | Description |
|---|---|---|---|
| `progress_interval_ms` | u64 | `2000` | Cadence (ms) for intra-step progress events. `0` or absent disables intra-step events — only phase boundaries fire. The Live throughput / Show ETA toggles in the pipeline runner UI also feed off this; setting `0` here forces "quiet" mode regardless of the per-run toggles. |

### `[paths]` — optional overrides

Every key is optional. Anything you omit falls back to the home-relative
default (`home_path/smartstudio/{tenant_id}/data/...`).

| Field | Type | Default | Description |
|---|---|---|---|
| `data_dir` | string | `<home>/smartstudio/{tenant_id}/data` | Override the tenant data directory. Most one-line override — every default below cascades. |
| `db_path` | string | `<data_dir>/smartstudio.db` | SQLite metadata DB (apps, dataviews, sources, connections, pipelines, dimensions, filter_configs, …). |
| `duckdb_path` | string | `<data_dir>/tenant_data.duckdb` | Materialized data DuckDB (extract outputs, derived tables, V7 article_selection, …). |
| `log_db_path` | string | `<data_dir>/log.duckdb` | Activity / pipeline trace log DuckDB. |
| `parquet_home` | string | `<data_dir>/parquet` | Parquet root for source bindings (`{PARQUET_HOME}` placeholder in dataview SQL). |
| `traces_dir` | string | `<data_dir>/traces` | Trace artifact directory. |

#### Path resolution rules

| Override value | Behavior |
|---|---|
| Field omitted or empty string | Use the home-relative default. |
| **Absolute** path | Used verbatim. |
| **Relative** path | Joined with `home_path`. |

The "relative joined with `home_path`" rule means a single `home_path` swap
between dev/uat/prod retargets all relative-overridden paths in one move.

---

## 4. Layout (default; with no `[paths]` overrides)

```
{home_path}/                                 ← from environment.toml
└── smartstudio/                             ← namespace dir
    └── {client}-{app_type}-{environment}/   ← tenant_root
        └── data/                            ← data_dir
            ├── smartstudio.db               ← SQLite metadata
            ├── tenant_data.duckdb           ← DuckDB materialized data
            ├── log.duckdb                   ← Activity / trace log
            ├── parquet/                     ← Parquet at rest
            └── traces/                      ← Trace artifacts
```

`dist/` (frontend bundle) lives outside this tree. It's resolved at startup
in this order: `DIST_DIR` env var → `<exe>/../../../dist` →
`<exe>/dist` → `<cwd>/dist`. Served by Axum's fallback as a static directory
with SPA fallback to `index.html`.

---

## 5. Bootstrap mode (`is_new`)

A fresh tenant directory has to be created somehow. To avoid silent
auto-creation that can mask config errors, the server requires an explicit
opt-in on the first boot.

| `db_path` exists | `is_new` | Result |
|---|---|---|
| ✗ | `false` (default) | **exit(1)** with "set `is_new = true` to bootstrap" |
| ✗ | `true` | Create namespace + tenant + parquet + traces dirs, init SQLite schema, log a "remove `is_new` before next start" warning, continue boot |
| ✓ | `false` | Boot normally |
| ✓ | `true` | **exit(1)** with "tenant already exists; remove `is_new` (or delete the tenant folder if you really intended a fresh bootstrap)" |

Rule: set `is_new = true` exactly once, on the first boot of a new tenant.
Remove (or set to `false`) before the next start.

---

## 6. Examples

### 6.1 Single-host development

```toml
home_path   = "/path/to/your/data"
environment = "dev"
client      = "boltbasket"
app_type    = "darkstoredash"

[server]
port      = 3002
grpc_port = 50051

[rcl]
enabled       = true
port_override = 5433
```

Resolves to `/path/to/your/data/smartstudio/boltbasket-darkstoredash-dev/data/`,
HTTP on 3002, gRPC on 50051, RCL service ON pointing at PG on port 5433.

### 6.2 Production: tenant data on a dedicated SSD

Move the entire tenant data dir to a fast local volume, leave everything
else (the `smartstudio/` namespace anchor) at the default:

```toml
home_path   = "/srv"
environment = "prod"
client      = "boltbasket"
app_type    = "darkstoredash"

[server]
port = 3001

[rcl]
enabled = true

[paths]
data_dir = "/local-ssd/boltbasket-darkstoredash-prod"
```

`db_path`, `duckdb_path`, `log_db_path`, `parquet_home`, `traces_dir` all
cascade under `/local-ssd/boltbasket-darkstoredash-prod/`.

### 6.3 Production: parquet on a shared lake, DuckDB on local SSD, metadata on root volume

```toml
home_path   = "/srv"
environment = "prod"
client      = "boltbasket"
app_type    = "darkstoredash"

[paths]
parquet_home = "/mnt/datalake/boltbasket/parquet"
duckdb_path  = "/local-ssd/boltbasket/tenant_data.duckdb"
# db_path, log_db_path, traces_dir are unset → fall back to defaults under
# /srv/smartstudio/boltbasket-darkstoredash-prod/data/
```

### 6.4 Relative override (per-env switch via `home_path` only)

```toml
home_path   = "/srv-prod"        # change to /srv-uat for UAT, /srv-dev for dev
environment = "prod"
client      = "boltbasket"
app_type    = "darkstoredash"

[paths]
parquet_home = "shared-data/boltbasket/parquet"
# Absolute path becomes /srv-prod/shared-data/boltbasket/parquet
# Swapping home_path retargets it automatically.
```

---

## 7. Connections (secrets)

Database connection strings (PostgreSQL hosts, BigQuery projects, etc.) are
**not** in `environment.toml`. They live in the SQLite metadata DB's
`connections` table:

```sql
SELECT id, type, is_default, json_extract(config, '$.host')
FROM connections;
```

Two ways to populate:

1. **Via the UI** — start the server, open the workspace's "Connections" tab,
   add rows interactively. Mark exactly one PG row as default — the RCL
   service, UAM cold-load, and article_selection materializer all read
   `is_default = 1`.
2. **Via SQL seed** — pre-populate `connections` before first start
   (or after `is_new` bootstrap) by inserting rows directly into
   `smartstudio.db`.

This separation is intentional: the TOML is operator-owned and
checked-into-deployment-config systems; secrets stay out of it.

---

## 8. Environment variables

The server intentionally reads only a tiny set of OS env vars:

| Var | Required | Purpose |
|---|---|---|
| `RUST_LOG` | optional | tracing-subscriber filter. Default: `smartstudio_server=info,tower_http=info`. |
| `DIST_DIR` | optional | Override the static-bundle path. Useful when running the binary from a non-standard location. |
| `HOME` | optional | Used by the `bq` CLI for GCP auth lookups (only matters if pipelines use `bq_export`). |

The server **does not** read `DB_PATH`, `PARQUET_HOME`, `PORT`, or any other
config-style env vars — those derive from `environment.toml`.

For `grpc_call` pipeline steps, two env-var families are also looked up:
`{SERVICE}_GRPC_ADDRESS` and `{SERVICE}_REST_ADDRESS` (where `{SERVICE}`
is the step's service name uppercased and dashes turned into underscores).
Set them only if you use that step type.

---

## 9. Verifying the active config

`GET /api/health` returns the resolved config plus runtime facts:

```json
{
  "status": "ok",
  "config": {
    "tenant_id": "boltbasket-darkstoredash-dev",
    "client": "boltbasket",
    "app_type": "darkstoredash",
    "environment": "dev",
    "db_path": "/Users/.../smartstudio.db",
    "duckdb_path": "/Users/.../tenant_data.duckdb",
    "parquet_home": "/Users/.../parquet",
    "port": "3002"
  },
  "runtime": {
    "DIST_DIR": "",
    "cwd": "/Users/.../smartstudio/server",
    "exe": "/Users/.../target/debug/smartstudio-server"
  }
}
```

`config` is everything that came from `environment.toml` (after override
resolution). `runtime` is process-level facts about the running binary. Use
this endpoint as a deploy-time smoke check.

---

## 10. Code references

- `server/src/instance_config.rs` — TOML schema (`InstanceConfig`,
  `PathOverrides`), discovery (`discover`), validation (`resolve`),
  bootstrap (`ensure_ready`).
- `server/src/main.rs` — startup wiring; calls `instance_config::discover()`
  → `load()` → `resolve()` → `ensure_ready()` and threads the resulting
  paths into `AppState`.
- `server/src/handlers/mod.rs` — `/api/health` reads from `AppState` and
  surfaces the resolved config.

---

## 11. What's still rigid (by design)

These are **not** overridable from the TOML, on purpose:

- **`tenant_id` composition**. It's always `{client}-{app_type}-{environment}`
  because the three components show up independently in logs, gRPC tags,
  and folder names. Override `data_dir` if you want to break the implicit
  folder convention.
- **The `smartstudio/` namespace anchor**. Internal directory namespace; not overridable.
- **Discovery search paths** for `environment.toml`. The three locations
  (exe-relative, exe, cwd) are hard-coded — operators control which file
  the server picks via where they place it, not via flags.

If a future use case requires any of these to be configurable, extending
`InstanceConfig` with another optional field is straightforward and
preserves backward compatibility (every override is `Option<…>` with a
sensible default).
