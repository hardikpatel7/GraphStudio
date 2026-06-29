//! Apply `<tenant_data_dir>/duckdb_views/*.sql` files to the tenant
//! DuckDB at boot. Each file should contain one or more `CREATE OR
//! REPLACE VIEW` statements (DDL only — DML is rejected); the loader
//! applies them in lexicographic filename order via `execute_batch` so
//! dependencies between views resolve as long as the filename ordering
//! reflects the dependency graph.
//!
//! Files arrive in the tenant data dir via product-template copy at
//! `is_new=true` bootstrap (`instance_config::copy_product_templates`);
//! operators edit the per-tenant copies thereafter.
//!
//! Behavior:
//!   - Missing directory → log and skip (fresh tenants don't have it yet).
//!   - Per-file failure → log a warning, continue with the rest. The view
//!     is just not applied; subsequent DataView reads against it will
//!     surface a clear "table/view not found" error at read time.
//!   - Boot is never aborted by this loader.
//!
//! Idempotency: the SQL files use `CREATE OR REPLACE VIEW`, so applying
//! the same file twice is a no-op.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::AppState;

/// Forbid anything that isn't CREATE / OR REPLACE / VIEW + SELECT. Belt
/// and suspenders — the directory is source-controlled, but we still
/// reject obvious mistakes so a stray INSERT/DROP can't slip through.
const FORBIDDEN_PATTERN: &[&str] = &[
    "INSERT ", "UPDATE ", "DELETE ",
    "DROP ", "TRUNCATE ", "ALTER ",
    "ATTACH ", "DETACH ", "COPY ", "EXPORT ",
    "PRAGMA ", "CALL ",
];

pub fn seed_duckdb_views(state: &Arc<AppState>) {
    let dir = Path::new(&state.data_dir).join("duckdb_views");
    let entries = match std::fs::read_dir(&dir) {
        Ok(d) => d,
        Err(e) => {
            tracing::info!(error = %e, path = %dir.display(),
                "[seed_duckdb_views] directory missing, skipping");
            return;
        }
    };

    // Lexicographic order so callers can express dependencies via filename
    // (e.g. `v_aid_per_store.sql` is applied before `v_store_group_performance.sql`
    // because the latter depends on the former, and `_aid_` < `_store_group_`).
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sql"))
        .collect();
    files.sort();

    let conn = match duckdb::Connection::open(&state.duckdb_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, path = %state.duckdb_path,
                "[seed_duckdb_views] could not open tenant DuckDB; skipping");
            return;
        }
    };

    let total = files.len();
    let mut applied = 0usize;
    let mut failed = 0usize;
    for path in files {
        let sql = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "[seed_duckdb_views] read failed; skipping");
                failed += 1;
                continue;
            }
        };
        if let Some(bad) = forbidden_keyword(&sql) {
            tracing::warn!(path = %path.display(), bad,
                "[seed_duckdb_views] file contains a forbidden keyword; skipping");
            failed += 1;
            continue;
        }
        match conn.execute_batch(&sql) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "[seed_duckdb_views] applied");
                applied += 1;
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(),
                    "[seed_duckdb_views] apply failed");
                failed += 1;
            }
        }
    }
    tracing::info!(total, applied, failed,
        "[seed_duckdb_views] done");
}

/// Returns the first forbidden token found, uppercase, or None.
/// Case-insensitive search on the uppercased SQL.
fn forbidden_keyword(sql: &str) -> Option<&'static str> {
    let upper = sql.to_uppercase();
    FORBIDDEN_PATTERN.iter().copied().find(|kw| upper.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_keyword_catches_common_cases() {
        assert!(forbidden_keyword("CREATE OR REPLACE VIEW v AS SELECT 1").is_none());
        assert!(forbidden_keyword("INSERT INTO foo VALUES (1)").is_some());
        assert!(forbidden_keyword("drop view foo").is_some());
        assert!(forbidden_keyword("-- DROP is fine in a comment\nCREATE VIEW v AS SELECT 1").is_some());
        // ^ the simple substring check would catch that; the SQL files are
        // source-controlled so a stray DROP in a comment is itself worth a warning.
    }
}
