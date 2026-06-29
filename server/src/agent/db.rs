//! SQLite wrapper for the agent's metadata (workspaces, sessions, prompts,
//! usage, api_call, pricing_config, model_allowlist). Mirrors the existing
//! `crate::db::Database` pattern: `Mutex<Connection>`, idempotent schema apply
//! via `include_str!`, additive ALTERs via `run_migrations`.
//!
//! Stored at `{data_dir}/agent.db`, alongside the existing `smartstudio.db`.

use anyhow::{anyhow, Result};
use rusqlite::{params_from_iter, types::ToSql, Connection};
use serde_json::Value;
use std::sync::Mutex;

pub struct AgentDb {
    conn: Mutex<Connection>,
    #[allow(dead_code)]
    path: String,
}

impl AgentDb {
    pub fn open(path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(include_str!("schema.sql"))?;
        Self::run_migrations(&conn);
        Ok(Self { conn: Mutex::new(conn), path: path.to_string() })
    }

    /// Idempotent ALTERs for forward-only schema evolution. Errors on
    /// "duplicate column" are silently ignored — matches the pattern in
    /// `crate::db::Database::run_migrations`.
    fn run_migrations(conn: &Connection) {
        let alters = [
            // Pre-discovered schema overview injected into the system
            // prompt per session. Added after the initial session table
            // shipped.
            "ALTER TABLE session ADD COLUMN schema_hint TEXT",
            // Truncated copies of each tool call's args + response so the
            // prompt-detail drawer can show what the agent actually ran.
            "ALTER TABLE api_call ADD COLUMN args_preview TEXT",
            "ALTER TABLE api_call ADD COLUMN response_preview TEXT",
            // Captured error message on failed prompts (Rig error chain,
            // context overflow, max-turns, …). Was previously thrown
            // through the SSE stream and lost; now persisted so the
            // prompt-detail drawer can show it on replay.
            "ALTER TABLE prompt ADD COLUMN error TEXT",
            // Dashboard + widget cache tables shipped after the initial
            // schema. The CREATE statements live in schema.sql (applied
            // via `execute_batch` on open); these ALTERs are placeholders
            // for any future additive columns on those tables — kept here
            // so the migration list stays in one place.
        ];
        for sql in alters {
            // `execute` returns Err when the column already exists;
            // discard since the schema converges to the same shape
            // regardless of starting point.
            let _ = conn.execute(sql, []);
        }
        // Invalidate oversized `session.schema_hint` rows so the next
        // prompt in those sessions re-discovers with whatever the current
        // schema.rs caps allow. Boot-time wipe is fine — the alternative
        // is letting OpenAI's 128K token cap surface as a runtime
        // failure on a session that's been cached since before the
        // current caps were tightened. Threshold matches HINT_BYTE_BUDGET
        // in schema.rs (40 KB) plus a small slack.
        let _ = conn.execute(
            "UPDATE session SET schema_hint = NULL WHERE length(schema_hint) > 50000",
            [],
        );
        // Invalidate hints generated before graph-backed DataViews
        // started emitting their kinds + metrics inline. Detection:
        // the hint mentions a graph DataView but lacks the marker the
        // new `graph_source_hint` adds. Skipping this would leave the
        // agent unable to see `dv_articles_graph`'s real shape
        // (kinds: brand, l0, …; metrics: lw_revenue, oh, …) on
        // pre-existing sessions, and it would silently route brand-
        // level questions to whatever other DV happens to expose
        // `brand` as a stored column.
        let _ = conn.execute(
            "UPDATE session SET schema_hint = NULL \
             WHERE schema_hint LIKE '%dv_articles_graph%' \
               AND schema_hint NOT LIKE '%graph-backed%'",
            [],
        );
    }

    pub fn query(&self, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Value>> {
        let conn = self.conn.lock().map_err(|e| anyhow!("lock: {e}"))?;
        let mut stmt = conn.prepare(sql)?;
        let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt.query_map(params_from_iter(params), |row| {
            let mut obj = serde_json::Map::new();
            for (i, name) in col_names.iter().enumerate() {
                obj.insert(name.clone(), row_value_to_json(row, i));
            }
            Ok(Value::Object(obj))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn query_one(&self, sql: &str, params: &[&dyn ToSql]) -> Result<Value> {
        self.query(sql, params)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("not found"))
    }

    pub fn execute(&self, sql: &str, params: &[&dyn ToSql]) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow!("lock: {e}"))?;
        Ok(conn.execute(sql, params_from_iter(params))?)
    }

    /// Batched inserts used by `meter::writer` to drain its channel without a
    /// per-row commit. Returns the number of rows inserted on success.
    pub fn execute_batch_inserts(
        &self,
        sql: &str,
        rows: impl Iterator<Item = Vec<Box<dyn ToSql>>>,
    ) -> Result<usize> {
        let mut conn = self.conn.lock().map_err(|e| anyhow!("lock: {e}"))?;
        let tx = conn.transaction()?;
        let mut count = 0usize;
        {
            let mut stmt = tx.prepare(sql)?;
            for r in rows {
                let refs: Vec<&dyn ToSql> = r.iter().map(|b| b.as_ref()).collect();
                stmt.execute(params_from_iter(refs.iter().copied()))?;
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }
}

fn row_value_to_json(row: &rusqlite::Row, i: usize) -> Value {
    use rusqlite::types::ValueRef;
    match row.get_ref(i).unwrap_or(ValueRef::Null) {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::from(n),
        ValueRef::Real(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).into_owned();
            // JSON columns roundtrip as parsed values; everything else stays text.
            serde_json::from_str(&s).unwrap_or(Value::String(s))
        }
        ValueRef::Blob(_) => Value::Null,
    }
}
