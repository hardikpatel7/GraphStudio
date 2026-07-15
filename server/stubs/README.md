# Vendored scaffold stubs

These six crates are **inert, local stand-ins** for the private
`rust-shared-utils` crates the server used to consume over SSH
(`ssh://git@bitbucket.org/insideinsight/rust-shared-utils.git`).

They exist so the **public** GraphStudio repo compiles and boots with **no SSH
access and no proprietary code**. They are NOT the real crates: they reproduce
only the public API *shapes* the server references, with no-op / empty bodies.

| Stub | Replaces | Behavior in this build |
|---|---|---|
| `pipeline` | ETL pipeline engine | `execute` returns an empty result — no extract/load/materialize. Trigger parsing (`PipelineTrigger`) is faithful (ordinary plumbing). |
| `pg` | Postgres pool helper | No pools opened; `initialize_named_pool` is a no-op success. Live PG disabled. |
| `app-config` | Config loader | Only `database::DatabaseConfig` (the type the server builds). No TOML/GCP loading. |
| `secret_manager` | GCP Secret Manager | No backend, no credentials; every fetch errors → callers fall back to shell env vars. |
| `cdc` | PG→DuckDB change capture | Disabled: `start` returns an error (so no source is falsely marked "streaming"); `stop`/`ensure_slot` are no-op. Boot's auto-start logs a warning and continues. |
| `rcl` | Retail Constraint Language rules engine | **Carries none of the resolution algorithm.** All `resolve_*` return empty, `RclRule::matches` is always `false`. RCL resolves to nothing (permissive/empty). |

## What this means functionally

The server **compiles, boots, and serves the UI/API**, but the features backed by
these crates are inert: no pipeline execution, no live Postgres pools, no CDC
streaming, no GCP secrets, and no RCL rule resolution. This is a scaffold for
experimentation — not a production deployment.

## Building against the real crates

Each dependency in `server/Cargo.toml` keeps its original `git = "ssh://…"` line
commented directly above the `path = "stubs/…"` line. To build against the real
private crates: restore the git lines, comment the path lines, and ensure SSH
access to the Bitbucket repo (see `.claude/skills/setting-up-on-macos`).
