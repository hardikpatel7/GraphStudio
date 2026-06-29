/// Centralized activity / event log backed by a single DuckDB file.
///
/// All events (activity, errors, pipeline runs) go into one `events` table,
/// distinguished by `category`. Settings and snapshots live in separate tables.
///
///   <tenant_data_dir>/log.duckdb
///     events         — unified activity log (one table to rule them all)
///     env_settings   — per-tenant key-value config
///     snapshots      — parquet snapshot history per dataview+step

use anyhow::{Result, anyhow};
use duckdb::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::broadcast;

/// Single DuckDB log database + per-tenant SSE broadcast channels.
pub struct TraceManager {
    conn: Mutex<Connection>,
    /// Per-tenant broadcast channels for SSE push.
    channels: Mutex<HashMap<String, broadcast::Sender<Value>>>,
}

impl TraceManager {
    pub fn new(log_path: &str) -> Self {
        if let Some(parent) = std::path::Path::new(log_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(log_path).expect("Failed to open log.duckdb");
        init_log_schema(&conn).expect("Failed to init log schema");
        TraceManager {
            conn: Mutex::new(conn),
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// Subscribe to real-time events for a tenant. Returns a broadcast receiver.
    pub fn subscribe(&self, tenant_id: &str) -> broadcast::Receiver<Value> {
        let mut channels = self.channels.lock().unwrap();
        let sender = channels.entry(tenant_id.to_string())
            .or_insert_with(|| broadcast::channel(256).0);
        sender.subscribe()
    }

    /// Broadcast an event to all SSE subscribers for a tenant.
    fn broadcast(&self, tenant_id: &str, event: &Value) {
        if let Ok(channels) = self.channels.lock() {
            if let Some(sender) = channels.get(tenant_id) {
                sender.send(event.clone()).ok();
            }
        }
    }

    /// Execute a closure with the DuckDB connection.
    fn with_conn<F, T>(&self, f: F) -> Result<T>
    where F: FnOnce(&Connection) -> Result<T>
    {
        let conn = self.conn.lock().map_err(|e| anyhow!("lock: {}", e))?;
        f(&conn)
    }

    // ── Events (unified activity log) ───────────────────────────────────

    /// Log an activity event.
    pub fn log_activity(&self, tenant_id: &str, category: &str, action: &str, status: &str, message: &str, detail: Option<&str>, duration_ms: Option<i64>) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO events (tenant_id, category, action, status, message, detail, duration_ms) VALUES (?, ?, ?, ?, ?, ?, ?)",
                duckdb::params![tenant_id, category, action, status, message, detail.unwrap_or(""), duration_ms.unwrap_or(0)],
            )?;
            Ok(())
        })?;
        self.broadcast(tenant_id, &json!({
            "type": "activity", "category": category, "action": action,
            "status": status, "message": message, "duration_ms": duration_ms.unwrap_or(0),
        }));
        Ok(())
    }

    /// Log an error (also stored with category='error' in events).
    pub fn log_error(&self, tenant_id: &str, source: &str, message: &str, detail: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO events (tenant_id, category, action, status, message, detail) VALUES (?, 'error', ?, 'failed', ?, ?)",
                duckdb::params![tenant_id, source, message, detail],
            )?;
            Ok(())
        })?;
        self.broadcast(tenant_id, &json!({
            "type": "error", "source": source, "message": message, "status": "failed",
        }));
        Ok(())
    }

    /// Log a pipeline run (category='pipeline').
    pub fn log_pipeline_run(&self, tenant_id: &str, dataview_id: &str, status: &str, tasks_json: &str, duration_ms: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO events (tenant_id, category, action, status, message, detail, duration_ms) VALUES (?, 'pipeline', ?, ?, ?, ?, ?)",
                duckdb::params![tenant_id, dataview_id, status, dataview_id, tasks_json, duration_ms],
            )?;
            Ok(())
        })
    }

    /// Toggle follow-up flag on an event.
    pub fn toggle_follow_up(&self, _tenant_id: &str, row_id: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE events SET follow_up = NOT follow_up WHERE rowid = ?",
                duckdb::params![row_id],
            )?;
            Ok(())
        })
    }

    /// Query activity log for a tenant.
    pub fn get_activity(&self, tenant_id: &str, limit: i64, offset: i64, category: Option<&str>, hours_ago: Option<i64>, follow_up_only: bool) -> Result<Value> {
        self.with_conn(|conn| {
            let mut conditions = vec!["tenant_id = ?".to_string()];
            let mut params: Vec<Box<dyn duckdb::ToSql>> = vec![Box::new(tenant_id.to_string())];

            if let Some(cat) = category {
                conditions.push("category = ?".to_string());
                params.push(Box::new(cat.to_string()));
            }
            if let Some(hours) = hours_ago {
                conditions.push(format!("timestamp >= current_timestamp::TIMESTAMP - INTERVAL '{} hours'", hours));
            }
            if follow_up_only {
                conditions.push("follow_up = true".to_string());
            }

            let where_clause = format!("WHERE {}", conditions.join(" AND "));
            let sql = format!("SELECT rowid, * FROM events {} ORDER BY timestamp DESC LIMIT {} OFFSET {}", where_clause, limit, offset);

            let mut stmt = conn.prepare(&sql)?;
            let param_refs: Vec<&dyn duckdb::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let frames = stmt.query_arrow(param_refs.as_slice())?;

            let mut rows = Vec::new();
            for batch in frames {
                let col_names: Vec<String> = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                for row_idx in 0..batch.num_rows() {
                    let mut obj = serde_json::Map::new();
                    for (col_idx, name) in col_names.iter().enumerate() {
                        obj.insert(name.clone(), crate::query::arrow_to_json(batch.column(col_idx), row_idx));
                    }
                    rows.push(Value::Object(obj));
                }
            }

            let count_sql = format!("SELECT COUNT(*) FROM events {}", where_clause);
            let count_params: Vec<&dyn duckdb::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let total: i64 = conn.query_row(&count_sql, count_params.as_slice(), |r| r.get(0)).unwrap_or(0);

            Ok(json!({ "rows": rows, "total": total }))
        })
    }

    /// Query errors for a tenant.
    pub fn get_errors(&self, tenant_id: &str, limit: i64) -> Result<Value> {
        self.with_conn(|conn| {
            let sql = format!(
                "SELECT rowid, * FROM events WHERE tenant_id = ? AND category = 'error' ORDER BY timestamp DESC LIMIT {}",
                limit
            );
            let mut stmt = conn.prepare(&sql)?;
            let frames = stmt.query_arrow(duckdb::params![tenant_id])?;

            let mut rows = Vec::new();
            for batch in frames {
                let col_names: Vec<String> = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                for row_idx in 0..batch.num_rows() {
                    let mut obj = serde_json::Map::new();
                    for (col_idx, name) in col_names.iter().enumerate() {
                        obj.insert(name.clone(), crate::query::arrow_to_json(batch.column(col_idx), row_idx));
                    }
                    rows.push(Value::Object(obj));
                }
            }
            Ok(json!({ "rows": rows }))
        })
    }

    /// Query pipeline runs for a tenant.
    pub fn get_pipeline_runs(&self, tenant_id: &str, limit: i64) -> Result<Value> {
        self.with_conn(|conn| {
            let sql = format!(
                "SELECT rowid, * FROM events WHERE tenant_id = ? AND category = 'pipeline' ORDER BY timestamp DESC LIMIT {}",
                limit
            );
            let mut stmt = conn.prepare(&sql)?;
            let frames = stmt.query_arrow(duckdb::params![tenant_id])?;

            let mut rows = Vec::new();
            for batch in frames {
                let col_names: Vec<String> = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                for row_idx in 0..batch.num_rows() {
                    let mut obj = serde_json::Map::new();
                    for (col_idx, name) in col_names.iter().enumerate() {
                        obj.insert(name.clone(), crate::query::arrow_to_json(batch.column(col_idx), row_idx));
                    }
                    rows.push(Value::Object(obj));
                }
            }
            Ok(json!({ "rows": rows }))
        })
    }

    // ── Settings ────────────────────────────────────────────────────────

    /// Set an environment setting.
    pub fn set_setting(&self, tenant_id: &str, key: &str, value: &str) -> Result<()> {
        self.with_conn(|conn| {
            // DuckDB doesn't support ON CONFLICT for non-PK; delete+insert
            conn.execute(
                "DELETE FROM env_settings WHERE tenant_id = ? AND key = ?",
                duckdb::params![tenant_id, key],
            )?;
            conn.execute(
                "INSERT INTO env_settings (tenant_id, key, value) VALUES (?, ?, ?)",
                duckdb::params![tenant_id, key, value],
            )?;
            Ok(())
        })
    }

    /// Get an environment setting.
    pub fn get_setting(&self, tenant_id: &str, key: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            match conn.query_row(
                "SELECT value FROM env_settings WHERE tenant_id = ? AND key = ?",
                duckdb::params![tenant_id, key],
                |row| row.get::<_, String>(0),
            ) {
                Ok(v) => Ok(Some(v)),
                Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Get all settings for a tenant.
    pub fn get_all_settings(&self, tenant_id: &str) -> Result<Value> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT key, value FROM env_settings WHERE tenant_id = ? ORDER BY key")?;
            let rows = stmt.query_map(duckdb::params![tenant_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut map = serde_json::Map::new();
            for r in rows.flatten() {
                map.insert(r.0, Value::String(r.1));
            }
            Ok(Value::Object(map))
        })
    }

    // ── Snapshots ───────────────────────────────────────────────────────

    /// Record a new snapshot.
    pub fn record_snapshot(&self, tenant_id: &str, dataview_id: &str, step: &str, path: &str, snapshot_ts: &str, row_count: i64, max_keep: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE snapshots SET active = false WHERE tenant_id = ? AND dataview_id = ? AND step = ?",
                duckdb::params![tenant_id, dataview_id, step],
            )?;
            conn.execute(
                "INSERT INTO snapshots (tenant_id, dataview_id, step, path, snapshot_ts, row_count, active) VALUES (?, ?, ?, ?, ?, ?, true)",
                duckdb::params![tenant_id, dataview_id, step, path, snapshot_ts, row_count],
            )?;
            conn.execute(
                &format!(
                    "DELETE FROM snapshots WHERE tenant_id = ? AND dataview_id = ? AND step = ? AND created_at NOT IN (SELECT created_at FROM snapshots WHERE tenant_id = ? AND dataview_id = ? AND step = ? ORDER BY created_at DESC LIMIT {})",
                    max_keep
                ),
                duckdb::params![tenant_id, dataview_id, step, tenant_id, dataview_id, step],
            )?;
            Ok(())
        })
    }

    /// Get snapshots for a dataview.
    pub fn get_snapshots(&self, tenant_id: &str, dataview_id: &str) -> Result<Value> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT rowid, tenant_id, dataview_id, step, path, snapshot_ts, row_count, active, created_at FROM snapshots WHERE tenant_id = ? AND dataview_id = ? ORDER BY step, created_at DESC"
            )?;
            let frames = stmt.query_arrow(duckdb::params![tenant_id, dataview_id])?;
            let mut rows = Vec::new();
            for batch in frames {
                let col_names: Vec<String> = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                for row_idx in 0..batch.num_rows() {
                    let mut obj = serde_json::Map::new();
                    for (col_idx, name) in col_names.iter().enumerate() {
                        obj.insert(name.clone(), crate::query::arrow_to_json(batch.column(col_idx), row_idx));
                    }
                    rows.push(Value::Object(obj));
                }
            }
            Ok(json!(rows))
        })
    }

    /// Get the active snapshot for a dataview+step.
    pub fn get_active_snapshot(&self, tenant_id: &str, dataview_id: &str, step: &str) -> Result<Option<Value>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT rowid, tenant_id, dataview_id, step, path, snapshot_ts, row_count, active, created_at FROM snapshots WHERE tenant_id = ? AND dataview_id = ? AND step = ? AND active = true LIMIT 1"
            )?;
            let frames = stmt.query_arrow(duckdb::params![tenant_id, dataview_id, step])?;
            for batch in frames {
                if batch.num_rows() > 0 {
                    let col_names: Vec<String> = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                    let mut obj = serde_json::Map::new();
                    for (col_idx, name) in col_names.iter().enumerate() {
                        obj.insert(name.clone(), crate::query::arrow_to_json(batch.column(col_idx), 0));
                    }
                    return Ok(Some(Value::Object(obj)));
                }
            }
            Ok(None)
        })
    }

    /// Switch active snapshot for a dataview+step.
    pub fn set_active_snapshot(&self, tenant_id: &str, dataview_id: &str, step: &str, snapshot_ts: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE snapshots SET active = false WHERE tenant_id = ? AND dataview_id = ? AND step = ?",
                duckdb::params![tenant_id, dataview_id, step],
            )?;
            conn.execute(
                "UPDATE snapshots SET active = true WHERE tenant_id = ? AND dataview_id = ? AND step = ? AND snapshot_ts = ?",
                duckdb::params![tenant_id, dataview_id, step, snapshot_ts],
            )?;
            Ok(())
        })
    }
}

fn init_log_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER,
            tenant_id VARCHAR NOT NULL,
            timestamp TIMESTAMP DEFAULT current_timestamp,
            category VARCHAR NOT NULL,
            action VARCHAR NOT NULL,
            status VARCHAR NOT NULL DEFAULT 'info',
            message VARCHAR,
            detail VARCHAR,
            duration_ms BIGINT DEFAULT 0,
            follow_up BOOLEAN DEFAULT false
        );

        CREATE TABLE IF NOT EXISTS env_settings (
            tenant_id VARCHAR NOT NULL,
            key VARCHAR NOT NULL,
            value VARCHAR NOT NULL,
            updated_at TIMESTAMP DEFAULT current_timestamp,
            PRIMARY KEY (tenant_id, key)
        );

        CREATE TABLE IF NOT EXISTS snapshots (
            id INTEGER,
            tenant_id VARCHAR NOT NULL,
            dataview_id VARCHAR NOT NULL,
            step VARCHAR NOT NULL,
            path VARCHAR NOT NULL,
            snapshot_ts VARCHAR NOT NULL,
            row_count BIGINT DEFAULT 0,
            active BOOLEAN DEFAULT false,
            created_at TIMESTAMP DEFAULT current_timestamp
        );
    ")?;
    Ok(())
}
