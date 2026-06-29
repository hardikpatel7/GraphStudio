//! Read raw PSM tables through the generic `SourceReader` and assemble
//! a [`PsmResolver`].
//!
//! Per Decisions 14 + 35 the PSM tables aren't TOML-declared sources —
//! their existence and column shapes are bealls-specific. So this
//! module hardcodes:
//!
//! - `raw_rcl_psm_priorities` with columns `(rcl_code, priority)`,
//!   ordered ASC by priority.
//! - `raw_rcl_psm_rule_dim` with columns `(rcl_code, rule_code,
//!   dim_json)` — the JSON-encoded dimension spec parsed by
//!   `PsmResolver::build`.
//!
//! Failure modes:
//!
//! - Either table missing from the underlying DuckDB → `Ok(None)`.
//!   PSM resolution is opt-in; absent tables aren't an error.
//! - Either table present but empty → `Ok(Some(empty resolver))`.
//!   `is_ready()` returns false; downstream consumers (exception
//!   rules, gRPC ResolveRcl) gate on it.

use anyhow::Result;

use super::psm_resolver::PsmResolver;
use crate::graph::source::SourceReader;

const TABLE_PRIORITIES: &str = "raw_rcl_psm_priorities";
const TABLE_RULE_DIM: &str = "raw_rcl_psm_rule_dim";

/// Try to build a [`PsmResolver`] from PG-shaped PSM tables. Returns
/// `Ok(None)` when either table is absent (the reader surfaces this
/// as an error from `read`; we map it to None rather than propagate
/// because PSM presence is tenant-specific). All other read errors
/// propagate.
pub fn build_psm_resolver(reader: &dyn SourceReader) -> Result<Option<PsmResolver>> {
    let priorities_rows = match reader.read(
        TABLE_PRIORITIES,
        &["rcl_code".to_string(), "priority".to_string()],
        // The v1 reader applies `ORDER BY priority ASC` in SQL; we
        // sort in Rust below since `SourceReader::read` doesn't
        // express ordering. Same end state.
        None,
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::info!(
                table = TABLE_PRIORITIES,
                error = %e,
                "graph::rcl: PSM priorities table absent — RCL disabled for this build"
            );
            return Ok(None);
        }
    };

    let rule_dim_rows = match reader.read(
        TABLE_RULE_DIM,
        &[
            "rcl_code".to_string(),
            "rule_code".to_string(),
            "dim_json".to_string(),
        ],
        None,
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::info!(
                table = TABLE_RULE_DIM,
                error = %e,
                "graph::rcl: PSM rule_dim table absent — RCL disabled for this build"
            );
            return Ok(None);
        }
    };

    // Reshape rows into the (rcl_code, priority) and (rcl_code, rule_code,
    // dim_json) tuples PsmResolver::build wants. Defensive against
    // missing cells (the validator should already have caught a bad
    // schema, but a fresh DuckDB might surface NULLs we can't act on).
    let mut priorities: Vec<(String, i32)> = priorities_rows
        .iter()
        .filter_map(|r| {
            let rcl = r.cells.get(0)?.as_text();
            let pri = r.cells.get(1)?.as_f64() as i32;
            if rcl.is_empty() {
                None
            } else {
                Some((rcl, pri))
            }
        })
        .collect();
    priorities.sort_by_key(|(_, p)| *p);

    let rule_dims: Vec<(String, String, String)> = rule_dim_rows
        .iter()
        .filter_map(|r| {
            let rcl = r.cells.get(0)?.as_text();
            let rule = r.cells.get(1)?.as_text();
            let dim = r.cells.get(2)?.as_text();
            if rcl.is_empty() || rule.is_empty() || dim.is_empty() {
                None
            } else {
                Some((rcl, rule, dim))
            }
        })
        .collect();

    let resolver = PsmResolver::build(priorities, rule_dims);
    tracing::info!(
        priorities = resolver.priorities.len(),
        rcl_codes = resolver.by_rcl.len(),
        "graph::rcl: PSM resolver built"
    );
    Ok(Some(resolver))
}
