//! Inert scaffold stub of the private `app-config` crate.
//!
//! This is NOT the real crate — it exists so the public GraphStudio repo
//! compiles without SSH access to `rust-shared-utils`. It provides only the
//! surface the server actually uses: `app_config::database::DatabaseConfig`.
//! No TOML parsing, no GCP, no Secret Manager loader. See CLAUDE.md
//! ("Vendored scaffold stubs").

pub mod database {
    /// Field-for-field mirror of the real crate's `DatabaseConfig` so the
    /// server's call sites compile unchanged. The server constructs this via
    /// a struct literal in `pg_pools.rs` and hands it to the `pg` stub's
    /// `initialize_named_pool`.
    #[derive(Debug, Clone)]
    pub struct DatabaseConfig {
        pub host: String,
        pub port: u16,
        pub username: String,
        pub password: String,
        pub database: String,
        pub pool_max_size: usize,
        pub pool_timeout_seconds: u64,
    }
}
