//! Initialize one named `deadpool_postgres` pool per row in the
//! smartstudio `connections` table (where `type IN ('pg', 'postgres')`).
//!
//! The pipeline crate's pg_extract steps obtain pooled clients via
//! `pg::get_named_connection(connection_ref)`. The pool name MUST match
//! the connection_ref id stored in `connections.id` and referenced by
//! pg_extract step configs. Keeps the convention simple: pool name ==
//! connection id.
//!
//! Pool size is sized to `available_parallelism()` by default. The cap
//! bounds total concurrent PG connections opened anywhere in a pipeline
//! run — every `pg_extract`, every parallel-COPY stream, every
//! partitioned batch goes through the same pool, so a heavy run can
//! never exceed `max_size` connections per upstream PG, even when
//! multiple steps fan out concurrently.

use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

use crate::AppState;
use app_config::database::DatabaseConfig;

/// Default pool size: number of logical CPUs the process can use,
/// respecting cgroup limits on Linux. Falls back to 4 on the rare
/// error path. Override via env var `SMARTSTUDIO_PG_MAX_CONCURRENCY`.
fn default_max_size() -> usize {
    if let Ok(v) = std::env::var("SMARTSTUDIO_PG_MAX_CONCURRENCY") {
        if let Ok(n) = v.parse::<usize>() {
            if n > 0 {
                return n;
            }
        }
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Build a `DatabaseConfig` from a smartstudio `connections.config` JSON
/// blob. Missing host / port / user / password / database → returns
/// `None`; the caller skips the row and emits a warn.
fn db_config_from_smartstudio_row(row: &Value, max_size: usize) -> Option<DatabaseConfig> {
    let cfg = row.get("config")?;
    Some(DatabaseConfig {
        host: cfg.get("host")?.as_str()?.to_string(),
        port: cfg
            .get("port")
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))?
            as u16,
        username: cfg.get("user")?.as_str()?.to_string(),
        password: cfg.get("password")?.as_str()?.to_string(),
        database: cfg.get("database")?.as_str()?.to_string(),
        pool_max_size: max_size,
        pool_timeout_seconds: 30,
    })
}

/// Read every pg/postgres connection from smartstudio's SQLite and
/// initialize a deadpool-postgres pool named after each row's id.
/// Idempotent: `pg::initialize_named_pool` is a no-op if a pool with
/// the same name is already registered.
pub async fn init_from_connections(state: Arc<AppState>) -> Result<()> {
    let rows = match state.db.query("SELECT * FROM connections", &[]) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "[pg-pool] failed to read connections table — no pools initialized");
            return Ok(());
        }
    };

    let max_size = default_max_size();
    let mut initialized = 0usize;
    let mut skipped = 0usize;

    for row in rows {
        let id = match row.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => {
                skipped += 1;
                continue;
            }
        };
        let kind = row.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind != "pg" && kind != "postgres" {
            continue;
        }
        if pg::has_pool(&id) {
            // Already initialized in a previous boot or earlier in this
            // process; nothing to do.
            initialized += 1;
            continue;
        }

        let Some(db_cfg) = db_config_from_smartstudio_row(&row, max_size) else {
            tracing::warn!(connection = %id, "[pg-pool] connection has incomplete config; skipping");
            skipped += 1;
            continue;
        };

        match pg::initialize_named_pool(&id, db_cfg).await {
            Ok(()) => {
                tracing::info!(connection = %id, max_size, "[pg-pool] initialized");
                initialized += 1;
            }
            Err(e) => {
                tracing::warn!(connection = %id, error = %e, "[pg-pool] init failed; skipping");
                skipped += 1;
            }
        }
    }

    tracing::info!(
        max_size,
        initialized,
        skipped,
        "[pg-pool] init complete"
    );
    Ok(())
}

/// Reinitialize a single pool — called after a connection row is
/// inserted or its config edited. Best-effort; logs and swallows
/// errors so the HTTP handler can return 200 to the user even when the
/// connection itself can't talk to PG. Operators see the warn line and
/// fix the config.
pub async fn refresh_one(state: &AppState, connection_id: &str) {
    let row = match state
        .db
        .query_one(
            "SELECT * FROM connections WHERE id = ?1",
            &[&connection_id as &dyn rusqlite::types::ToSql],
        ) {
        Ok(r) => r,
        Err(_) => return,
    };
    let kind = row.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if kind != "pg" && kind != "postgres" {
        return;
    }
    let max_size = default_max_size();
    let Some(db_cfg) = db_config_from_smartstudio_row(&row, max_size) else {
        return;
    };
    if let Err(e) = pg::initialize_named_pool(connection_id, db_cfg).await {
        tracing::warn!(connection = %connection_id, error = %e, "[pg-pool] refresh failed");
    } else {
        tracing::info!(connection = %connection_id, "[pg-pool] refreshed");
    }
}
