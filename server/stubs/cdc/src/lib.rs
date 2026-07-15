//! Inert scaffold stub of the private `cdc` crate (Postgres change-data-capture).
//!
//! This is NOT the real crate — it exists so the public GraphStudio repo
//! compiles without SSH access to `rust-shared-utils`. It reproduces only the
//! surface the server uses (`CdcManager` + `CdcStartParams` + `consumer`). CDC
//! is disabled in this build: `start` returns an error (so no source is falsely
//! marked "streaming"), while `stop`/`ensure_slot` are harmless no-ops. See CLAUDE.md.

pub mod consumer;

use std::sync::Arc;

use consumer::PgParams;

/// Opaque, inert manager. The real type tracks live consumers; this one holds
/// nothing. Empty struct is `Send + Sync`, as required by `Arc<AppState>`.
pub struct CdcManager {}

/// Constructed field-for-field by the server. The two closure fields are built
/// server-side and simply never invoked here.
#[derive(Clone)]
pub struct CdcStartParams {
    pub key: String,
    pub pg: PgParams,
    pub slot: String,
    pub publication: String,
    pub start_lsn: String,
    pub duckdb_path: String,
    pub duckdb_table: String,
    pub pk_columns: Vec<String>,
    pub on_lsn_update: Arc<dyn Fn(String) + Send + Sync>,
    pub on_status_update: Arc<dyn Fn(&str) + Send + Sync>,
}

impl CdcManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Honest inert behavior: report that CDC is unavailable rather than
    /// silently succeeding — otherwise the caller would persist a source as
    /// `status = "streaming"` when no WAL consumer exists. The `cdc_start`
    /// handler turns this into a clear error, and boot's `cdc_auto_start_all`
    /// logs a warning and continues (it never unwraps).
    pub async fn start(&self, _params: CdcStartParams) -> Result<(), String> {
        Err("CDC streaming is disabled in the public scaffold build (no rust-shared-utils)".to_string())
    }

    /// Stopping a non-existent stream is a harmless no-op success.
    pub async fn stop(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }
}

impl Default for CdcManager {
    fn default() -> Self {
        Self::new()
    }
}
