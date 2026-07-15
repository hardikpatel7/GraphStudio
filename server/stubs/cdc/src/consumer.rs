//! Inert stand-in for `cdc::consumer`. Only `PgParams` and `ensure_slot` are
//! used by the server.

/// Constructed field-for-field by the server (`handlers/sources.rs`).
#[derive(Debug, Clone)]
pub struct PgParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

/// No-op: pretends the replication slot exists and reports the "no persisted
/// LSN" sentinel `0/0`, which the server treats as a clean starting point.
pub async fn ensure_slot(
    _pg: &PgParams,
    _slot: &str,
    _publication: &str,
    _tables: &[String],
) -> Result<String, String> {
    Ok("0/0".to_string())
}
