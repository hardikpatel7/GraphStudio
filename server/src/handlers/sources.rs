//! Source-unification Phase 2 handler. CRUD for the `sources` table — the
//! kind-discriminated addressing layer that DataViews bind to.
//!
//! Six kinds (see docs/primer.md §3.2):
//!   pg_query | bq_query | duckdb_query | parquet_glob | duckdb_table | cdc_pg
//!
//! Materialize + CDC start/stop live in this file too (Phase 2b).

use axum::{Json, extract::{Path, State}};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Instant;

use crate::AppState;
use super::{err, stringify};

// ── Connection lookup (mirrors query_sources::get_connection_config) ───────

fn get_connection_config(state: &Arc<AppState>, connection_ref: &str) -> Result<Value, (axum::http::StatusCode, Json<Value>)> {
    // Match by exact id first.
    if let Ok(row) = state.db.query_one(
        "SELECT * FROM connections WHERE id = ?1",
        &[&connection_ref as &dyn rusqlite::types::ToSql],
    ) {
        return Ok(row["config"].clone());
    }
    // Fallback: default-marked PG, else first PG.
    if let Ok(rows) = state.db.query("SELECT * FROM connections", &[]) {
        let is_pg = |c: &&Value| {
            let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
            t == "pg" || t == "postgres"
        };
        let is_default = |c: &&Value| c.get("is_default").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
        if let Some(ds) = rows.iter().find(|c| is_pg(c) && is_default(c))
            .or_else(|| rows.iter().find(is_pg))
        {
            if let Some(cfg) = ds.get("config") { return Ok(cfg.clone()); }
        }
    }
    Err(err(404, &format!("Connection '{connection_ref}' not found")))
}

const JSON_FIELDS: &[&str] = &["config", "primary_key"];

const ALLOWED_KINDS: &[&str] = &[
    "pg_query", "bq_query", "duckdb_query", "parquet_glob", "duckdb_table", "cdc_pg",
    // Rows projected from the in-memory graph snapshot. No
    // connection_ref / target_table — the graph is built by
    // `pl_build_article_graph` and lives on `AppState.legacy_graph`.
    // `config.node_kind` (default "ARTICLE") picks which graph nodes
    // to emit.
    //
    // Note: UAM is a policy layer, not a kind. UAM data is
    // materialized to the `uam_summary` DuckDB table at every
    // cold-load; DataViews bound to that data use `kind = duckdb_table`,
    // `target_table = "uam_summary"`.
    "graph",
    // ClickHouse query — `config.sql` against a connection of type
    // "clickhouse". Same shape as `pg_query` / `bq_query` (mechanism
    // = SQL over the connector); the connection's config carries
    // host / port / credentials / TLS / timeout / write-access flag.
    "ch_query",
];

fn log(state: &Arc<AppState>, action: &str, status: &str, message: &str, detail: Option<&str>, duration_ms: Option<i64>) {
    if let Err(e) = state.traces.log_activity(&state.tenant_id, "source", action, status, message, detail, duration_ms) {
        tracing::warn!(error = %e, "Failed to log activity");
    }
}

/// Parse known JSON-text columns back into JSON for the API response.
fn parse_json_fields(mut row: Value) -> Value {
    for field in JSON_FIELDS {
        if let Some(s) = row.get(*field).and_then(|v| v.as_str()) {
            if let Ok(v) = serde_json::from_str::<Value>(s) {
                row.as_object_mut().unwrap().insert(field.to_string(), v);
            }
        }
    }
    row
}

// ── List / Get ──────────────────────────────────────────────────────────────

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    crate::service::sources::list(&state)
        .await
        .map(|rows| Json(Value::Array(rows)))
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    crate::service::sources::describe(&state, &id)
        .await
        .map(Json)
        .map_err(|_| err(404, "Source not found"))
}

// ── Create / Update / Delete ────────────────────────────────────────────────

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("").trim();
    let display_name = body["display_name"].as_str().unwrap_or("").trim();
    let kind = body["kind"].as_str().unwrap_or("");
    if id.is_empty() || display_name.is_empty() {
        return Err(err(400, "id and display_name are required"));
    }
    if !ALLOWED_KINDS.contains(&kind) {
        return Err(err(400, &format!("invalid kind '{kind}' — must be one of {:?}", ALLOWED_KINDS)));
    }

    let connection_ref = body.get("connection_ref").and_then(|v| v.as_str()).map(String::from);
    let config = body.get("config").map(stringify).unwrap_or_else(|| "{}".into());
    let target_table = body.get("target_table").and_then(|v| v.as_str()).map(String::from);
    let primary_key = body.get("primary_key").map(stringify).unwrap_or_else(|| "[]".into());
    let cdc_enabled = body.get("cdc_enabled").and_then(|v| v.as_bool()).unwrap_or(false) as i64;
    let initial_status = match kind {
        "duckdb_table" => "not_yet_populated",
        "cdc_pg" => "not_yet_populated",
        _ => "not_yet_populated",
    };

    state.db.execute(
        "INSERT INTO sources \
            (id, display_name, kind, connection_ref, config, target_table, \
             primary_key, cdc_enabled, status) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        &[
            &id as &dyn rusqlite::types::ToSql,
            &display_name as _,
            &kind as _,
            &connection_ref as _,
            &config as _,
            &target_table as _,
            &primary_key as _,
            &cdc_enabled as _,
            &initial_status as _,
        ],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one(
        "SELECT * FROM sources WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    log(&state, "create", "success", &format!("Created Source '{display_name}' (kind={kind})"), Some(id), Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(parse_json_fields(row))))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets: Vec<&'static str> = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

    // Kind cannot be changed once set (per primer §3.2.3). Reject explicitly.
    if let Some(new_kind) = body.get("kind").and_then(|v| v.as_str()) {
        let current_kind: String = state.db.query_one(
            "SELECT kind FROM sources WHERE id = ?1",
            &[&id as &dyn rusqlite::types::ToSql],
        )
        .ok()
        .and_then(|r| r.get("kind").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_default();
        if !current_kind.is_empty() && current_kind != new_kind {
            return Err(err(400, &format!(
                "Source kind cannot be changed (current='{current_kind}', requested='{new_kind}'). Create a new Source instead."
            )));
        }
    }

    if let Some(v) = body.get("display_name").and_then(|v| v.as_str()) {
        if !v.is_empty() { sets.push("display_name = ?"); vals.push(Box::new(v.to_string())); }
    }
    if let Some(v) = body.get("connection_ref").and_then(|v| v.as_str()) {
        sets.push("connection_ref = ?"); vals.push(Box::new(v.to_string()));
    }
    if body.get("config").is_some() {
        sets.push("config = ?"); vals.push(Box::new(stringify(&body["config"])));
    }
    if let Some(v) = body.get("target_table").and_then(|v| v.as_str()) {
        sets.push("target_table = ?"); vals.push(Box::new(v.to_string()));
    }
    if body.get("primary_key").is_some() {
        sets.push("primary_key = ?"); vals.push(Box::new(stringify(&body["primary_key"])));
    }
    if let Some(v) = body.get("cdc_enabled").and_then(|v| v.as_bool()) {
        sets.push("cdc_enabled = ?"); vals.push(Box::new(v as i64));
    }
    if let Some(v) = body.get("status").and_then(|v| v.as_str()) {
        sets.push("status = ?"); vals.push(Box::new(v.to_string()));
    }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE sources SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter()
        .map(|b| b.as_ref() as &dyn rusqlite::types::ToSql)
        .collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Source not found")); }

    let row = state.db.query_one(
        "SELECT * FROM sources WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    log(&state, "update", "success", &format!("Updated Source '{id}'"), Some(&id), Some(elapsed));
    Ok(Json(parse_json_fields(row)))
}

/// Delete a Source. Blocks if any DataView's `source.config.source_id` matches.
/// (Symmetric across all kinds — see primer §3.2.)
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();

    // Find DataViews that bind to this Source via the new shape
    // ({type:'source', config:{source_id:'...'}}). Stored as text — pattern match
    // is fine for the fields-of-a-JSON-text-column case.
    let needle = format!("\"source_id\":\"{}\"", id);
    let bound_dvs = state.db.query(
        "SELECT id, display_name FROM dataviews WHERE source LIKE ?1",
        &[&format!("%{}%", needle) as &dyn rusqlite::types::ToSql],
    ).unwrap_or_default();

    if !bound_dvs.is_empty() {
        let names: Vec<String> = bound_dvs.iter()
            .filter_map(|r| r.get("display_name").and_then(|v| v.as_str()).map(String::from))
            .collect();
        return Err(err(409, &format!(
            "Source '{id}' is bound by {} DataView(s): [{}]. Rewire or delete those first.",
            bound_dvs.len(), names.join(", ")
        )));
    }

    let n = state.db.execute(
        "DELETE FROM sources WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Source not found")); }

    let elapsed = t.elapsed().as_millis() as i64;
    log(&state, "delete", "success", &format!("Deleted Source '{id}'"), Some(&id), Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}

// ── Materialize ─────────────────────────────────────────────────────────────

/// Materialize a Source's data into its `target_table` in `tenant_data.duckdb`.
/// Supported only for kinds that produce a persistent artifact:
///   - `cdc_pg`        : initial PG COPY → DuckDB. CDC streaming starts via /cdc/start.
///   - `duckdb_table`  : noop here; pipelines populate these. Returns 400 for now.
///   - others          : 400 (live-execution kinds have nothing to materialize).
pub async fn materialize(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t0 = Instant::now();
    let row = state.db.query_one(
        "SELECT * FROM sources WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "Source not found"))?;
    let src = parse_json_fields(row);

    let kind = src["kind"].as_str().unwrap_or("");
    if kind != "cdc_pg" {
        return Err(err(400, &format!(
            "materialize is only supported for cdc_pg Sources today. \
             For duckdb_table Sources, run a Pipeline that targets them. \
             For live-execution kinds (pg_query, bq_query, duckdb_query, parquet_glob) \
             there's nothing to materialize. (kind='{kind}')"
        )));
    }

    let connection_ref = src["connection_ref"].as_str().unwrap_or("").to_string();
    if connection_ref.is_empty() {
        return Err(err(400, "connection_ref is required for cdc_pg materialization"));
    }
    let upstream = src["config"]["upstream_table"].as_str().unwrap_or("").to_string();
    if upstream.is_empty() {
        return Err(err(400, "config.upstream_table is required (e.g. 'inventory_smart.orders')"));
    }
    let target = src["target_table"].as_str().unwrap_or("").to_string();
    if target.is_empty() {
        return Err(err(400, "target_table is required"));
    }

    // Mark populating.
    let _ = state.db.execute(
        "UPDATE sources SET status = 'populating', updated_at = datetime('now') WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    );

    // Resolve PG creds.
    let pg_config = get_connection_config(&state, &connection_ref)?;
    let host = pg_config["host"].as_str().unwrap_or("localhost").to_string();
    let port = pg_config["port"].as_u64().unwrap_or(5432) as u16;
    let user = pg_config["user"].as_str().unwrap_or("postgres").to_string();
    let password = pg_config["password"].as_str().unwrap_or("").to_string();
    let database = pg_config["database"].as_str().unwrap_or("postgres").to_string();
    let pg_dsn = format!("dbname={database} user={user} password={password} host={host} port={port}");

    // Run the COPY in a blocking task — DuckDB conn is !Send.
    let duckdb_path = state.duckdb_path.clone();
    let target_clone = target.clone();
    let upstream_clone = upstream.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        let conn = duckdb::Connection::open(&duckdb_path)
            .map_err(|e| format!("DuckDB open: {e}"))?;
        // ATTACH PG via DuckDB postgres extension. Recreate the target table.
        conn.execute_batch(&format!(
            "INSTALL postgres; LOAD postgres;
             ATTACH '{pg_dsn}' AS _ms_pg (TYPE postgres, READ_ONLY);
             DROP TABLE IF EXISTS \"{target_clone}\";
             CREATE TABLE \"{target_clone}\" AS SELECT * FROM _ms_pg.{upstream_clone};
             DETACH _ms_pg;
             CHECKPOINT;"
        )).map_err(|e| format!("COPY: {e}"))?;
        let n: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM \"{target_clone}\""),
            [], |r| r.get(0)
        ).unwrap_or(0);
        Ok(n)
    }).await
    .map_err(|e| err(500, &format!("task join: {e}")))?
    .map_err(|e| {
        // Flip back to failed on error.
        let _ = state.db.execute(
            "UPDATE sources SET status = 'failed', updated_at = datetime('now') WHERE id = ?1",
            &[&id as &dyn rusqlite::types::ToSql],
        );
        err(500, &e)
    })?;

    // Mark populated.
    let _ = state.db.execute(
        "UPDATE sources SET status = 'populated', last_populated_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    );

    let elapsed = t0.elapsed().as_millis() as i64;
    log(&state, "materialize", "success",
        &format!("Materialized Source '{id}': {result} rows from {upstream} → {target}"),
        Some(&id), Some(elapsed));
    Ok(Json(json!({
        "rows": result,
        "target_table": target,
        "duration_ms": elapsed,
    })))
}

// ── CDC start / stop ────────────────────────────────────────────────────────

/// Start CDC streaming for a `cdc_pg` Source. Source must be materialized first.
pub async fn cdc_start(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t0 = Instant::now();
    let row = state.db.query_one(
        "SELECT * FROM sources WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "Source not found"))?;
    let src = parse_json_fields(row);

    if src["kind"].as_str() != Some("cdc_pg") {
        return Err(err(400, "CDC streaming only applies to cdc_pg Sources"));
    }
    let connection_ref = src["connection_ref"].as_str().unwrap_or("").to_string();
    if connection_ref.is_empty() {
        return Err(err(400, "connection_ref is required"));
    }
    let target = src["target_table"].as_str().unwrap_or("").to_string();
    if target.is_empty() {
        return Err(err(400, "target_table is required. Materialize the Source first."));
    }
    let upstream = src["config"]["upstream_table"].as_str().unwrap_or("").to_string();
    if upstream.is_empty() {
        return Err(err(400, "config.upstream_table is required"));
    }
    let primary_key: Vec<String> = src["primary_key"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if primary_key.is_empty() {
        return Err(err(400, "primary_key is required for CDC streaming"));
    }

    // PG creds.
    let pg_config = get_connection_config(&state, &connection_ref)?;
    let pg = cdc::consumer::PgParams {
        host: pg_config["host"].as_str().unwrap_or("localhost").to_string(),
        port: pg_config["port"].as_u64().unwrap_or(5432) as u16,
        user: pg_config["user"].as_str().unwrap_or("postgres").to_string(),
        password: pg_config["password"].as_str().unwrap_or("").to_string(),
        database: pg_config["database"].as_str().unwrap_or("postgres").to_string(),
    };

    // Slot/publication: persisted in config JSON if previously set, else derive.
    let tenant = state.tenant_id.clone();
    let derive = |prefix: &str| format!("{prefix}_{}_{}", tenant.replace('-', "_"), id.replace('-', "_"));
    let slot_name = src["config"]["cdc_slot"].as_str()
        .filter(|s| !s.is_empty()).map(String::from)
        .unwrap_or_else(|| derive("ss"));
    let pub_name = src["config"]["cdc_publication"].as_str()
        .filter(|s| !s.is_empty()).map(String::from)
        .unwrap_or_else(|| derive("ss_pub"));

    let actual_lsn = cdc::consumer::ensure_slot(&pg, &slot_name, &pub_name, &[upstream.clone()])
        .await.map_err(|e| err(500, &e))?;
    let start_lsn = src["config"]["cdc_lsn"].as_str()
        .filter(|s| !s.is_empty() && *s != "0/0")
        .map(String::from)
        .unwrap_or(actual_lsn.clone());

    // Persist slot + lsn in config JSON. Read current config, merge, write back.
    let mut config = src["config"].clone();
    if !config.is_object() { config = json!({}); }
    config["cdc_slot"] = json!(slot_name);
    config["cdc_publication"] = json!(pub_name);
    config["cdc_lsn"] = json!(start_lsn);
    let _ = state.db.execute(
        "UPDATE sources SET config = ?1, status = 'streaming', cdc_enabled = 1, updated_at = datetime('now') WHERE id = ?2",
        &[&config.to_string() as &dyn rusqlite::types::ToSql, &id as _],
    );

    // LSN persistence callback. Also publishes to `state.cdc_change_tx` so
    // the pipeline scheduler can fire `Cdc`-triggered pipelines.
    let db_ref = state.db.clone_for_cdc();
    let id_for_lsn = id.clone();
    let cdc_change_tx = state.cdc_change_tx.clone();
    let on_lsn_update: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |lsn: String| {
        let _ = db_ref.execute(
            "UPDATE sources SET config = json_set(config, '$.cdc_lsn', ?1), updated_at = datetime('now') WHERE id = ?2",
            &[&lsn as &dyn rusqlite::types::ToSql, &id_for_lsn as _],
        );
        let _ = cdc_change_tx.send(crate::services::pipeline_scheduler::CdcChangeEvent {
            source_id: id_for_lsn.clone(),
            lsn,
        });
    });
    let db_ref2 = state.db.clone_for_cdc();
    let id_for_status = id.clone();
    let on_status_update: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |s: &str| {
        let mapped = match s {
            "running" => "streaming",
            "idle" => "populated",
            "reconnecting" => "populating",
            other => other,
        };
        let _ = db_ref2.execute(
            "UPDATE sources SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            &[&mapped as &dyn rusqlite::types::ToSql, &id_for_status as _],
        );
    });

    let cdc_key = format!("source/{}/{}", tenant, id);
    state.cdc_manager.start(cdc::CdcStartParams {
        key: cdc_key,
        pg,
        slot: slot_name.clone(),
        publication: pub_name.clone(),
        start_lsn: start_lsn.clone(),
        duckdb_path: state.duckdb_path.clone(),
        duckdb_table: target.clone(),
        pk_columns: primary_key,
        on_lsn_update,
        on_status_update,
    }).await.map_err(|e| err(500, &e))?;

    let elapsed = t0.elapsed().as_millis() as i64;
    log(&state, "cdc_start", "success",
        &format!("CDC started for Source '{id}' from LSN {start_lsn}"),
        Some(&format!("slot={slot_name}, publication={pub_name}")), Some(elapsed));
    Ok(Json(json!({
        "status": "streaming",
        "slot": slot_name,
        "publication": pub_name,
        "start_lsn": start_lsn,
    })))
}

/// Stop CDC streaming for a `cdc_pg` Source.
pub async fn cdc_stop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let key = format!("source/{}/{}", state.tenant_id, id);
    if let Err(e) = state.cdc_manager.stop(&key).await {
        return Err(err(404, &e));
    }
    let _ = state.db.execute(
        "UPDATE sources SET status = 'populated', updated_at = datetime('now') WHERE id = ?1",
        &[&id as &dyn rusqlite::types::ToSql],
    );
    log(&state, "cdc_stop", "success", &format!("CDC stopped for Source '{id}'"), Some(&id), None);
    Ok(Json(json!({"status": "stopped"})))
}

// ── Boot-time auto-resume ───────────────────────────────────────────────────

/// On server start, restart CDC streams for every `cdc_pg` Source that has
/// `cdc_enabled=1` and a non-empty `target_table`. Per docs/primer.md §3.4.
pub async fn cdc_auto_start_all(state: Arc<AppState>) {
    let rows = match state.db.query(
        "SELECT * FROM sources WHERE kind = 'cdc_pg' AND cdc_enabled = 1 AND target_table IS NOT NULL AND target_table != ''",
        &[],
    ) {
        Ok(r) => r.into_iter().map(parse_json_fields).collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!(error = %e, "[sources] CDC auto-start: failed to query sources");
            return;
        }
    };
    let mut started = 0;
    for src in rows {
        let id = src["id"].as_str().unwrap_or("").to_string();
        if id.is_empty() { continue; }
        // Reuse cdc_start by faking the request.
        match cdc_start(State(state.clone()), Path(id.clone())).await {
            Ok(_) => started += 1,
            Err((_, Json(e))) => {
                tracing::warn!(source_id = %id, error = ?e, "[sources] CDC auto-start failed");
            }
        }
    }
    if started > 0 {
        tracing::info!("[sources] CDC auto-start: resumed {} streams", started);
    }
}
