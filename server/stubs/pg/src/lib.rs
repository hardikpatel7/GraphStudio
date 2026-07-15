//! Inert scaffold stub of the private `pg` crate (Postgres connection pools).
//!
//! This is NOT the real crate — it exists so the public GraphStudio repo
//! compiles without SSH access to `rust-shared-utils`. The server only calls
//! `has_pool` and `initialize_named_pool` (both from `pg_pools.rs`);
//! `get_named_connection` is used solely by the real `pipeline` crate, which is
//! itself stubbed here, so it is intentionally omitted. No pools are opened —
//! live Postgres connectivity is disabled in this build. See CLAUDE.md.

use app_config::database::DatabaseConfig;

/// Inert: there is no pool registry, so nothing is ever "present". The server
/// treats `false` as "not yet initialized" and proceeds to `initialize_named_pool`.
pub fn has_pool(_name: &str) -> bool {
    false
}

/// No-op: accepts the config and reports success without opening a pool. The
/// server logs this as success; the best-effort boot path swallows the rest.
pub async fn initialize_named_pool(
    _name: &str,
    _db_config: DatabaseConfig,
) -> Result<(), anyhow::Error> {
    Ok(())
}
