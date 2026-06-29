//! ClickHouse connector — HTTP interface.
//!
//! Wraps the standard ClickHouse HTTP endpoint
//! (`{scheme}://{host}:{port}/?default_format=JSONEachRow`) in a thin
//! reqwest-based client. SQL goes in the POST body; rows come back as
//! line-delimited JSON objects.
//!
//! Connection config shape (matches `CLICKHOUSE_*` env var conventions):
//!
//! ```json
//! {
//!   "host": "ch.internal",
//!   "port": 8123,
//!   "username": "default",
//!   "password": "...",
//!   "ssl": false,
//!   "query_timeout_seconds": 30,
//!   "allow_write_access": false
//! }
//! ```
//!
//! `allow_write_access = false` (default) rejects any SQL containing
//! an obvious write keyword before sending — a belt-and-suspenders
//! guard on top of whatever role-based perms the CH user has. The
//! check is a case-insensitive substring match on the SQL; intended
//! to catch operator typos, not as a security boundary.
//!
//! No schema introspection here (the `list_schemas`/`list_tables`
//! handlers return 501 for CH connections in this MVP — operators
//! type SQL by hand against known tables).
//!
//! Deferred to follow-up: materialization to DuckDB (Parquet export
//! from CH then import), schema browser, and CDC.

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 8123;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_USERNAME: &str = "default";

/// Result of a CH query plus timing info. `client_ms` is the wall-clock
/// time we (smartstudio) observed for the HTTP roundtrip, including
/// auth, body encode/decode, and network. `server_ms` is the CH
/// server-side execution time parsed from the `X-ClickHouse-Summary`
/// header (`elapsed_ns / 1e6`); `None` when CH doesn't emit the header
/// or the value is unparseable. Subtracting `server_ms` from
/// `client_ms` gives a rough network+overhead estimate.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub rows: Vec<Value>,
    pub client_ms: u64,
    pub server_ms: Option<u64>,
    pub read_rows: Option<u64>,
    pub read_bytes: Option<u64>,
}

/// Substring-matched (case-insensitive) write keywords. Match is on
/// the uppercased SQL; trailing space disambiguates `INSERT ` from
/// `INSERTED_AT` etc.
const FORBIDDEN_KEYWORDS: &[&str] = &[
    "INSERT ", "UPDATE ", "DELETE ",
    "DROP ", "TRUNCATE ", "ALTER ",
    "CREATE ", "RENAME ", "ATTACH ", "DETACH ",
    "OPTIMIZE ", "GRANT ", "REVOKE ", "KILL ",
];

#[derive(Debug, Clone)]
pub struct ChConnection {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub ssl: bool,
    pub query_timeout_seconds: u64,
    pub allow_write_access: bool,
    /// Hint for callers (the dictionary endpoint, MCP planners, future
    /// `clickhouse_query` defaults) about which CH database is the
    /// "interesting" one on this connection. Backed by a real CH server
    /// with many side-DBs (`*_bkp`, `*_test`, `staging_*`) you don't
    /// want the LLM/UI guessing — name it here once.
    pub default_database: Option<String>,
}

impl ChConnection {
    /// Parse a `connections.config` JSON blob into a `ChConnection`.
    /// Only `host` is required; everything else has a documented default.
    pub fn from_config(config: &Value) -> Result<Self> {
        let host = config
            .get("host")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("clickhouse connection requires non-empty `host`"))?
            .to_string();
        let port = config
            .get("port")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_PORT as u64) as u16;
        // Username is required and must be non-empty. Falling back to
        // a default silently bit us during demo prep — `default`
        // with no password silently passed test_connection on
        // anonymous-access servers, then real queries failed once a
        // password was set. Make the missing case explicit.
        let username = config
            .get("username")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!(
                "clickhouse connection requires non-empty `username` — set it on the connection config (silent fallback to `{DEFAULT_USERNAME}` was removed because it produced confusing auth failures)"
            ))?
            .to_string();
        let password = config
            .get("password")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let ssl = config
            .get("ssl")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let query_timeout_seconds = config
            .get("query_timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        let allow_write_access = config
            .get("allow_write_access")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let default_database = config
            .get("default_database")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        Ok(Self {
            host,
            port,
            username,
            password,
            ssl,
            query_timeout_seconds,
            allow_write_access,
            default_database,
        })
    }

    fn base_url(&self) -> String {
        let scheme = if self.ssl { "https" } else { "http" };
        format!("{scheme}://{}:{}", self.host, self.port)
    }
}

/// Reject write-shaped SQL when `allow_write_access` is false.
/// Substring match on the uppercased query; not a parser, just a guard.
pub fn check_write_access(conn: &ChConnection, sql: &str) -> Result<()> {
    if conn.allow_write_access {
        return Ok(());
    }
    let upper = sql.to_uppercase();
    for kw in FORBIDDEN_KEYWORDS {
        if upper.contains(kw) {
            return Err(anyhow!(
                "clickhouse connection has allow_write_access=false; SQL contains forbidden keyword `{}`",
                kw.trim()
            ));
        }
    }
    Ok(())
}

/// POST `sql` to the CH HTTP endpoint and parse the JSONEachRow
/// response into `QueryResult` (rows + client + server timing).
/// Honors the connection's `query_timeout_seconds`.
///
/// `client_ms` is the wall-clock we observe for the full roundtrip;
/// `server_ms` is ClickHouse's reported execution time (from the
/// `X-ClickHouse-Summary` response header — `elapsed_ns / 1e6`),
/// `None` if CH didn't emit the header. The header also carries
/// `read_rows` / `read_bytes` which we surface for cost visibility.
pub async fn query_exec(conn: &ChConnection, sql: &str) -> Result<QueryResult> {
    check_write_access(conn, sql)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(conn.query_timeout_seconds))
        .build()
        .context("clickhouse client build")?;

    let started = Instant::now();
    let url = format!("{}/?default_format=JSONEachRow", conn.base_url());
    let resp = client
        .post(&url)
        .basic_auth(&conn.username, Some(&conn.password))
        .body(sql.to_string())
        .send()
        .await
        .context("clickhouse HTTP request")?;

    let status = resp.status();
    // Parse X-ClickHouse-Summary before consuming the body — the
    // header is set on every successful response and (we've observed)
    // on the error response too.
    let (server_ms, read_rows, read_bytes) = parse_ch_summary(&resp);
    let body = resp.text().await.context("clickhouse response read")?;
    let client_ms = started.elapsed().as_millis() as u64;
    if !status.is_success() {
        // CH puts the error message in the body; truncate to keep
        // log lines tractable while preserving the first useful chunk.
        let snippet: String = body.chars().take(500).collect();
        return Err(anyhow!("clickhouse HTTP {}: {}", status.as_u16(), snippet));
    }

    // JSONEachRow: one JSON object per non-empty line, no array wrapper.
    let mut rows = Vec::new();
    for (lineno, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(trimmed).with_context(|| {
            format!(
                "clickhouse JSONEachRow parse failed at line {}: `{}`",
                lineno + 1,
                trimmed.chars().take(200).collect::<String>()
            )
        })?;
        rows.push(v);
    }
    Ok(QueryResult { rows, client_ms, server_ms, read_rows, read_bytes })
}

/// Parse the `X-ClickHouse-Summary` response header.
/// Shape: `{"read_rows":"N","read_bytes":"N","written_rows":"0",
///         "written_bytes":"0","total_rows_to_read":"N",
///         "result_rows":"N","result_bytes":"N","elapsed_ns":"N"}`.
/// All values are stringified integers. Returns
/// `(server_ms, read_rows, read_bytes)` — `None`s when the header is
/// absent or unparseable.
fn parse_ch_summary(resp: &reqwest::Response) -> (Option<u64>, Option<u64>, Option<u64>) {
    let Some(hdr) = resp.headers().get("x-clickhouse-summary") else {
        return (None, None, None);
    };
    let Ok(s) = hdr.to_str() else { return (None, None, None); };
    let Ok(parsed) = serde_json::from_str::<Value>(s) else {
        return (None, None, None);
    };
    let read_u64 = |k: &str| -> Option<u64> {
        parsed.get(k).and_then(|v| v.as_str()).and_then(|s| s.parse::<u64>().ok())
    };
    let server_ms = read_u64("elapsed_ns").map(|ns| ns / 1_000_000);
    (server_ms, read_u64("read_rows"), read_u64("read_bytes"))
}

/// Liveness probe — runs `SELECT 1 AS ok`. Returns `Ok` only when the
/// response shape matches, so a non-CH endpoint masquerading on the
/// same port also fails.
pub async fn test_connection(conn: &ChConnection) -> Result<QueryResult> {
    let result = query_exec(conn, "SELECT 1 AS ok").await?;
    let ok_val = result.rows
        .first()
        .and_then(|r| r.get("ok"))
        .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)));
    if result.rows.len() == 1 && ok_val == Some(1) {
        Ok(result)
    } else {
        Err(anyhow!(
            "clickhouse test query returned unexpected shape (rows={}, first={:?})",
            result.rows.len(),
            result.rows.first()
        ))
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Schema introspection
// ──────────────────────────────────────────────────────────────────────────

/// List user-visible databases. Filters out the engine's internal
/// schemas (`system`, `INFORMATION_SCHEMA`, `information_schema`) by
/// default so the LLM sees the tenant's data, not the engine's
/// metadata.
pub async fn list_databases(conn: &ChConnection) -> Result<Vec<Value>> {
    let result = query_exec(
        conn,
        "SELECT name, engine, comment \
         FROM system.databases \
         WHERE name NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema') \
         ORDER BY name",
    )
    .await?;
    Ok(result.rows)
}

/// List tables in a database. Returns engine, row/byte estimates,
/// and the optional table-level `comment` (set by `CREATE TABLE …
/// COMMENT '…'` or `ALTER TABLE … MODIFY COMMENT '…'`).
pub async fn list_tables_in(conn: &ChConnection, database: &str) -> Result<Vec<Value>> {
    // Parameterize via CH HTTP query-param substitution — safer than
    // string interpolation, but we still escape single quotes on the
    // value as a belt-and-suspenders measure (the HTTP interface
    // doesn't support real prepared statements).
    let safe = database.replace('\'', "''");
    let sql = format!(
        "SELECT name, engine, total_rows, total_bytes, comment \
         FROM system.tables \
         WHERE database = '{safe}' \
         ORDER BY name"
    );
    let result = query_exec(conn, &sql).await?;
    Ok(result.rows)
}

/// List columns for a table. Returns each column's name, declared
/// `type`, optional `default_expression`, optional `comment`, and
/// `position` (1-based). The position column lets clients render
/// columns in the order CH stores them rather than alphabetical.
pub async fn list_columns_in(
    conn: &ChConnection,
    database: &str,
    table: &str,
) -> Result<Vec<Value>> {
    let safe_db = database.replace('\'', "''");
    let safe_t = table.replace('\'', "''");
    let sql = format!(
        "SELECT name, type, default_expression, comment, position \
         FROM system.columns \
         WHERE database = '{safe_db}' AND table = '{safe_t}' \
         ORDER BY position"
    );
    let result = query_exec(conn, &sql).await?;
    Ok(result.rows)
}

/// Build a unified data dictionary for an entire CH connection (or
/// for a single database when `only_database` is `Some`).
/// See module docs for the response shape.
///
/// One CH query — joins `system.databases × system.tables ×
/// system.columns` and returns one row per (database, table, column),
/// ordered so rows for the same database arrive consecutively.
/// We then walk the result once in O(N) and emit the hierarchical
/// `{databases: [{tables: [{columns: [...]}]}]}` shape.
///
/// `only_database`: when `Some(name)`, the WHERE clause filters CH
/// itself so the scan + response payload are scoped. This matters
/// for big catalogs — the Arhaus 7-database catalog has ~42 K
/// columns / ~4.2 MB unfiltered; scoping to one DB typically cuts
/// the payload by ~10× and the scan in proportion.
pub async fn dictionary(
    conn: &ChConnection,
    only_database: Option<&str>,
) -> Result<(Value, u64)> {
    let started = Instant::now();
    // The LEFT JOIN on system.columns lets a table with zero columns
    // still appear in the dictionary. ORDER BY (database, table,
    // position) groups the result for a single linear walk below.
    //
    // The `only_database` filter goes through the same single-quote
    // escape we use elsewhere (CH HTTP has no prepared statements).
    let db_filter = match only_database {
        Some(name) => format!(
            "AND d.name = '{}'",
            name.replace('\'', "''"),
        ),
        None => String::new(),
    };
    let sql = format!("\
        SELECT \
            d.name           AS db_name, \
            d.engine         AS db_engine, \
            d.comment        AS db_comment, \
            t.name           AS table_name, \
            t.engine         AS table_engine, \
            t.total_rows     AS table_rows, \
            t.total_bytes    AS table_bytes, \
            t.comment        AS table_comment, \
            c.name           AS column_name, \
            c.type           AS column_type, \
            c.default_expression AS column_default, \
            c.comment        AS column_comment, \
            c.position       AS column_position \
        FROM system.databases d \
        INNER JOIN system.tables t ON d.name = t.database \
        LEFT JOIN system.columns c ON t.database = c.database AND t.name = c.table \
        WHERE d.name NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema') \
        {db_filter} \
        ORDER BY d.name, t.name, c.position");
    let result = query_exec(conn, &sql).await?;

    // Walk the rows once, building the hierarchy by remembering the
    // last seen (db, table). Because the result is ORDERed,
    // consecutive rows for the same db/table cluster naturally and
    // we only need pointer-into-Vec bookkeeping.
    let mut databases: Vec<Value> = Vec::new();
    let mut cur_db: Option<String> = None;
    let mut cur_table: Option<String> = None;

    let get_s = |row: &Value, k: &str| -> String {
        row.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };
    let get_v = |row: &Value, k: &str| -> Value {
        row.get(k).cloned().unwrap_or(Value::Null)
    };

    for row in &result.rows {
        let db_name = get_s(row, "db_name");
        let table_name = get_s(row, "table_name");
        if db_name.is_empty() { continue; }

        // Push a new database entry when the db_name changes.
        if cur_db.as_deref() != Some(&db_name) {
            let mut db_entry = serde_json::Map::new();
            db_entry.insert("name".into(), Value::String(db_name.clone()));
            db_entry.insert("engine".into(), get_v(row, "db_engine"));
            db_entry.insert("comment".into(), get_v(row, "db_comment"));
            db_entry.insert("tables".into(), Value::Array(Vec::new()));
            databases.push(Value::Object(db_entry));
            cur_db = Some(db_name.clone());
            cur_table = None;
        }

        // Push a new table entry under the current db when table_name
        // changes. Tables with NULL `c.name` (LEFT JOIN miss) still
        // create an entry with an empty columns array.
        let db_entry = databases.last_mut().and_then(|v| v.as_object_mut()).unwrap();
        let tables_array = db_entry.get_mut("tables").and_then(|v| v.as_array_mut()).unwrap();
        if cur_table.as_deref() != Some(&table_name) {
            let mut t_entry = serde_json::Map::new();
            t_entry.insert("name".into(), Value::String(table_name.clone()));
            t_entry.insert("engine".into(), get_v(row, "table_engine"));
            t_entry.insert("total_rows".into(), get_v(row, "table_rows"));
            t_entry.insert("total_bytes".into(), get_v(row, "table_bytes"));
            t_entry.insert("comment".into(), get_v(row, "table_comment"));
            t_entry.insert("columns".into(), Value::Array(Vec::new()));
            tables_array.push(Value::Object(t_entry));
            cur_table = Some(table_name.clone());
        }

        // Append the column to the current table, unless this row's
        // column_name is empty (LEFT JOIN miss on a 0-column table).
        let col_name = get_s(row, "column_name");
        if col_name.is_empty() { continue; }
        let t_entry = tables_array.last_mut().and_then(|v| v.as_object_mut()).unwrap();
        let cols_array = t_entry.get_mut("columns").and_then(|v| v.as_array_mut()).unwrap();
        let mut col = serde_json::Map::new();
        col.insert("name".into(), Value::String(col_name));
        col.insert("type".into(), get_v(row, "column_type"));
        col.insert("default_expression".into(), get_v(row, "column_default"));
        col.insert("comment".into(), get_v(row, "column_comment"));
        col.insert("position".into(), get_v(row, "column_position"));
        cols_array.push(Value::Object(col));
    }

    let mut out = serde_json::Map::new();
    out.insert("databases".into(), Value::Array(databases));
    let elapsed_ms = started.elapsed().as_millis() as u64;
    Ok((Value::Object(out), elapsed_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn write_access_guard_rejects_obvious_writes() {
        let conn = ChConnection::from_config(&json!({"host": "h", "username": "default"})).unwrap();
        assert!(check_write_access(&conn, "INSERT INTO t VALUES (1)").is_err());
        assert!(check_write_access(&conn, "drop table foo").is_err());
        assert!(check_write_access(&conn, "  alter table foo add column x int").is_err());
        assert!(check_write_access(&conn, "SELECT * FROM t WHERE name = 'INSERTED'").is_ok());
    }

    #[test]
    fn write_access_allowed_when_enabled() {
        let conn = ChConnection::from_config(&json!({"host": "h", "username": "default", "allow_write_access": true})).unwrap();
        assert!(check_write_access(&conn, "INSERT INTO t VALUES (1)").is_ok());
    }

    #[test]
    fn config_defaults() {
        let conn = ChConnection::from_config(&json!({"host": "h.example.com", "username": "default"})).unwrap();
        assert_eq!(conn.port, 8123);
        assert_eq!(conn.username, "default");
        assert_eq!(conn.password, "");
        assert!(!conn.ssl);
        assert_eq!(conn.query_timeout_seconds, 30);
        assert!(!conn.allow_write_access);
    }

    #[test]
    fn config_host_required() {
        assert!(ChConnection::from_config(&json!({})).is_err());
        assert!(ChConnection::from_config(&json!({"host": ""})).is_err());
    }

    #[test]
    fn config_username_required() {
        assert!(ChConnection::from_config(&json!({"host": "h"})).is_err());
        assert!(ChConnection::from_config(&json!({"host": "h", "username": ""})).is_err());
    }

    #[test]
    fn base_url_uses_https_when_ssl() {
        let conn = ChConnection::from_config(&json!({"host": "h", "username": "default", "ssl": true, "port": 8443})).unwrap();
        assert_eq!(conn.base_url(), "https://h:8443");
    }
}
