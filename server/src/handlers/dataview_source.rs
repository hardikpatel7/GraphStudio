//! Dataview source resolution: introspection (schema) and read (data).
//!
//! A dataview row has a `source` JSON column with shape:
//! ```json
//! { "type": "pipeline" | "duckdb_table" | "parquet_glob"
//!         | "duckdb_query" | "pg_query" | "bq_query",
//!   "config": { ...kind-specific... } }
//! ```
//!
//! For `pipeline`, the sink (where the pipeline writes its output) is either set
//! explicitly under `config.sink = { "kind": "duckdb_table" | "parquet_glob",
//! "ref": "..." }`, or inferred at call time from the dataview's
//! `backend_workflow.pipeline` terminal step.

use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use crate::AppState;
use super::err;

// ----------------------------------------------------------------------------
// Source resolution helpers
// ----------------------------------------------------------------------------

/// Load the dataview row and resolve its `source` field through the Source
/// table. The DataView always binds via `{type:"source", source_id, output?}`
/// (post-source-unification); inline source shapes are no longer accepted.
fn load_source(state: &AppState, dv_id: &str) -> Result<(Value, Value), (axum::http::StatusCode, Json<Value>)> {
    let row = state.db.query_one("SELECT * FROM dataviews WHERE id = ?1",
        &[&dv_id as &dyn rusqlite::types::ToSql])
        .map_err(|_| err(404, "DataView not found"))?;
    let src = row.get("source").cloned().unwrap_or(json!({}));
    let resolved = resolve_source_binding(state, src)?;
    Ok((row, resolved))
}

/// Resolve a DataView's `source` field through the `sources` table.
///
/// Expects `{"type":"source","config":{"source_id":"...","output":"..."}}`.
/// Looks up the Source row by id and projects it to the kind-specific inline
/// shape that downstream code (dataview_select_sql) consumes:
///   pg_query / bq_query / duckdb_query / parquet_glob → same `type`, same `config`
///   duckdb_table  → `{type:"duckdb_table", config:{table_name:<target_table>}}`
///   cdc_pg        → `{type:"duckdb_table", config:{table_name:<target_table>}}`
fn resolve_source_binding(state: &AppState, source: Value) -> Result<Value, (axum::http::StatusCode, Json<Value>)> {
    if source.get("type").and_then(|v| v.as_str()) != Some("source") {
        return Err(err(400, &format!(
            "DataView source must be {{type:'source', config:{{source_id, output?}}}}. \
             Inline source shapes are no longer supported. (got: {})",
            source.get("type").and_then(|v| v.as_str()).unwrap_or("(none)")
        )));
    }
    let source_id = source.get("config")
        .and_then(|c| c.get("source_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if source_id.is_empty() {
        return Err(err(400, "source binding requires config.source_id"));
    }
    let row = state.db.query_one(
        "SELECT * FROM sources WHERE id = ?1",
        &[&source_id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, &format!("Source '{source_id}' not found")))?;

    // The db layer already parses `config` as JSON for us (it's in JSON_COLS).
    let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let cfg  = row.get("config").cloned().unwrap_or(json!({}));
    let target_table = row.get("target_table").and_then(|v| v.as_str()).unwrap_or("");

    let inline = match kind {
        // Pass-through kinds: keep their config as-is.
        "pg_query" | "bq_query" | "duckdb_query" | "parquet_glob" | "ch_query" => {
            json!({ "type": kind, "config": cfg })
        }
        // Materialized kinds: read from the underlying DuckDB table.
        "duckdb_table" | "cdc_pg" => {
            if target_table.is_empty() {
                return Err(err(400, &format!(
                    "Source '{source_id}' (kind={kind}) has no target_table set."
                )));
            }
            json!({ "type": "duckdb_table", "config": { "table_name": target_table } })
        }
        // In-memory graph snapshot. Pass node_kind through; the read
        // path routes to `graph::legacy::projection` rather than DuckDB.
        "graph" => {
            json!({ "type": "graph", "config": cfg })
        }
        other => {
            return Err(err(400, &format!("Source '{source_id}' has unknown kind '{other}'")));
        }
    };
    Ok(inline)
}

fn resolve_parquet_path(parquet_home: &str, path: &str) -> String {
    if path.contains("{PARQUET_HOME}") {
        path.replace("{PARQUET_HOME}", parquet_home).replace("${PARQUET_HOME}", parquet_home)
    } else if path.starts_with('/') || path.starts_with("gs://") {
        path.to_string()
    } else {
        format!("{}/{}", parquet_home.trim_end_matches('/'), path)
    }
}

/// Resolve which `target_kind` nodes are reachable from `filter_node`.
/// Used by the graph branch of `data()` to honor filters whose
/// `column` is a kind name — e.g. `filter=brand=DASH` on an article
/// projection should return only the articles connected to brand
/// node DASH.
///
/// Three cases, tried in order:
///   1. `filter_node`'s kind == `target_kind` — return [filter_node].
///   2. Same hierarchy: walk the spine subtree (descendants) under
///      `filter_node` and collect `target_kind` nodes. Covers the
///      common case of filtering article-level rows by a hierarchy
///      ancestor (l1, l2, …, ph_code).
///   3. Different hierarchy: scan registered cross-edges for one
///      that connects `filter_node`'s kind to `target_kind`. Use the
///      forward or reverse index depending on which side the filter
///      kind sits on. Covers `brand → article`, `store_group → store`,
///      `dc → store`, etc.
///
/// Returns an empty Vec when no path is found. Multi-hop walks
/// (e.g. brand → article → product_code) are out of scope here —
/// they'd need a path search, and the bealls graph hasn't surfaced
/// the use case yet.
fn nodes_of_kind_from(
    g: &crate::graph::graph::Graph,
    filter_node: crate::graph::graph::NodeId,
    target_kind: crate::graph::graph::KindId,
) -> Vec<crate::graph::graph::NodeId> {
    use crate::graph::graph::NodeId;
    let filter_kind = g.node(filter_node).kind;
    if filter_kind == target_kind {
        return vec![filter_node];
    }

    // Spine descent: BFS through children, collecting target-kind hits.
    let mut out: Vec<NodeId> = Vec::new();
    let mut stack: Vec<NodeId> = vec![filter_node];
    while let Some(id) = stack.pop() {
        let n = g.node(id);
        if n.kind == target_kind {
            out.push(id);
            // Don't descend past a target-kind hit — preserves the
            // "leaves of a subtree at this level" semantic. If a
            // target_kind has further children of the same kind, the
            // caller should request a deeper level.
            continue;
        }
        for c in n.children.iter() {
            stack.push(*c);
        }
    }
    if !out.is_empty() {
        return out;
    }

    // Cross-edge fallback. First-match wins; bealls has at most one
    // direct edge between any (kind_a, kind_b) pair.
    for (i, meta) in g.cross_edges.metas.iter().enumerate() {
        let eid = crate::graph::graph::CrossEdgeId(i as u32);
        let idx = g.cross_edges.get(eid);
        if meta.kind_a == filter_kind && meta.kind_b == target_kind {
            if let Some(v) = idx.forward.get(&filter_node) {
                return v.iter().copied().collect();
            }
        }
        if meta.kind_b == filter_kind && meta.kind_a == target_kind {
            if let Some(v) = idx.reverse.get(&filter_node) {
                return v.iter().copied().collect();
            }
        }
    }
    Vec::new()
}

/// Construct a SELECT query that yields the dataview's rows. For duckdb_table /
/// parquet_glob this is a straightforward SELECT; for *_query kinds it's the user's SQL
/// wrapped as a subquery so we can apply outer LIMIT/OFFSET/sort safely.
fn dataview_select_sql(state: &AppState, source: &Value) -> Result<(String, &'static str), (axum::http::StatusCode, Json<Value>)> {
    let kind = source.get("type").and_then(|v| v.as_str()).unwrap_or("duckdb_table");
    let cfg  = source.get("config").cloned().unwrap_or(json!({}));

    match kind {
        "duckdb_table" => {
            let t = cfg.get("table_name").and_then(|v| v.as_str()).unwrap_or("");
            if t.is_empty() { return Err(err(400, "duckdb_table source requires config.table_name")); }
            Ok((format!("SELECT * FROM {}", t), "duckdb"))
        }
        "parquet_glob" => {
            let p = cfg.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if p.is_empty() { return Err(err(400, "parquet_glob source requires config.path")); }
            let abs = resolve_parquet_path(&state.parquet_home, p);
            let hive = cfg.get("hive_partitioning").and_then(|v| v.as_bool()).unwrap_or(true);
            Ok((format!("SELECT * FROM read_parquet('{}', hive_partitioning={})", abs, hive), "duckdb"))
        }
        "duckdb_query" => {
            let sql = cfg.get("sql").and_then(|v| v.as_str()).unwrap_or("");
            if sql.is_empty() { return Err(err(400, "duckdb_query source requires config.sql")); }
            Ok((sql.to_string(), "duckdb"))
        }
        "pg_query" => {
            let sql = cfg.get("sql").and_then(|v| v.as_str()).unwrap_or("");
            if sql.is_empty() { return Err(err(400, "pg_query source requires config.sql")); }
            Ok((sql.to_string(), "pg"))
        }
        "ch_query" => {
            let sql = cfg.get("sql").and_then(|v| v.as_str()).unwrap_or("");
            if sql.is_empty() { return Err(err(400, "ch_query source requires config.sql")); }
            Ok((sql.to_string(), "clickhouse"))
        }
        "bq_query" => Err(err(501, "bq_query source is not yet implemented")),
        "pipeline" => Err(err(400, "'pipeline' source kind has been removed; pipelines now live in shared_pipelines and dataview sources point at the pipeline's output (duckdb_table or parquet_glob).")),
        _ => Err(err(400, &format!("unknown source type '{}'", kind))),
    }
}

// ----------------------------------------------------------------------------
// Introspection
// ----------------------------------------------------------------------------

fn introspect_duckdb_blocking(duckdb_path: &str, select_sql: &str) -> Result<Vec<Value>, String> {
    let conn = duckdb::Connection::open(duckdb_path)
        .map_err(|e| format!("DuckDB open: {}", e))?;
    let view_sql = format!("CREATE OR REPLACE TEMP VIEW _ds_intro AS {}", select_sql);
    conn.execute(&view_sql, []).map_err(|e| format!("Schema introspection failed: {}", e))?;

    let mut stmt = conn.prepare("DESCRIBE _ds_intro")
        .map_err(|e| format!("DESCRIBE prepare failed: {}", e))?;
    let mut out: Vec<Value> = Vec::new();
    let rows = stmt.query_map([], |row| {
        Ok(json!({
            "name": row.get::<_, String>(0).unwrap_or_default(),
            "type": row.get::<_, String>(1).unwrap_or_default(),
        }))
    }).map_err(|e| format!("DESCRIBE query failed: {}", e))?;
    for r in rows.flatten() { out.push(r); }
    Ok(out)
}

async fn introspect_pg(dsn: &str, sql: &str) -> Result<Vec<Value>, String> {
    let (client, conn) = tokio_postgres::connect(dsn, tokio_postgres::NoTls).await
        .map_err(|e| format!("Connection failed: {}", e))?;
    tokio::spawn(async move { conn.await.ok(); });

    // Prepare gives us column names + PG type oids without executing the query.
    let stmt = client.prepare(sql).await
        .map_err(|e| format!("Prepare failed: {}", e))?;

    let cols: Vec<Value> = stmt.columns().iter().map(|c| {
        json!({ "name": c.name(), "type": c.type_().name() })
    }).collect();
    Ok(cols)
}

/// Resolve a `connections.config` JSON for a ClickHouse connection.
/// Returns 400 when the id is missing, not found, or not a CH type.
fn ch_connection_config(
    state: &AppState,
    connection_ref: &str,
) -> Result<Value, (axum::http::StatusCode, Json<Value>)> {
    let row = state.db.query_one(
        "SELECT * FROM connections WHERE id = ?1",
        &[&connection_ref as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, &format!("connection '{connection_ref}' not found")))?;
    let conn_type = row.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if conn_type != "clickhouse" {
        return Err(err(400, &format!(
            "connection '{connection_ref}' has type='{conn_type}', expected 'clickhouse'"
        )));
    }
    row.get("config").cloned()
        .ok_or_else(|| err(500, "connection has no config blob"))
}

/// One-off CH probe using FORMAT JSON so we can read `meta` (column
/// list with declared types). Avoids running the data query — `LIMIT 0`
/// in the wrapped SQL keeps the scan empty.
async fn ch_introspect_columns(
    conn: &crate::clickhouse::ChConnection,
    sql_with_format: &str,
) -> Result<Vec<Value>, String> {
    use std::time::Duration;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(conn.query_timeout_seconds))
        .build()
        .map_err(|e| format!("clickhouse client build: {e}"))?;
    let scheme = if conn.ssl { "https" } else { "http" };
    let url = format!("{scheme}://{}:{}/", conn.host, conn.port);
    let resp = client
        .post(&url)
        .basic_auth(&conn.username, Some(&conn.password))
        .body(sql_with_format.to_string())
        .send()
        .await
        .map_err(|e| format!("ClickHouse request: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("ClickHouse response read: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "ClickHouse HTTP {}: {}",
            status.as_u16(),
            body.chars().take(500).collect::<String>()
        ));
    }
    let parsed: Value = serde_json::from_str(&body)
        .map_err(|e| format!("parse ClickHouse FORMAT JSON: {e}"))?;
    let meta = parsed.get("meta").and_then(|v| v.as_array()).cloned()
        .unwrap_or_default();
    Ok(meta.into_iter().map(|m| {
        json!({
            "name": m.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            "type": m.get("type").and_then(|v| v.as_str()).unwrap_or(""),
        })
    }).collect())
}

/// Pick a PG DSN based on connection_ref (data_source id) or the default.
fn resolve_pg_dsn_for(state: &AppState, connection_ref: Option<&str>) -> Option<String> {
    let sources = state.db.query("SELECT * FROM connections", &[]).ok()?;
    let is_pg = |c: &&Value| {
        let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
        t == "pg" || t == "postgres"
    };
    let is_default = |c: &&Value| c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
    let conn = if let Some(id) = connection_ref.filter(|s| !s.is_empty()) {
        sources.iter().find(|c| c.get("id").and_then(|v| v.as_str()) == Some(id))?
    } else {
        sources.iter().find(|c| is_pg(c) && is_default(c))
            .or_else(|| sources.iter().find(is_pg))?
    };
    crate::query::pg_conn_str(conn.get("config")?)
}

/// POST /api/dataviews/{id}/introspect-source
/// Returns the column projection of the source as `{ columns: [{name, type}] }`.
pub async fn introspect_source(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let (_row, source) = load_source(&state, &dv_id)?;
    let kind = source.get("type").and_then(|v| v.as_str()).unwrap_or("duckdb_table").to_string();

    // For PG, do an async prepare. For everything else, route through DuckDB.
    if kind == "pg_query" {
        let cfg = source.get("config").cloned().unwrap_or(json!({}));
        let sql = cfg.get("sql").and_then(|v| v.as_str()).unwrap_or("");
        if sql.is_empty() { return Err(err(400, "pg_query source requires config.sql")); }
        let conn_ref = cfg.get("connection_ref").and_then(|v| v.as_str());
        let dsn = resolve_pg_dsn_for(&state, conn_ref)
            .ok_or_else(|| err(400, "no PG data_source available (mark one as default for type=pg)"))?;
        let columns = introspect_pg(&dsn, sql).await
            .map_err(|e| err(400, &format!("PG introspection failed: {}", e)))?;
        return Ok(Json(json!({
            "source": source,
            "columns": columns,
            "engine": "pg",
        })));
    }

    if kind == "bq_query" {
        return Err(err(501, "bq_query introspection is not yet implemented"));
    }

    if kind == "ch_query" {
        // ClickHouse introspection: run the SQL with `LIMIT 0` so CH
        // returns header rows without scanning data, then derive
        // column names from the first row's keys. The CH HTTP
        // endpoint doesn't ship a separate prepare/describe call;
        // this is the standard workaround.
        let cfg = source.get("config").cloned().unwrap_or(json!({}));
        let sql = cfg.get("sql").and_then(|v| v.as_str()).unwrap_or("");
        if sql.is_empty() { return Err(err(400, "ch_query source requires config.sql")); }
        let conn_ref = cfg.get("connection_ref").and_then(|v| v.as_str())
            .ok_or_else(|| err(400, "ch_query source requires config.connection_ref"))?;
        let conn_cfg = ch_connection_config(&state, conn_ref)?;
        let conn = crate::clickhouse::ChConnection::from_config(&conn_cfg)
            .map_err(|e| err(400, &format!("ClickHouse config invalid: {e:#}")))?;
        // Wrap as subquery + LIMIT 0 so we get column metadata only.
        let probe = format!("SELECT * FROM ({sql}) LIMIT 0");
        // LIMIT 0 returns no rows but column headers are not part of
        // JSONEachRow. Use FORMAT JSON to get a "meta" array instead.
        let probe = format!("{probe} FORMAT JSON");
        // ChConnection writes JSONEachRow by default; we need JSON
        // for the meta. Route a one-off raw HTTP request here.
        let columns = ch_introspect_columns(&conn, &probe).await
            .map_err(|e| err(400, &format!("ClickHouse introspection failed: {e:#}")))?;
        return Ok(Json(json!({
            "source": source,
            "columns": columns,
            "engine": "clickhouse",
        })));
    }

    if kind == "graph" {
        // Read columns from the live in-memory graph projection. No
        // DuckDB hit. Falls back to a generic ARTICLE schema if the
        // graph hasn't been built yet — keeps introspect from
        // erroring before pl_build_article_graph runs.
        use crate::graph::legacy::projection::{columns_for, parse_node_kind};
        use crate::graph::legacy::NodeKind as GraphNodeKind;
        let cfg = source.get("config").cloned().unwrap_or(json!({}));
        let node_kind_str = cfg
            .get("node_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("ARTICLE");
        let node_kind = parse_node_kind(node_kind_str).unwrap_or(GraphNodeKind::Article);
        let columns: Vec<Value> = columns_for(node_kind)
            .into_iter()
            .map(|c| json!({ "name": c.name, "type": c.r#type }))
            .collect();
        return Ok(Json(json!({
            "source": source,
            "columns": columns,
            "engine": "graph",
        })));
    }

    let (sql, _engine) = dataview_select_sql(&state, &source)?;
    let duckdb_path = state.duckdb_path.clone();
    let columns = tokio::task::spawn_blocking(move || introspect_duckdb_blocking(&duckdb_path, &sql))
        .await
        .map_err(|e| err(500, &format!("task join: {}", e)))?
        .map_err(|e| err(400, &e))?;
    Ok(Json(json!({
        "source": source,
        "columns": columns,
        "engine": "duckdb",
    })))
}

// ----------------------------------------------------------------------------
// Read path
// ----------------------------------------------------------------------------

#[derive(serde::Deserialize, Default)]
struct DataReq {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    sort_col: Option<String>,
    #[serde(default)]
    sort_dir: Option<String>,
    /// Frontend-controlled flag: when true the server skips the
    /// `SELECT COUNT(*)` companion query and returns `total = 0` (the
    /// client is expected to keep its prior count). Used on page/sort
    /// changes — paging through the same query can't change total, so
    /// re-running the count is pure waste. Filter changes still send
    /// `false` (default) because filters can change cardinality.
    #[serde(default)]
    skip_total: bool,
    // Cross-filter selections from the Live View dropdowns. Resolved
    // against the article graph via `cross_filter::resolver::apply_filters`
    // (article_graph source only — pg/duckdb paths ignore for now).
    #[serde(default)]
    filters: Vec<crate::cross_filter::model::Filter>,
    // Optional Phase-1 exception rule narrowing (stockout, overstock, ...).
    // Article candidate set = intersection of cross-filter results AND
    // articles firing any of these rules.
    #[serde(default)]
    rules: Vec<String>,
    // Optional node_kind override for article_graph-backed DataViews.
    // When present, overrides `source.config.node_kind` so the same
    // DataView can be projected at different hierarchy levels (L0..L5,
    // ARTICLE, PRODUCT_CODE) without re-binding the source.
    #[serde(default)]
    node_kind: Option<String>,
    // Server-side GROUP BY. Column names validated against the dataview's
    // declared columns. Paired with `aggregates` below — at least one
    // aggregate is required when group_by is non-empty.
    #[serde(default)]
    group_by: Vec<String>,
    // Aggregate specs applied alongside `group_by`. Each spec produces one
    // additional output column with name = `alias` (or `<column>_<op>` if
    // omitted). Operators: sum, avg, count, count_distinct, min, max.
    #[serde(default)]
    aggregates: Vec<AggregateSpec>,
    // Post-group filters. Each clause names an alias (a group_by column or
    // an aggregate's output name on this same request) and is rendered into
    // a HAVING fragment. Empty when group_by is empty.
    #[serde(default)]
    having: Vec<HavingClause>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct AggregateSpec {
    column: String,
    op: String,
    #[serde(default)]
    alias: Option<String>,
}
fn default_limit() -> i64 { 100 }

/// Translate `req.filters` into a safely-inlined `WHERE` clause. Returns the
/// clause (already prefixed with `WHERE `, or empty string if no filters
/// applied) on success. Errors when the request references an unknown column
/// or supplies a non-numeric value to a numeric operator.
///
/// Why inline-and-escape instead of placeholder-bind? The outer SELECT path
/// runs through different DB drivers (tokio_postgres for pg_query, duckdb-rs
/// for duckdb_table/duckdb_query) whose parameter-binding APIs differ.
/// Inline literals + strict validation (column allowlist + per-operator
/// value parsing) keeps the call sites uniform without sacrificing safety:
/// every value is either a parsed number or a single-quote-escaped string,
/// and attribute names are checked against the dataview's declared columns.
fn build_where_clause(
    filters: &[crate::cross_filter::model::Filter],
    allowed_columns: &[(String, String)],
) -> Result<String, String> {
    let allowed: std::collections::HashSet<&str> =
        allowed_columns.iter().map(|(n, _)| n.as_str()).collect();
    let mut parts: Vec<String> = Vec::new();
    for f in filters {
        let col = f.attribute_name.trim();
        if col.is_empty() { continue; }
        // Identifier safety: alphanumeric + underscore, must start with letter.
        // Belt + suspenders alongside the allowlist check below.
        if !col.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
           || !col.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(format!("invalid column identifier in filter: {col:?}"));
        }
        if !allowed.contains(col) {
            return Err(format!(
                "filter references column {col:?} not declared on this dataview"
            ));
        }
        let values = f.values.as_strings();
        if values.is_empty() { continue; }

        let quoted_ident = format!("\"{}\"", col);
        let frag = render_predicate(&quoted_ident, f.operator, &values, col)?;
        parts.push(frag);
    }
    if parts.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!(" WHERE {}", parts.join(" AND ")))
    }
}

/// Single-quote SQL string literal: wraps in `'…'` and doubles embedded `'`.
/// Strips NUL bytes (Postgres rejects them in text literals anyway).
fn sql_quote_literal(v: &str) -> String {
    let cleaned: String = v.chars().filter(|c| *c != '\0').collect();
    format!("'{}'", cleaned.replace('\'', "''"))
}

/// Translate a single (operand, operator, values) into a SQL predicate
/// fragment. Shared between filter (WHERE) and having (HAVING) builders;
/// `operand_sql` is whatever LHS the caller wants — a quoted column for
/// filters, an inlined `OP("col")` expression for HAVING on aggregates.
fn render_predicate(
    operand_sql: &str,
    op: crate::cross_filter::model::Operator,
    values: &[String],
    label: &str,
) -> Result<String, String> {
    use crate::cross_filter::model::Operator;
    Ok(match op {
        Operator::In | Operator::InEq => {
            let list: Vec<String> = values.iter().map(|v| sql_quote_literal(v)).collect();
            format!("{} IN ({})", operand_sql, list.join(", "))
        }
        Operator::NotIn => {
            let list: Vec<String> = values.iter().map(|v| sql_quote_literal(v)).collect();
            format!("{} NOT IN ({})", operand_sql, list.join(", "))
        }
        Operator::Eq | Operator::IsEq => {
            format!("{} = {}", operand_sql, sql_quote_literal(&values[0]))
        }
        Operator::Ne | Operator::IsNot => {
            format!("{} <> {}", operand_sql, sql_quote_literal(&values[0]))
        }
        Operator::Like => format!("{} LIKE {}", operand_sql, sql_quote_literal(&values[0])),
        Operator::ILike => format!("{} ILIKE {}", operand_sql, sql_quote_literal(&values[0])),
        Operator::Gt | Operator::Lt | Operator::Gte | Operator::Lte => {
            let n: f64 = values[0].parse().map_err(|_| format!(
                "predicate on {label:?} uses operator {op:?} but value {:?} is not a number",
                values[0]
            ))?;
            let op_str = match op {
                Operator::Gt => ">",
                Operator::Lt => "<",
                Operator::Gte => ">=",
                Operator::Lte => "<=",
                _ => unreachable!(),
            };
            format!("{} {} {}", operand_sql, op_str, n)
        }
        Operator::Between => {
            if values.len() != 2 {
                return Err(format!(
                    "predicate on {label:?} uses `between` but expected 2 values, got {}",
                    values.len()
                ));
            }
            let lo: f64 = values[0].parse().map_err(|_| format!(
                "between low bound on {label:?} {:?} is not a number", values[0]
            ))?;
            let hi: f64 = values[1].parse().map_err(|_| format!(
                "between high bound on {label:?} {:?} is not a number", values[1]
            ))?;
            format!("{} BETWEEN {} AND {}", operand_sql, lo, hi)
        }
    })
}

/// Result of parsing `req.group_by` + `req.aggregates`:
///   projection      — the SELECT list ("*" when no aggregation, else "col1, col2, SUM(metric) AS alias")
///   group_cols_csv  — quoted CSV of group-by columns (used to build the nested SELECT in count_sql)
///   group_by_clause — " GROUP BY \"col1\", \"col2\"" or empty
///   alias_to_expr   — alias → underlying SQL expression. Used to resolve HAVING clauses without
///                     forcing the inner SELECT to project the aggregate.
#[derive(Default, Clone)]
struct AggregateClauses {
    projection: String,
    group_cols_csv: String,
    group_by_clause: String,
    alias_to_expr: std::collections::HashMap<String, String>,
}

fn build_aggregate_clauses(
    group_by: &[String],
    aggregates: &[AggregateSpec],
    allowed_columns: &[(String, String)],
) -> Result<AggregateClauses, String> {
    if group_by.is_empty() && aggregates.is_empty() {
        return Ok(AggregateClauses {
            projection: "*".to_string(),
            ..Default::default()
        });
    }
    if group_by.is_empty() {
        return Err("aggregates require at least one group_by column".to_string());
    }

    let allowed: std::collections::HashSet<&str> =
        allowed_columns.iter().map(|(n, _)| n.as_str()).collect();
    let type_of: std::collections::HashMap<&str, &str> =
        allowed_columns.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();

    fn is_safe_ident(s: &str) -> bool {
        !s.is_empty()
            && s.chars().next().map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    let mut select_parts: Vec<String> = Vec::new();
    let mut group_cols: Vec<String> = Vec::new();
    let mut alias_to_expr: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for c in group_by {
        if !is_safe_ident(c) {
            return Err(format!("invalid group_by column identifier: {c:?}"));
        }
        if !allowed.contains(c.as_str()) {
            return Err(format!(
                "group_by references column {c:?} not declared on this dataview"
            ));
        }
        let q = format!("\"{}\"", c);
        select_parts.push(q.clone());
        group_cols.push(q.clone());
        // The group_by column itself is a valid HAVING operand.
        alias_to_expr.insert(c.clone(), q);
    }

    for a in aggregates {
        // `count` with column="*" is the standard row-count idiom; handle it
        // before the identifier check (which would reject "*").
        let is_count_star = a.op.eq_ignore_ascii_case("count") && a.column == "*";
        let column_render = if is_count_star {
            "*".to_string()
        } else {
            if !is_safe_ident(&a.column) {
                return Err(format!("invalid aggregate column identifier: {:?}", a.column));
            }
            if !allowed.contains(a.column.as_str()) {
                return Err(format!(
                    "aggregate references column {:?} not declared on this dataview",
                    a.column
                ));
            }
            format!("\"{}\"", a.column)
        };
        // Translate op → SQL expression with type-aware handling:
        //   - BOOLEAN min/max use BOOL_AND / BOOL_OR (PG's MIN/MAX reject
        //     bool natively; both PG and DuckDB support these aggregates).
        //   - BOOLEAN sum/avg are semantically meaningless on a true/false
        //     column — reject up-front with a clear message.
        //   - count_distinct emits COUNT(DISTINCT col), rejecting '*'.
        let op_lc = a.op.to_ascii_lowercase();
        let col_type_upper = if is_count_star {
            String::new()
        } else {
            type_of.get(a.column.as_str()).copied().unwrap_or("").to_ascii_uppercase()
        };
        let is_bool_col = col_type_upper == "BOOLEAN" || col_type_upper == "BOOL";
        let agg_expr = match (op_lc.as_str(), is_bool_col) {
            ("sum",            true)  => return Err(format!(
                "sum on boolean column {:?} is meaningless; use count + filter instead",
                a.column,
            )),
            ("avg",            true)  => return Err(format!(
                "avg on boolean column {:?} is meaningless; use bool_or/bool_and (min/max) for true/false rollup",
                a.column,
            )),
            ("max",            true)  => format!("BOOL_OR({})", column_render),
            ("min",            true)  => format!("BOOL_AND({})", column_render),
            ("sum",            false) => format!("SUM({})", column_render),
            ("avg",            false) => format!("AVG({})", column_render),
            ("count",          _)     => format!("COUNT({})", column_render),
            ("count_distinct", _)     => {
                if is_count_star {
                    return Err("count_distinct(*) is meaningless; use count(*)".to_string());
                }
                format!("COUNT(DISTINCT {})", column_render)
            }
            ("min",            false) => format!("MIN({})", column_render),
            ("max",            false) => format!("MAX({})", column_render),
            (other, _) => {
                return Err(format!(
                    "unsupported aggregate op {other:?}; allowed: sum|avg|count|count_distinct|min|max"
                ));
            }
        };
        let default_alias_suffix = if op_lc == "count_distinct" { "distinct" } else { op_lc.as_str() };
        let alias = a
            .alias
            .clone()
            .unwrap_or_else(|| format!("{}_{}", a.column, default_alias_suffix));
        // Allow `*_count` even though `*` isn't a valid identifier; canonicalize.
        let alias_sanitized = if alias == "*_count" {
            "count_all".to_string()
        } else {
            alias
        };
        if !is_safe_ident(&alias_sanitized) {
            return Err(format!("invalid aggregate alias: {alias_sanitized:?}"));
        }
        select_parts.push(format!("{} AS \"{}\"", agg_expr, alias_sanitized));
        alias_to_expr.insert(alias_sanitized, agg_expr);
    }

    Ok(AggregateClauses {
        projection: select_parts.join(", "),
        group_cols_csv: group_cols.join(", "),
        group_by_clause: format!(" GROUP BY {}", group_cols.join(", ")),
        alias_to_expr,
    })
}

/// Build a HAVING clause from `req.having`. Each clause names an alias that
/// either matches a group_by column or an aggregate's output name; the alias
/// is resolved back to its underlying SQL expression so HAVING works without
/// requiring the inner SELECT to project the aggregate column directly.
fn build_having_clause(
    having: &[HavingClause],
    ac: &AggregateClauses,
) -> Result<String, String> {
    if having.is_empty() {
        return Ok(String::new());
    }
    if ac.group_by_clause.is_empty() {
        return Err("having clauses require group_by (no aggregation context)".to_string());
    }
    let mut parts: Vec<String> = Vec::new();
    for h in having {
        let alias = h.alias.trim();
        if alias.is_empty() { continue; }
        let expr = ac.alias_to_expr.get(alias).ok_or_else(|| format!(
            "having references alias {alias:?} which is neither a group_by column nor an aggregate alias on this request"
        ))?;
        let values = h.values.as_strings();
        if values.is_empty() { continue; }
        let frag = render_predicate(expr, h.operator, &values, alias)?;
        parts.push(frag);
    }
    if parts.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!(" HAVING {}", parts.join(" AND ")))
    }
}

/// Wire shape of a HAVING clause. Mirrors `cross_filter::model::Filter`
/// except `attribute_name` is replaced with `alias` since HAVING references
/// the post-projection name.
#[derive(Clone, Debug, serde::Deserialize)]
struct HavingClause {
    alias: String,
    operator: crate::cross_filter::model::Operator,
    values: crate::cross_filter::model::Values,
}

fn build_outer_select(
    inner_sql: &str,
    req: &DataReq,
    where_clause: &str,
    ac: &AggregateClauses,
    having_clause: &str,
) -> String {
    let mut out = format!(
        "SELECT {} FROM ({}) AS _q{}{}{}",
        ac.projection, inner_sql, where_clause, ac.group_by_clause, having_clause
    );
    if let Some(c) = req.sort_col.as_ref().filter(|s| !s.is_empty()) {
        let dir = req.sort_dir.as_deref().unwrap_or("ASC").to_uppercase();
        let dir = if dir == "DESC" { "DESC" } else { "ASC" };
        // Quote the identifier defensively. (Single-tenant DB, advisory only.)
        out.push_str(&format!(" ORDER BY \"{}\" {}", c.replace('"', "\"\""), dir));
    }
    if req.limit > 0 {
        out.push_str(&format!(" LIMIT {} OFFSET {}", req.limit, req.offset.max(0)));
    }
    out
}

fn count_sql(inner_sql: &str, where_clause: &str, ac: &AggregateClauses, having_clause: &str) -> String {
    if ac.group_by_clause.is_empty() {
        format!("SELECT COUNT(*) FROM ({}) AS _q{}", inner_sql, where_clause)
    } else {
        // For aggregated reads, "total" is the number of GROUPS post-HAVING.
        // The inner SELECT projects only the group-by cols; HAVING references
        // aggregate expressions inlined (not aliases) so this works without
        // duplicating the aggregate projection here.
        format!(
            "SELECT COUNT(*) FROM (SELECT {} FROM ({}) AS _src{}{}{}) AS _g",
            ac.group_cols_csv, inner_sql, where_clause, ac.group_by_clause, having_clause
        )
    }
}

/// Walk a tokio_postgres::Error's source chain and stitch the messages
/// together so the caller sees the real cause. The default Display on
/// tokio_postgres::Error truncates to "db error" which hides everything
/// useful (column type mismatch, missing column, etc.).
fn fmt_pg_err(prefix: &str, e: &tokio_postgres::Error) -> String {
    use std::error::Error;
    let mut parts = vec![format!("{}: {}", prefix, e)];
    if let Some(db_err) = e.as_db_error() {
        parts.push(format!(
            "code={} severity={} message={}",
            db_err.code().code(),
            db_err.severity(),
            db_err.message(),
        ));
    }
    let mut src: Option<&(dyn Error + 'static)> = e.source();
    while let Some(inner) = src {
        parts.push(format!("caused by: {}", inner));
        src = inner.source();
    }
    parts.join(" | ")
}

async fn data_pg(dsn: &str, inner_sql: &str, req: &DataReq, where_clause: &str, ac: &AggregateClauses, having_clause: &str) -> Result<Value, String> {
    let (client, conn) = tokio_postgres::connect(dsn, tokio_postgres::NoTls).await
        .map_err(|e| format!("Connection failed: {}", e))?;
    tokio::spawn(async move { conn.await.ok(); });

    client.execute("BEGIN", &[]).await.ok();
    client.execute("SET TRANSACTION READ ONLY", &[]).await.ok();
    client.execute("SET LOCAL statement_timeout = '30s'", &[]).await.ok();

    let outer = build_outer_select(inner_sql, req, where_clause, ac, having_clause);
    let data_rows = client.query(&outer, &[]).await
        .map_err(|e| fmt_pg_err("Query failed", &e))?;

    let col_names: Vec<String> = if !data_rows.is_empty() {
        data_rows[0].columns().iter().map(|c| c.name().to_string()).collect()
    } else { vec![] };

    let mut rows = Vec::new();
    for r in &data_rows {
        let mut obj = serde_json::Map::new();
        for (i, name) in col_names.iter().enumerate() {
            obj.insert(name.clone(), crate::query::pg_val(r, i));
        }
        rows.push(Value::Object(obj));
    }

    // See note in data_duckdb_blocking — skip the count when the client
    // says it already has it (page/sort change can't change cardinality).
    let total: i64 = if req.skip_total {
        0
    } else {
        client.query_one(&count_sql(inner_sql, where_clause, ac, having_clause), &[]).await
            .map(|r| r.get(0))
            .unwrap_or_else(|_| rows.len() as i64)
    };

    client.execute("COMMIT", &[]).await.ok();

    Ok(json!({
        "rows": rows,
        "total": total,
        "columns": col_names.iter().map(|n| json!({"name": n})).collect::<Vec<_>>(),
        "sql": outer,
    }))
}

fn data_duckdb_blocking(duckdb_path: &str, inner_sql: &str, req: &DataReq, where_clause: &str, ac: &AggregateClauses, having_clause: &str) -> Result<Value, String> {
    let t_open = Instant::now();
    let conn = duckdb::Connection::open(duckdb_path)
        .map_err(|e| format!("DuckDB open: {}", e))?;
    let open_ms = t_open.elapsed().as_millis() as u64;
    let outer = build_outer_select(inner_sql, req, where_clause, ac, having_clause);

    let t_query = Instant::now();
    let mut stmt = conn.prepare(&outer).map_err(|e| format!("prepare: {}", e))?;
    let frames = stmt.query_arrow(duckdb::params![]).map_err(|e| format!("query: {}", e))?;

    let mut col_names: Vec<String> = Vec::new();
    let mut rows: Vec<Value> = Vec::new();
    for batch in frames {
        if col_names.is_empty() {
            col_names = batch.schema().fields().iter().map(|f| f.name().clone()).collect();
        }
        for row_idx in 0..batch.num_rows() {
            let mut obj = serde_json::Map::new();
            for (col_idx, name) in col_names.iter().enumerate() {
                obj.insert(name.clone(), crate::query::arrow_to_json(batch.column(col_idx), row_idx));
            }
            rows.push(Value::Object(obj));
        }
    }
    let query_ms = t_query.elapsed().as_millis() as u64;

    // Total via separate COUNT(*) query — full-scan over the inner SQL.
    // Skipped when the client signals it already has the count (page/sort
    // change, where total can't have changed).
    let t_count = Instant::now();
    let total: i64 = if req.skip_total {
        0
    } else {
        conn.query_row(&count_sql(inner_sql, where_clause, ac, having_clause), [], |r| r.get::<_, i64>(0))
            .unwrap_or(rows.len() as i64)
    };
    let count_ms = if req.skip_total { 0 } else { t_count.elapsed().as_millis() as u64 };

    tracing::info!(
        target: "live_view_timing",
        kind = "duckdb",
        open_ms,
        query_ms,
        count_ms,
        skip_total = req.skip_total,
        rows = rows.len(),
        total,
        sort_col = ?req.sort_col,
        offset = req.offset,
        "live_view duckdb timing",
    );

    Ok(json!({
        "rows": rows,
        "total": total,
        "columns": col_names.iter().map(|n| json!({"name": n})).collect::<Vec<_>>(),
        "sql": outer,
    }))
}

/// POST /api/dataviews/{id}/data — { limit, offset, sort_col, sort_dir }
/// Returns rows + columns + total for the dataview's source.
pub async fn data(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    // Strict-parse the request body. Previous `unwrap_or_default()`
    // silently swallowed malformed shapes (e.g. the agent sending
    // `aggregates: [{count: {alias: "..."}}]` instead of
    // `aggregates: [{column, op, alias}]`). That dropped the model's
    // entire request, ran the dataview at default settings, and
    // returned a paginated firehose the model couldn't interpret.
    // 400-ing the bad request gives the agent a clear correction
    // signal and prevents the multi-turn flailing that's been
    // stalling chat sessions.
    let req: DataReq = match serde_json::from_value(body.clone()) {
        Ok(r) => r,
        Err(e) => {
            return Err(err(
                400,
                &format!(
                    "request body doesn't match DataReq schema: {e}. \
                     Each aggregate spec must be {{column, op, alias?}} \
                     (ops: sum/avg/count/count_distinct/min/max). \
                     Each filter must be {{column, value|values, op?}} \
                     or {{attribute_name, values, operator}}.",
                ),
            ));
        }
    };
    let (row, source) = load_source(&state, &dv_id)?;
    let kind = source.get("type").and_then(|v| v.as_str()).unwrap_or("duckdb_table").to_string();
    let t = Instant::now();

    // For pg_query / duckdb_* sources, translate req.filters / group_by /
    // aggregates into safely-inlined SQL clauses. article_graph has its own
    // resolution pipeline below; uam_* and bq_query ignore these.
    let (where_clause, ac, having_clause) = if kind == "pg_query" || kind == "duckdb_table" || kind == "duckdb_query" {
        // Pull (name, type) pairs so the aggregate builder can do type-aware
        // op dispatch (e.g. BOOL_OR/BOOL_AND for max/min on BOOLEAN cols).
        // Filters and HAVING only need names; aggregates need both.
        let cols: Vec<(String, String)> = row.get("columns")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter()
                .filter_map(|c| {
                    let name = c.get("name").and_then(|v| v.as_str())?.to_string();
                    let ty = c.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    Some((name, ty))
                })
                .collect())
            .unwrap_or_default();
        let w = build_where_clause(&req.filters, &cols)
            .map_err(|e| err(400, &format!("filter rejected: {}", e)))?;
        let agg = build_aggregate_clauses(&req.group_by, &req.aggregates, &cols)
            .map_err(|e| err(400, &format!("aggregate rejected: {}", e)))?;
        let h = build_having_clause(&req.having, &agg)
            .map_err(|e| err(400, &format!("having rejected: {}", e)))?;
        (w, agg, h)
    } else {
        (String::new(), AggregateClauses::default(), String::new())
    };

    let result = if kind == "pg_query" {
        let cfg = source.get("config").cloned().unwrap_or(json!({}));
        let sql = cfg.get("sql").and_then(|v| v.as_str()).unwrap_or("");
        if sql.is_empty() { return Err(err(400, "pg_query source requires config.sql")); }
        let dsn = resolve_pg_dsn_for(&state, cfg.get("connection_ref").and_then(|v| v.as_str()))
            .ok_or_else(|| err(400, "no PG data_source available (mark one as default for type=pg)"))?;
        data_pg(&dsn, sql, &req, &where_clause, &ac, &having_clause).await.map_err(|e| err(400, &e))?
    } else if kind == "ch_query" {
        let cfg = source.get("config").cloned().unwrap_or(json!({}));
        let sql = cfg.get("sql").and_then(|v| v.as_str()).unwrap_or("");
        if sql.is_empty() { return Err(err(400, "ch_query source requires config.sql")); }
        let conn_ref = cfg.get("connection_ref").and_then(|v| v.as_str())
            .ok_or_else(|| err(400, "ch_query source requires config.connection_ref"))?;
        let conn_cfg = ch_connection_config(&state, conn_ref)?;
        let conn = crate::clickhouse::ChConnection::from_config(&conn_cfg)
            .map_err(|e| err(400, &format!("ClickHouse config invalid: {e:#}")))?;
        // CH MVP doesn't apply server-side filter/group_by/having yet
        // — we just paginate the SQL result client-side. The wrap
        // pattern mirrors pg/duckdb so eventual WHERE/HAVING support
        // slots in by piping `where_clause` / `ac` / `having_clause`
        // into the outer SELECT.
        let outer = format!(
            "SELECT * FROM ({sql}) ORDER BY {sort_clause} LIMIT {limit} OFFSET {offset}",
            sort_clause = req.sort_col.as_deref()
                .map(|c| format!("{c} {}", req.sort_dir.as_deref().unwrap_or("ASC")))
                .unwrap_or_else(|| "1".to_string()),
            limit = if req.limit > 0 { req.limit } else { 100 },
            offset = req.offset.max(0),
        );
        let result = crate::clickhouse::query_exec(&conn, &outer).await
            .map_err(|e| err(400, &format!("ClickHouse query failed: {e:#}")))?;
        let columns: Vec<Value> = result.rows.first()
            .and_then(|r| r.as_object())
            .map(|m| m.keys().map(|k| json!({"name": k})).collect())
            .unwrap_or_default();
        let row_len = result.rows.len() as i64;
        let total: i64 = if req.skip_total {
            0
        } else {
            // Wrap the inner SQL in COUNT(*) so we get cardinality
            // without re-issuing the data query.
            let count_sql = format!("SELECT count() AS c FROM ({sql})");
            crate::clickhouse::query_exec(&conn, &count_sql).await
                .ok()
                .and_then(|rs| rs.rows.first()
                    .and_then(|r| r.get("c"))
                    .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64))))
                .unwrap_or(row_len)
        };
        let elapsed_ms = t.elapsed().as_millis() as i64;
        json!({
            "rows": result.rows,
            "total": total,
            "columns": columns,
            "sql": outer,
            "duration_ms": elapsed_ms,
            "client_ms": result.client_ms,
            "server_ms": result.server_ms,
            "read_rows": result.read_rows,
            "read_bytes": result.read_bytes,
        })
    } else if kind == "bq_query" {
        return Err(err(501, "bq_query data path is not yet implemented"));
    } else if kind == "graph" {
        // Article-graph DataView: read directly from the in-memory
        // ArcSwap snapshot of `state.legacy_graph`. No SQL. Use
        // `project_page`: sort node IDs on the graph (cheap
        // primitives, no JSON), paginate IDs, project only the page.
        // Per-request work is O(N log N) sort + O(limit) project,
        // independent of total node count for the JSON allocation
        // cost.
        //
        // When the metadata-driven `graph::Graph` grows the
        // projection methods this path uses, swap the source of
        // truth to `state.graphs[default_id]` and retire
        // `state.legacy_graph` (see docs/v1-cleanup-todo.md).
        use crate::graph::legacy::projection::{columns_for, parse_node_kind, project_page};
        use crate::graph::legacy::NodeKind as GraphNodeKind;
        let cfg = source.get("config").cloned().unwrap_or(json!({}));

        // `config.graph_id` selects the metadata-driven snapshot in
        // `state.graphs`. When set we route through `graph::project::
        // project_page` instead of the legacy projection; when absent
        // we fall back to the hand-coded `state.legacy_graph` below.
        // This is the transitional discriminator — once every
        // article-graph DataView is migrated to set graph_id, the
        // legacy branch (and `state.legacy_graph`) can be retired.
        if let Some(graph_id) = cfg.get("graph_id").and_then(|v| v.as_str()) {
            let slot = {
                let graphs = state.graphs.read().await;
                graphs.get(graph_id).cloned()
            };
            let slot = slot.ok_or_else(|| err(404, &format!(
                "graph `{graph_id}` not built — call POST /api/graphs/{graph_id}/build"
            )))?;
            let snapshot = slot.load();
            let g = snapshot.as_ref().ok_or_else(|| err(404, &format!(
                "graph `{graph_id}` snapshot empty"
            )))?;

            // Resolve the target kind. Precedence:
            //   1. explicit req.node_kind
            //   2. first group_by column (mapped 1:1 to a kind name —
            //      Decision 28 says level ids are globally unique, so
            //      a bare column like "brand" resolves uniquely)
            //   3. source.config.node_kind
            //   4. default "article"
            // group_by-driven kind switching is what lets the agent
            // ask "lw_revenue grouped by brand" on the article-grain
            // DataView and have the server project the brand grain
            // (whose nodes already carry the rolled-up sum thanks to
            // cross_edge_rollup).
            let kind_name = req
                .node_kind
                .clone()
                .or_else(|| req.group_by.first().cloned())
                .or_else(|| cfg.get("node_kind").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_else(|| "article".to_string());

            // Resolve filter sets per filter. Each filter narrows the
            // candidate node id set of the target kind. Filter column
            // is interpreted as a kind name — Decision 28 makes level
            // ids globally unique so a bare `brand` / `l1` / `article`
            // resolves uniquely. Then for each filter value:
            //   - find the filter-kind node by name
            //   - map that node to the current kind via
            //     `nodes_of_kind_under(filter_node, current_kind)`,
            //     which walks the spine subtree first (same hierarchy)
            //     and falls back to cross-edge traversal (different
            //     hierarchy)
            // The intersection of those per-filter id sets is the
            // candidate id set for projection.
            let target_kind_id = g
                .kinds
                .id_of(&kind_name)
                .ok_or_else(|| err(400, &format!("unknown kind `{kind_name}`")))?;
            let mut candidate_ids: Option<std::collections::HashSet<crate::graph::graph::NodeId>> = None;
            for f in &req.filters {
                let filter_kind_id = match g.kinds.id_of(&f.attribute_name) {
                    Some(k) => k,
                    // Unknown column — could be a metric, post-group alias,
                    // or just a typo. Skipping it is safer than 400'ing the
                    // whole request; the model can react to the unfiltered
                    // total returned.
                    None => continue,
                };
                let names = f.values.as_strings();
                if names.is_empty() {
                    continue;
                }
                let mut this_filter_ids: std::collections::HashSet<crate::graph::graph::NodeId> =
                    std::collections::HashSet::new();
                for name in names {
                    if let Some(filter_node) =
                        g.find_by_name(filter_kind_id, &name)
                    {
                        for nid in nodes_of_kind_from(g, filter_node, target_kind_id) {
                            this_filter_ids.insert(nid);
                        }
                    }
                }
                candidate_ids = Some(match candidate_ids.take() {
                    None => this_filter_ids,
                    Some(prev) => prev.intersection(&this_filter_ids).copied().collect(),
                });
            }

            // Project EVERY row of the kind (or the candidate subset
            // when filters narrowed it) so we can apply aggregate
            // aliases, sort by any column, and paginate. project_page
            // only sorts by node name and doesn't honor filters, so
            // we pull the full slice ourselves when a filter is set.
            let mut all_rows: Vec<Value> = if let Some(ids) = candidate_ids {
                let opts = crate::graph::project::ProjectionOptions {
                    include_ancestors: true,
                    include_metrics: true,
                    include_cross_edges: true,
                };
                ids.into_iter()
                    .map(|nid| crate::graph::project::flatten_row_public(crate::graph::project::project(g, nid, &opts)))
                    .collect()
            } else {
                let (rows, _total) = crate::graph::project::project_page(
                    g,
                    &kind_name,
                    None,
                    0,
                    0,
                )
                .map_err(|e| err(400, &e))?;
                rows
            };

            // Aggregate aliasing: graph metrics are PRE-AGGREGATED at
            // the row's kind by the rollup pass, so an aggregate spec
            // like `sum(lw_revenue) as total_lw_revenue` reduces to
            // "copy `lw_revenue` to the alias name". `op` is currently
            // accepted-and-ignored for sum/avg/min/max because the
            // value-as-stored matches the operator declared in the
            // graph spec; mismatched ops would need a true regroup,
            // which graph-source DataViews don't carry. Count/
            // count_distinct fall through (the row count IS the
            // distinct count of the kind).
            for spec in &req.aggregates {
                let alias = spec
                    .alias
                    .clone()
                    .unwrap_or_else(|| format!("{}_{}", spec.column, spec.op));
                for row in all_rows.iter_mut() {
                    let Some(obj) = row.as_object_mut() else { continue };
                    if spec.op.eq_ignore_ascii_case("count")
                        || spec.op.eq_ignore_ascii_case("count_distinct")
                    {
                        obj.insert(alias.clone(), json!(1));
                    } else if let Some(v) = obj.get(&spec.column).cloned() {
                        obj.insert(alias.clone(), v);
                    }
                }
            }

            // Sort by an arbitrary column (metric, ancestor, name).
            // Numbers compare numerically; strings lexically; nulls sort
            // last. `req.sort_dir` "desc"/"DESC" reverses order.
            if let Some(sort_col) = req.sort_col.as_deref() {
                let desc = req
                    .sort_dir
                    .as_deref()
                    .map(|d| d.eq_ignore_ascii_case("desc"))
                    .unwrap_or(false);
                all_rows.sort_by(|a, b| {
                    use std::cmp::Ordering;
                    let av = a.get(sort_col);
                    let bv = b.get(sort_col);
                    let raw = match (av, bv) {
                        (None | Some(Value::Null), None | Some(Value::Null)) => Ordering::Equal,
                        (None | Some(Value::Null), _) => Ordering::Greater,
                        (_, None | Some(Value::Null)) => Ordering::Less,
                        (Some(Value::Number(x)), Some(Value::Number(y))) => x
                            .as_f64()
                            .unwrap_or(0.0)
                            .partial_cmp(&y.as_f64().unwrap_or(0.0))
                            .unwrap_or(Ordering::Equal),
                        (Some(x), Some(y)) => x.to_string().cmp(&y.to_string()),
                    };
                    if desc { raw.reverse() } else { raw }
                });
            }

            let total = all_rows.len() as i64;
            let off = req.offset.max(0) as usize;
            let lim = if req.limit > 0 { req.limit as usize } else { usize::MAX };
            let page: Vec<Value> = all_rows.into_iter().skip(off).take(lim).collect();

            let columns: Vec<Value> = page
                .first()
                .and_then(|r| r.as_object())
                .map(|obj| {
                    obj.keys()
                        .map(|k| json!({ "name": k }))
                        .collect()
                })
                .unwrap_or_default();
            return Ok(Json(json!({
                "rows": page,
                "total": total,
                "columns": columns,
                "sql": format!(
                    "(graph `{graph_id}`; kind={kind_name}; nodes={total}; group_by={:?}; sort={:?})",
                    req.group_by, req.sort_col
                ),
                "duration_ms": t.elapsed().as_millis() as i64,
            })));
        }

        // Request body wins over source.config so the Live View can
        // pivot the same DataView between hierarchy levels without
        // mutating the source binding.
        let node_kind_str = req
            .node_kind
            .as_deref()
            .or_else(|| cfg.get("node_kind").and_then(|v| v.as_str()))
            .unwrap_or("ARTICLE")
            .to_string();
        let node_kind = parse_node_kind(&node_kind_str).unwrap_or(GraphNodeKind::Article);
        let graph_arc = state.legacy_graph.load_full().ok_or_else(|| {
            err(503, "article_graph not built yet — run pipeline pl_build_article_graph first")
        })?;
        // Snapshot the live RuleSet so the article projection can fill in
        // the rcl-resolved end-user columns (store_groups / dc_rule /
        // min_stock / max_stock / wos / aps). When the rcl service isn't
        // running, we project without the rcl columns (cells emit null).
        let ruleset_snapshot = {
            let guard = state.rcl_store.read().await;
            guard.as_ref().map(|store| store.snapshot())
        };
        // Resolve the candidate article set:
        //   filters → cross_filter (dropdown intersections).
        //   rules → exception predicates (stockout, overstock, ...).
        //   intersection if both. None = unfiltered.
        // The same `build_alive_set` powers the traversal path so the
        // tree view and the data view stay in sync.
        let candidate_articles: Option<std::collections::BTreeSet<crate::graph::legacy::NodeId>> =
            if req.filters.is_empty() && req.rules.is_empty() {
                None
            } else if req.rules.is_empty() {
                // Filters only — keep the existing fast path.
                Some(crate::cross_filter::resolver::apply_filters(
                    &graph_arc,
                    &req.filters,
                    None,
                ))
            } else {
                // Rules (with or without filters): use the unified resolver
                // which AND-composes both. It returns the alive-ancestor
                // set; we want only the article NodeIds for project_page.
                let alive = crate::graph::legacy::traverse::build_alive_set(
                    &graph_arc,
                    &req.filters,
                    &req.rules,
                    ruleset_snapshot.as_deref(),
                );
                alive.map(|set| {
                    set.into_iter()
                        .filter(|id| {
                            matches!(
                                graph_arc.node(*id).kind,
                                crate::graph::legacy::NodeKind::Article
                            )
                        })
                        .collect::<std::collections::BTreeSet<_>>()
                })
            };
        let has_store_filter = matches!(node_kind, GraphNodeKind::Article)
            && req
                .filters
                .iter()
                .any(|f| matches!(f.dimension, Some(crate::cross_filter::model::Dimension::Store)));

        // Sort columns we know how to push to DuckDB store-scoped sort path.
        let metric_sort_col = req.sort_col.as_deref().filter(|c| {
            matches!(
                *c,
                "lw_units" | "lw_revenue" | "lw_margin" | "oh" | "oo" | "it" | "reserve_quantity"
            )
        });
        let push_sort_to_duckdb = has_store_filter && metric_sort_col.is_some();

        let t_project = Instant::now();
        let (mut page, total): (Vec<Value>, i64) = if push_sort_to_duckdb {
            // Store-filter + metric sort: push the sort to DuckDB so the
            // page is ordered by store-scoped values, not graph all-store
            // totals. Returns (article, metrics) in sort order; project
            // each through the graph for hierarchy/RCL/etc.
            let candidate_names: Vec<String> = match candidate_articles.as_ref() {
                Some(set) => set
                    .iter()
                    .map(|id| graph_arc.get_str(graph_arc.node(*id).name).to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                None => graph_arc
                    .by_kind[GraphNodeKind::Article.idx()]
                    .iter()
                    .filter_map(|(name, _)| {
                        let s = graph_arc.get_str(*name);
                        if s.is_empty() { None } else { Some(s.to_string()) }
                    })
                    .collect(),
            };
            match store_scoped_sorted_page(
                &state.duckdb_path,
                &req.filters,
                &candidate_names,
                metric_sort_col.unwrap(),
                req.sort_dir.as_deref().unwrap_or("ASC"),
                req.limit,
                req.offset,
            ) {
                Ok((sorted, total_n)) => {
                    // Project each article via the graph for the non-metric
                    // columns (article name, hierarchy, brand, RCL fields),
                    // then overlay the DuckDB-computed store-scoped metrics.
                    let mut rows: Vec<Value> = Vec::with_capacity(sorted.len());
                    let selected_count = sorted
                        .first()
                        .map(|(_, m)| m.selected_stores)
                        .unwrap_or(0);
                    for (article, m) in sorted {
                        let Some(name_id) = graph_arc
                            .by_kind[GraphNodeKind::Article.idx()]
                            .iter()
                            .find(|(n, _)| graph_arc.get_str(**n) == article)
                            .map(|(_, id)| *id)
                        else { continue };
                        let Some(mut row) = crate::graph::legacy::projection::project_single(
                            &graph_arc,
                            GraphNodeKind::Article,
                            name_id,
                            ruleset_snapshot.as_deref(),
                        ) else { continue };
                        if let Some(obj) = row.as_object_mut() {
                            obj.insert("lw_units".into(), json!(m.lw_units));
                            obj.insert("lw_revenue".into(), json!(m.lw_revenue));
                            obj.insert("lw_margin".into(), json!(m.lw_margin));
                            obj.insert("oh".into(), json!(m.oh));
                            obj.insert("oo".into(), json!(m.oo));
                            obj.insert("it".into(), json!(m.it));
                            obj.insert("reserve_quantity".into(), json!(m.reserve_quantity));
                            if selected_count > 0 {
                                let pct = (m.in_stock_count as f64 / selected_count as f64 * 100.0).round() / 100.0;
                                obj.insert("in_stock_perc".into(), json!(pct));
                            }
                        }
                        rows.push(row);
                    }
                    (rows, total_n)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "store-scoped sort path failed; falling back to graph sort + overlay"
                    );
                    // Fall back to the standard project_page + overlay flow.
                    let (page, total) = project_page(
                        &graph_arc,
                        node_kind,
                        req.sort_col.as_deref(),
                        req.sort_dir.as_deref(),
                        req.limit,
                        req.offset,
                        candidate_articles.as_ref(),
                        ruleset_snapshot.as_deref(),
                    );
                    (page, total)
                }
            }
        } else {
            project_page(
                &graph_arc,
                node_kind,
                req.sort_col.as_deref(),
                req.sort_dir.as_deref(),
                req.limit,
                req.offset,
                candidate_articles.as_ref(),
                ruleset_snapshot.as_deref(),
            )
        };
        let project_ms = t_project.elapsed().as_millis() as u64;

        // Store-scoped metric overlay. When the request carries any store-
        // dimension filter on an Article view, the rolled-up values on each
        // Article node (sums across all stores / DCs) are wrong for the
        // filtered context. Resolve filters → store_codes → array indices,
        // then overwrite lw_units / lw_revenue / lw_margin / in_stock_perc
        // (per-store) and oh / oo / it / reserve_quantity (per-DC, via
        // store→DC mapping) on each row.
        //
        // The push-the-sort path above already ran the equivalent overlay
        // inline; this branch handles store-filtered reads sorted by
        // non-metric columns (article name, brand, l*_name, etc.).
        let mut store_overlay_ms: Option<u64> = None;
        if has_store_filter && !push_sort_to_duckdb {
            let t_overlay = Instant::now();
            let articles: Vec<String> = page
                .iter()
                .filter_map(|r| r.get("article").and_then(|v| v.as_str()).map(String::from))
                .collect();
            if !articles.is_empty() {
                match store_scoped_metrics(&state.duckdb_path, &req.filters, &articles) {
                    Ok(metrics_by_article) => {
                        let selected_count = metrics_by_article
                            .values()
                            .next()
                            .map(|m| m.selected_stores)
                            .unwrap_or(0);
                        for row in page.iter_mut() {
                            let Some(article) = row
                                .get("article")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                            else { continue };
                            let Some(obj) = row.as_object_mut() else { continue };
                            if let Some(m) = metrics_by_article.get(&article) {
                                obj.insert("lw_units".into(), json!(m.lw_units));
                                obj.insert("lw_revenue".into(), json!(m.lw_revenue));
                                obj.insert("lw_margin".into(), json!(m.lw_margin));
                                obj.insert("oh".into(), json!(m.oh));
                                obj.insert("oo".into(), json!(m.oo));
                                obj.insert("it".into(), json!(m.it));
                                obj.insert("reserve_quantity".into(), json!(m.reserve_quantity));
                                if selected_count > 0 {
                                    let pct = (m.in_stock_count as f64 / selected_count as f64 * 100.0).round() / 100.0;
                                    obj.insert("in_stock_perc".into(), json!(pct));
                                }
                            } else {
                                // Article missing from both per-store and
                                // per-DC tables — zero out so the user
                                // doesn't see leftover all-store totals.
                                obj.insert("lw_units".into(), json!(0));
                                obj.insert("lw_revenue".into(), json!(0));
                                obj.insert("lw_margin".into(), json!(0));
                                obj.insert("oh".into(), json!(0));
                                obj.insert("oo".into(), json!(0));
                                obj.insert("it".into(), json!(0));
                                obj.insert("reserve_quantity".into(), json!(0));
                                obj.insert("in_stock_perc".into(), json!(0.0));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "store-scoped metric overlay failed; rendering with all-store totals"
                        );
                    }
                }
            }
            store_overlay_ms = Some(t_overlay.elapsed().as_millis() as u64);
        }

        tracing::info!(
            target: "live_view_timing",
            kind = "graph",
            node_kind = %node_kind_str,
            project_ms,
            store_overlay_ms,
            rows = page.len(),
            total,
            sort_col = ?req.sort_col,
            offset = req.offset,
            filters = req.filters.len(),
            ruleset = ruleset_snapshot.is_some(),
            "live_view article_graph timing",
        );
        let columns: Vec<Value> = columns_for(node_kind)
            .into_iter()
            .map(|c| json!({ "name": c.name }))
            .collect();
        json!({
            "rows": page,
            "total": total,
            "columns": columns,
            "sql": format!(
                "(article_graph in-memory; node_kind={}, version={}, filters={})",
                node_kind_str, graph_arc.graph_version, req.filters.len()
            ),
        })
    } else {
        let (inner_sql, _engine) = dataview_select_sql(&state, &source)?;
        let duckdb_path = state.duckdb_path.clone();
        let inner_owned = inner_sql.clone();
        let where_owned = where_clause.clone();
        let having_owned = having_clause.clone();
        let ac_owned = ac.clone();
        let req_owned = DataReq {
            limit: req.limit, offset: req.offset,
            sort_col: req.sort_col.clone(), sort_dir: req.sort_dir.clone(),
            skip_total: req.skip_total,
            // For DuckDB sources, req.filters + group_by + aggregates + having
            // are translated above into `where_owned`, `ac_owned`, and
            // `having_owned` (passed positionally). Article-graph-style
            // filter resolution doesn't apply here.
            filters: req.filters.clone(),
            // node_kind override only applies to article_graph sources.
            node_kind: req.node_kind.clone(),
            // Rules likewise only narrow the article_graph path; ignored
            // by pg/duckdb sources today.
            rules: req.rules.clone(),
            // Passed through for completeness; data_duckdb_blocking uses the
            // pre-built `ac_owned` for SQL generation rather than re-parsing.
            group_by: req.group_by.clone(),
            aggregates: req.aggregates.clone(),
            having: req.having.clone(),
        };
        tokio::task::spawn_blocking(move || data_duckdb_blocking(&duckdb_path, &inner_owned, &req_owned, &where_owned, &ac_owned, &having_owned))
            .await
            .map_err(|e| err(500, &format!("task join: {}", e)))?
            .map_err(|e| err(400, &e))?
    };

    let elapsed = t.elapsed().as_millis() as i64;
    let mut out = result;
    if let Some(o) = out.as_object_mut() { o.insert("duration_ms".to_string(), json!(elapsed)); }
    Ok(Json(out))
}

/// In-process pagination/sort over a small Vec<Value>. Mirrors the
/// shape used by the article_graph read path; lifted here so the
/// `uam_entitlement` path doesn't pull in
/// `crate::graph::legacy::projection`.
fn simple_paginate(
    rows: Vec<Value>,
    sort_col: Option<&str>,
    sort_dir: Option<&str>,
    limit: i64,
    offset: i64,
) -> Vec<Value> {
    let mut out = rows;
    if let Some(col) = sort_col.filter(|s| !s.is_empty()) {
        let desc = sort_dir.map(|s| s.eq_ignore_ascii_case("DESC")).unwrap_or(false);
        out.sort_by(|a, b| {
            let av = a.get(col);
            let bv = b.get(col);
            let ord = match (av, bv) {
                (Some(Value::Number(an)), Some(Value::Number(bn))) => an
                    .as_f64()
                    .partial_cmp(&bn.as_f64())
                    .unwrap_or(std::cmp::Ordering::Equal),
                (Some(a), Some(b)) => a.to_string().cmp(&b.to_string()),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            };
            if desc { ord.reverse() } else { ord }
        });
    }
    let off = offset.max(0) as usize;
    let lim = if limit > 0 { limit as usize } else { out.len() };
    out.into_iter().skip(off).take(lim).collect()
}


/// Store-scoped metrics for one article, summed across the user-selected
/// stores (txs side) and across the DCs serving those stores (inventory
/// side). Returned by [`store_scoped_metrics`].
struct StoreScopedMetrics {
    // Txs (per-store granular).
    lw_units: i64,
    lw_revenue: i64,
    lw_margin: i64,
    in_stock_count: i64,
    /// Number of stores in the selected set (constant across all articles).
    /// Used by the caller to compute `in_stock_perc = in_stock_count / selected_stores * 100`.
    selected_stores: i64,
    // Inventory (per-DC granular). Stores → DCs via raw_store_dc_mapping.
    oh: i64,
    oo: i64,
    it: i64,
    reserve_quantity: i64,
}

/// Build the WHERE clause for resolving store-dim filters → store_codes via
/// raw_store_channels. Returns the SQL fragment plus a count of recognized
/// filters; if no recognized filters are present the caller should bail
/// (no store filter = no overlay needed).
fn build_store_filter_where(filters: &[crate::cross_filter::model::Filter]) -> (String, usize) {
    use crate::cross_filter::model::{Dimension, Operator};
    let mut where_clauses: Vec<String> = vec!["active = true".into()];
    let mut applied = 0usize;
    for f in filters {
        if !matches!(f.dimension, Some(Dimension::Store)) {
            continue;
        }
        if !matches!(f.operator, Operator::In | Operator::InEq | Operator::Eq) {
            continue;
        }
        let col = match f.attribute_name.as_str() {
            "s0_name" | "s1_name" | "s2_name" | "store_code" | "channel" => f.attribute_name.as_str(),
            other => {
                tracing::warn!(
                    "store-scoped metrics: unsupported store attribute {} (skipping)",
                    other
                );
                continue;
            }
        };
        let needles: Vec<String> = f.values.as_strings();
        if needles.is_empty() {
            continue;
        }
        let in_list: Vec<String> = needles
            .iter()
            .map(|v| format!("'{}'", v.replace('\'', "''")))
            .collect();
        where_clauses.push(format!("{} IN ({})", col, in_list.join(",")));
        applied += 1;
    }
    (where_clauses.join(" AND "), applied)
}

/// Resolve store-dim filters → store_codes → indices in `asv2_store_index`
/// (txs) and `asv2_dc_index` (inventory, via the store→DC mapping) → array
/// reductions over `asv2_aid_per_store` and `asv2_inventory_per_dc` for the
/// given page articles.
///
/// One DuckDB query with two parallel index lookups (stores and DCs). Returns
/// one row per article present in either table; articles missing from both
/// are omitted (caller zeroes them).
///
/// Build-time invariants (enforced by `check_aid_per_store_alignment`):
///   - every metric array in asv2_aid_per_store has length |asv2_store_index|
///   - every metric array in asv2_inventory_per_dc has length |asv2_dc_index|
fn store_scoped_metrics(
    duckdb_path: &str,
    filters: &[crate::cross_filter::model::Filter],
    articles: &[String],
) -> Result<std::collections::HashMap<String, StoreScopedMetrics>, String> {
    if articles.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let (store_where, applied) = build_store_filter_where(filters);
    if applied == 0 {
        return Ok(std::collections::HashMap::new());
    }

    let articles_in: Vec<String> = articles
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', "''")))
        .collect();

    let sql = format!(
        r#"WITH selected_stores AS (
            SELECT store_code FROM raw_store_channels WHERE {}
          ),
          selected_dcs AS (
            SELECT DISTINCT m.dc_code
            FROM raw_store_dc_mapping m JOIN selected_stores s USING (store_code)
          ),
          idx_s AS (
            SELECT array_agg(si.idx + 1) AS positions, COUNT(*) AS n
            FROM asv2_store_index si JOIN selected_stores ss USING (store_code)
          ),
          idx_d AS (
            SELECT array_agg(di.idx + 1) AS positions
            FROM asv2_dc_index di JOIN selected_dcs sd USING (dc_code)
          ),
          art AS (SELECT UNNEST([{}]) AS article)
          SELECT
            art.article,
            COALESCE(list_sum(list_select(a.lw_units,   idx_s.positions)), 0)::BIGINT AS lw_units,
            COALESCE(list_sum(list_select(a.lw_revenue, idx_s.positions)), 0)::BIGINT AS lw_revenue,
            COALESCE(list_sum(list_select(a.lw_margin,  idx_s.positions)), 0)::BIGINT AS lw_margin,
            COALESCE(list_sum(list_select(a.in_stock,   idx_s.positions)), 0)::BIGINT AS in_stock_count,
            COALESCE(idx_s.n, 0)                                                       AS selected_stores,
            COALESCE(list_sum(list_select(b.oh, idx_d.positions)), 0)::BIGINT AS oh,
            COALESCE(list_sum(list_select(b.oo, idx_d.positions)), 0)::BIGINT AS oo,
            COALESCE(list_sum(list_select(b.it, idx_d.positions)), 0)::BIGINT AS it,
            COALESCE(list_sum(list_select(b.reserve_quantity, idx_d.positions)), 0)::BIGINT AS reserve_quantity
          FROM art
          LEFT JOIN asv2_aid_per_store    a USING (article)
          LEFT JOIN asv2_inventory_per_dc b USING (article)
          CROSS JOIN idx_s
          CROSS JOIN idx_d"#,
        store_where,
        articles_in.join(",")
    );

    let conn = duckdb::Connection::open(duckdb_path).map_err(|e| format!("open duckdb: {}", e))?;
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {}", e))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1).unwrap_or(0),
                r.get::<_, i64>(2).unwrap_or(0),
                r.get::<_, i64>(3).unwrap_or(0),
                r.get::<_, i64>(4).unwrap_or(0),
                r.get::<_, i64>(5).unwrap_or(0),
                r.get::<_, i64>(6).unwrap_or(0),
                r.get::<_, i64>(7).unwrap_or(0),
                r.get::<_, i64>(8).unwrap_or(0),
                r.get::<_, i64>(9).unwrap_or(0),
            ))
        })
        .map_err(|e| format!("query: {}", e))?;

    let mut out = std::collections::HashMap::new();
    for row in rows.flatten() {
        out.insert(
            row.0,
            StoreScopedMetrics {
                lw_units: row.1,
                lw_revenue: row.2,
                lw_margin: row.3,
                in_stock_count: row.4,
                selected_stores: row.5,
                oh: row.6,
                oo: row.7,
                it: row.8,
                reserve_quantity: row.9,
            },
        );
    }
    Ok(out)
}

/// Push-the-sort path: when `sort_col` is a metric column AND store filters
/// are present, the page's article ordering must use store-scoped values, not
/// the graph's all-store rolled-up values.
///
/// Runs the same store-scoped aggregation as [`store_scoped_metrics`] but
/// over the *full candidate set*, sorts in DuckDB by the requested metric,
/// then LIMIT/OFFSETs to the page. Returns the resulting articles in sort
/// order, each with its store-scoped metrics, plus the total candidate
/// count.
///
/// Caller projects each returned article through the graph for hierarchy +
/// brand + RCL columns and overlays the metrics from this function.
fn store_scoped_sorted_page(
    duckdb_path: &str,
    filters: &[crate::cross_filter::model::Filter],
    candidates: &[String],
    sort_col: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
) -> Result<(Vec<(String, StoreScopedMetrics)>, i64), String> {
    if candidates.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let (store_where, applied) = build_store_filter_where(filters);
    if applied == 0 {
        return Ok((Vec::new(), 0));
    }

    let metric_expr = match sort_col {
        "lw_units" => "lw_units",
        "lw_revenue" => "lw_revenue",
        "lw_margin" => "lw_margin",
        "oh" => "oh",
        "oo" => "oo",
        "it" => "it",
        "reserve_quantity" => "reserve_quantity",
        _ => return Err(format!("store_scoped_sorted_page: unsupported sort_col '{}'", sort_col)),
    };
    let dir = if sort_dir.eq_ignore_ascii_case("DESC") { "DESC" } else { "ASC" };

    let candidates_in: Vec<String> = candidates
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', "''")))
        .collect();
    let lim = if limit > 0 { limit } else { i64::MAX };
    let off = offset.max(0);

    let sql = format!(
        r#"WITH selected_stores AS (
            SELECT store_code FROM raw_store_channels WHERE {where_clause}
          ),
          selected_dcs AS (
            SELECT DISTINCT m.dc_code
            FROM raw_store_dc_mapping m JOIN selected_stores s USING (store_code)
          ),
          idx_s AS (
            SELECT array_agg(si.idx + 1) AS positions, COUNT(*) AS n
            FROM asv2_store_index si JOIN selected_stores ss USING (store_code)
          ),
          idx_d AS (
            SELECT array_agg(di.idx + 1) AS positions
            FROM asv2_dc_index di JOIN selected_dcs sd USING (dc_code)
          ),
          art AS (SELECT UNNEST([{cands}]) AS article),
          per_article AS (
            SELECT
              art.article,
              COALESCE(list_sum(list_select(a.lw_units,   idx_s.positions)), 0)::BIGINT AS lw_units,
              COALESCE(list_sum(list_select(a.lw_revenue, idx_s.positions)), 0)::BIGINT AS lw_revenue,
              COALESCE(list_sum(list_select(a.lw_margin,  idx_s.positions)), 0)::BIGINT AS lw_margin,
              COALESCE(list_sum(list_select(a.in_stock,   idx_s.positions)), 0)::BIGINT AS in_stock_count,
              COALESCE(idx_s.n, 0)                                                       AS selected_stores,
              COALESCE(list_sum(list_select(b.oh, idx_d.positions)), 0)::BIGINT AS oh,
              COALESCE(list_sum(list_select(b.oo, idx_d.positions)), 0)::BIGINT AS oo,
              COALESCE(list_sum(list_select(b.it, idx_d.positions)), 0)::BIGINT AS it,
              COALESCE(list_sum(list_select(b.reserve_quantity, idx_d.positions)), 0)::BIGINT AS reserve_quantity
            FROM art
            LEFT JOIN asv2_aid_per_store    a USING (article)
            LEFT JOIN asv2_inventory_per_dc b USING (article)
            CROSS JOIN idx_s
            CROSS JOIN idx_d
          )
          SELECT *, (SELECT COUNT(*) FROM per_article) AS total
          FROM per_article
          ORDER BY {metric} {dir}
          LIMIT {lim} OFFSET {off}"#,
        where_clause = store_where,
        cands = candidates_in.join(","),
        metric = metric_expr,
        dir = dir,
        lim = lim,
        off = off,
    );

    let conn = duckdb::Connection::open(duckdb_path).map_err(|e| format!("open duckdb: {}", e))?;
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {}", e))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1).unwrap_or(0),
                r.get::<_, i64>(2).unwrap_or(0),
                r.get::<_, i64>(3).unwrap_or(0),
                r.get::<_, i64>(4).unwrap_or(0),
                r.get::<_, i64>(5).unwrap_or(0),
                r.get::<_, i64>(6).unwrap_or(0),
                r.get::<_, i64>(7).unwrap_or(0),
                r.get::<_, i64>(8).unwrap_or(0),
                r.get::<_, i64>(9).unwrap_or(0),
                r.get::<_, i64>(10).unwrap_or(0),
            ))
        })
        .map_err(|e| format!("query: {}", e))?;

    let mut out: Vec<(String, StoreScopedMetrics)> = Vec::new();
    let mut total: i64 = 0;
    for row in rows.flatten() {
        total = row.10;
        out.push((
            row.0,
            StoreScopedMetrics {
                lw_units: row.1,
                lw_revenue: row.2,
                lw_margin: row.3,
                in_stock_count: row.4,
                selected_stores: row.5,
                oh: row.6,
                oo: row.7,
                it: row.8,
                reserve_quantity: row.9,
            },
        ));
    }
    Ok((out, total))
}


/// Render a dataview page from a v2 `Graph` snapshot. Returns the same
/// `{rows, total, columns, sql}` shape as the v1 article_graph branch
/// so existing UI clients consume it unchanged.
///
/// Scope (first cut):
///   - Supports `node_kind = ARTICLE` (the bealls case). Other kinds
///     return an error; extension is mechanical — add a branch in
///     `project_row_v2` per kind, mirroring v1's `columns_for`.
///   - Cross-filter `filters` and exception `rules` are accepted but
///     not yet applied here. They have working implementations in
///     `graph::cross_filter` and `graph::exception::alive_set`;
///     wiring is a TODO once the dataview render path is verified.
///   - RCL-resolved columns (store_groups / dc_rule / min_stock /
///     max_stock / wos / aps) emit null until rcl_store is plumbed
///     through. The metric columns (oh / oo / it / lw_units / …) come
///     from `Node.metrics` directly via the `name → slot` map.
async fn graph_dataview_page(
    graph: &std::sync::Arc<crate::graph::Graph>,
    cfg: &Value,
    req: &DataReq,
    ruleset: Option<std::sync::Arc<rcl::RuleSet>>,
) -> Result<Value, (axum::http::StatusCode, Json<Value>)> {
    use crate::graph::exception::MetricLookup;
    let node_kind_str = req
        .node_kind
        .as_deref()
        .or_else(|| cfg.get("node_kind").and_then(|v| v.as_str()))
        .unwrap_or("ARTICLE")
        .to_lowercase();
    let kind_id = match graph.kinds.id_of(&node_kind_str) {
        Some(k) => k,
        None => {
            return Err(err(
                400,
                &format!("v2 graph has no kind named `{node_kind_str}`"),
            ));
        }
    };

    // Column inventory. v1 had a 12-arm match on NodeKind; this first
    // cut covers `article` (the bealls live-view default). Adding
    // other kinds is a straightforward extension when the dataview
    // catalog needs them.
    let columns_def: Vec<(&'static str, &'static str)> = match node_kind_str.as_str() {
        "article" => vec![
            ("article", "VARCHAR"),
            ("l0_name", "VARCHAR"),
            ("l1_name", "VARCHAR"),
            ("l2_name", "VARCHAR"),
            ("l3_name", "VARCHAR"),
            ("l4_name", "VARCHAR"),
            ("l5_name", "VARCHAR"),
            ("brand", "VARCHAR"),
            ("channel", "VARCHAR"),
            ("store_groups", "VARCHAR"),
            ("dc_rule", "VARCHAR"),
            ("min_stock", "NUMERIC"),
            ("max_stock", "NUMERIC"),
            ("wos", "NUMERIC"),
            ("aps", "NUMERIC"),
            ("oh", "BIGINT"),
            ("oo", "BIGINT"),
            ("it", "BIGINT"),
            ("reserve_quantity", "BIGINT"),
            ("allocated_units", "BIGINT"),
            ("lw_units", "BIGINT"),
            ("lw_revenue", "BIGINT"),
            ("lw_margin", "BIGINT"),
        ],
        other => {
            return Err(err(
                501,
                &format!("graph dataview: kind `{other}` not yet supported (article only in this first cut)"),
            ));
        }
    };
    let columns_json: Vec<Value> = columns_def
        .iter()
        .map(|(n, t)| json!({ "name": n, "type": t }))
        .collect();

    // Collect every non-empty-name node of `kind_id` and project each.
    // The metric lookup is built once per request and reused across
    // every row, matching the cost profile of the v1 path.
    // Apply cross-filter + exception-rule narrowing BEFORE entering
    // the blocking task — both are fast (rayon-parallel over a 46K
    // candidate set), and pre-resolving makes the inner ids-collect
    // O(matching) instead of O(total + filter).
    let candidate_set: Option<std::collections::HashSet<crate::graph::graph::NodeId>> = {
        use crate::graph::cross_filter::{FilterCriterion, apply_filters};
        use crate::graph::exception::{Rule, alive_set};
        let mut criteria: Vec<FilterCriterion> = req
            .filters
            .iter()
            .map(FilterCriterion::from)
            .collect();
        for c in &mut criteria {
            c.attribute_name = crate::handlers::normalize_bealls_attribute(&c.attribute_name);
        }
        let parsed_rules: Vec<Rule> = req
            .rules
            .iter()
            .filter_map(|s| Rule::from_wire(s))
            .collect();
        if criteria.is_empty() && parsed_rules.is_empty() {
            None
        } else if parsed_rules.is_empty() {
            // Filters-only fast path: skip alive_set's ancestor walk
            // (the dataview consumes only target-kind ids, no tree
            // pruning to feed).
            Some(
                apply_filters(graph, kind_id, &criteria, None)
                    .into_iter()
                    .collect(),
            )
        } else {
            let rs_ref = ruleset.as_deref();
            let alive = alive_set(graph, kind_id, &criteria, &parsed_rules, None, rs_ref);
            alive.map(|set| {
                set.into_iter()
                    .filter(|id| graph.node(*id).kind == kind_id)
                    .collect()
            })
        }
    };

    let lookup = MetricLookup::build(graph);
    let empty = crate::graph::graph::StrId(0);
    let graph_for_blocking = graph.clone();
    let lookup_for_blocking = lookup;
    let ruleset_for_blocking = ruleset.clone();
    let candidate_for_blocking = candidate_set;
    let req_limit = req.limit.max(0) as usize;
    let req_offset = req.offset.max(0) as usize;
    let req_sort_col = req.sort_col.clone();
    let req_sort_dir = req.sort_dir.clone();
    let skip_total = req.skip_total;

    let (rows, total) = tokio::task::spawn_blocking(move || -> (Vec<Value>, i64) {
        // Pre-collect ids of the target kind, narrowed by the
        // candidate set when filters/rules were supplied. Either
        // path filters out the empty-name sentinel to match v1.
        let mut ids: Vec<crate::graph::graph::NodeId> = match candidate_for_blocking {
            Some(set) => set
                .into_iter()
                .filter(|id| graph_for_blocking.node(*id).name != empty)
                .collect(),
            None => graph_for_blocking
                .iter_kind(kind_id)
                .filter(|id| graph_for_blocking.node(*id).name != empty)
                .collect(),
        };

        let total_count = ids.len() as i64;

        // Sort. Anything we don't know how to compare leaves the
        // natural NodeId order (cheap fallback so paging stays stable).
        if let Some(col) = req_sort_col.as_deref() {
            let dir_desc = req_sort_dir.as_deref() == Some("desc")
                || req_sort_dir.as_deref() == Some("DESC");
            sort_v2_ids(&graph_for_blocking, &lookup_for_blocking, &mut ids, col, dir_desc);
        }

        // Paginate.
        let start = req_offset.min(ids.len());
        let end = (req_offset.saturating_add(req_limit)).min(ids.len());
        let page_ids = &ids[start..end];

        // Project each id in the page to the v1-compatible row shape.
        let rs_ref: Option<&rcl::RuleSet> = ruleset_for_blocking.as_deref();
        let rows: Vec<Value> = page_ids
            .iter()
            .map(|id| project_row_v2(&graph_for_blocking, &lookup_for_blocking, *id, rs_ref))
            .collect();
        let total_emit = if skip_total { 0 } else { total_count };
        (rows, total_emit)
    })
    .await
    .map_err(|e| err(500, &format!("graph dataview join: {e}")))?;

    Ok(json!({
        "rows": rows,
        "total": total,
        "columns": columns_json,
        "sql": format!(
            "(graph in-memory; kind={node_kind_str}; nodes={}; metrics={})",
            graph.count_kind(kind_id),
            graph.metrics.len()
        ),
    }))
}



/// Resolve the channel name for an article via the
/// `ph_master_channel_bridge` cross-edge. Returns empty string when no
/// channel kind is registered or no edge exists for the article.
/// Mirrors v1's `cross_indices.article_to_channel` lookup but reads
/// from the bridge-registered `CrossEdgeRegistry`.
fn channel_for_article(
    graph: &crate::graph::Graph,
    id: crate::graph::graph::NodeId,
) -> String {
    use crate::graph::graph::CrossEdgeId;
    let Some(channel_kind) = graph.kinds.id_of("channel") else {
        return String::new();
    };
    let article_kind = graph.node(id).kind;
    for (i, meta) in graph.cross_edges.metas.iter().enumerate() {
        let eid = CrossEdgeId(i as u32);
        let idx = graph.cross_edges.get(eid);
        let neighbors = if meta.kind_a == article_kind && meta.kind_b == channel_kind {
            idx.forward.get(&id)
        } else if meta.kind_b == article_kind && meta.kind_a == channel_kind {
            idx.reverse.get(&id)
        } else {
            continue;
        };
        if let Some(ns) = neighbors {
            if let Some(&first) = ns.first() {
                return graph.get_str(graph.node(first).name).to_string();
            }
        }
    }
    String::new()
}

/// Per-node projection in v1's article column shape. When `ruleset`
/// is `Some`, the RCL-resolved columns (`store_groups`, `dc_rule`,
/// `min_stock`, `max_stock`, `wos`, `aps`) get populated via
/// `explain_dc_policy` + `explain_constraints`. When `None`, those
/// columns emit null — matching v1's behavior on rcl-disabled paths.
fn project_row_v2(
    graph: &crate::graph::Graph,
    lookup: &crate::graph::exception::MetricLookup,
    id: crate::graph::graph::NodeId,
    ruleset: Option<&rcl::RuleSet>,
) -> Value {
    let h = crate::graph::rcl::owned_hierarchy_for(graph, id);
    let n = graph.node(id);
    let metric = |name: &str| -> Value {
        match lookup.get(graph, id, name) {
            Some(v) if v.is_finite() => json!(v as i64),
            _ => Value::Null,
        }
    };
    let channel = channel_for_article(graph, id);

    // RCL columns. v1 reads:
    //   - dc_rule + store_groups via DcPolicy.{rule_label,store_groups}
    //   - min_stock + max_stock via ConstraintRow.first()
    //   - wos + aps via ConstraintRow.first() (same row)
    // Same fields here so the wire shape stays invariant.
    let mut store_groups = Value::Null;
    let mut dc_rule = Value::Null;
    let mut min_stock = Value::Null;
    let mut max_stock = Value::Null;
    let mut wos = Value::Null;
    let mut aps = Value::Null;
    if let Some(rules) = ruleset {
        let p = h.borrow();
        // DcPolicy → store_groups + dc_rule, matching v1's
        // `default_store_groups.join(", ")` / `dc_store_rule.clone()`
        // shape exactly so the wire output is invariant.
        if let Some(dc) = crate::graph::rcl::explain_dc_policy(rules, &p) {
            store_groups = json!(dc.policy.default_store_groups.join(", "));
            dc_rule = json!(dc.policy.dc_store_rule.clone());
        }
        // ConstraintRow → min_stock / max_stock / wos / aps, again
        // matching v1's field reads.
        if let Some(c) = crate::graph::rcl::explain_constraints(rules, &p) {
            if let Some(row) = c.rows.first() {
                min_stock = json!(row.min_stock);
                max_stock = json!(row.max_stock);
                wos = json!(row.wos);
                aps = json!(row.aps);
            }
        }
    }

    json!({
        "article":          graph.get_str(n.name),
        "l0_name":          h.l0_name,
        "l1_name":          h.l1_name,
        "l2_name":          h.l2_name,
        "l3_name":          h.l3_name,
        "l4_name":          h.l4_name,
        "l5_name":          h.l5_name,
        "brand":            h.brand,
        "channel":          channel,
        "store_groups":     store_groups,
        "dc_rule":          dc_rule,
        "min_stock":        min_stock,
        "max_stock":        max_stock,
        "wos":              wos,
        "aps":              aps,
        "oh":               metric("oh"),
        "oo":               metric("oo"),
        "it":               metric("it"),
        "reserve_quantity": metric("reserve_quantity"),
        "allocated_units":  metric("allocated_units"),
        "lw_units":         metric("lw_units"),
        "lw_revenue":       metric("lw_revenue"),
        "lw_margin":        metric("lw_margin"),
    })
}

/// In-place sort of node ids by a named column. Unknown columns
/// no-op (caller falls back to insertion order). Numeric metric
/// columns use `MetricLookup::get`; string columns walk
/// `owned_hierarchy_for`.
fn sort_v2_ids(
    graph: &crate::graph::Graph,
    lookup: &crate::graph::exception::MetricLookup,
    ids: &mut Vec<crate::graph::graph::NodeId>,
    col: &str,
    desc: bool,
) {
    // Metric columns first — cheaper path (single slot lookup).
    let metric_names = [
        "oh", "oo", "it", "reserve_quantity", "allocated_units",
        "lw_units", "lw_revenue", "lw_margin",
    ];
    if metric_names.contains(&col) {
        let col_owned = col.to_string();
        ids.sort_by(|a, b| {
            let va = lookup.get(graph, *a, &col_owned).unwrap_or(0.0);
            let vb = lookup.get(graph, *b, &col_owned).unwrap_or(0.0);
            if desc { vb.partial_cmp(&va).unwrap_or(std::cmp::Ordering::Equal) }
            else { va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal) }
        });
        return;
    }
    // String columns — need the owned hierarchy or node name. Build
    // a single ProductHierarchy per id; cheap relative to n log n
    // sort costs at bealls scale (~50K rows).
    let key_of = |id: crate::graph::graph::NodeId| -> String {
        match col {
            "article" => graph.get_str(graph.node(id).name).to_string(),
            "l0_name" | "l1_name" | "l2_name" | "l3_name" | "l4_name" | "l5_name" | "brand" => {
                let h = crate::graph::rcl::owned_hierarchy_for(graph, id);
                match col {
                    "l0_name" => h.l0_name,
                    "l1_name" => h.l1_name,
                    "l2_name" => h.l2_name,
                    "l3_name" => h.l3_name,
                    "l4_name" => h.l4_name,
                    "l5_name" => h.l5_name,
                    "brand" => h.brand,
                    _ => String::new(),
                }
            }
            _ => String::new(), // unknown column — sort stays a no-op
        }
    };
    if col == "article"
        || matches!(col, "l0_name" | "l1_name" | "l2_name" | "l3_name" | "l4_name" | "l5_name" | "brand")
    {
        ids.sort_by(|a, b| {
            let ka = key_of(*a);
            let kb = key_of(*b);
            if desc { kb.cmp(&ka) } else { ka.cmp(&kb) }
        });
    }
}
