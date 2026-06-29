//! Pre-discover the tenant's schema once per session and inject it into the
//! system prompt. Replaces the per-prompt exploration loop with a single
//! cached overview, dramatically reducing turn count + token cost.
//!
//! Discovery is workspace-kind aware:
//!
//! - **Inventory** → list of DataView ids/display_names + list of graphs.
//!   Cheap (SQLite reads) so we can include the full list.
//!
//! - **Item** → per ClickHouse connection: SHOW DATABASES, then SHOW TABLES
//!   FROM each user database. Capped at a few databases × ~40 tables each
//!   to keep the injected text small (the whole point is to avoid the
//!   60K-byte dictionary dump).
//!
//! Other kinds return an empty string until they're wired.
//!
//! Called from two places:
//!
//! - `sessions::create` spawns this as a background tokio task and writes
//!   the result to `session.schema_hint`. Subsequent prompts read from the
//!   row — no rediscovery, no cost.
//!
//! - `prompts::submit` checks `schema_hint` and falls back to running
//!   `discover` inline when NULL (first prompt before pre-warm finished).

use std::sync::Arc;

use serde_json::Value;

use crate::agent::tools::WorkspaceKind;
use crate::service;
use crate::AppState;

/// Soft caps on what we put in the injected text. Sized to keep the
/// overall hint under HINT_BYTE_BUDGET so the system prompt + user
/// prompt + tool definitions stay comfortably inside gpt-4o-mini's
/// 128K-token context window.
const MAX_DATAVIEWS:         usize = 40;
const MAX_COLS_PER_DATAVIEW: usize = 20;
const MAX_GRAPHS:            usize = 20;
const MAX_DATABASES_PER_CONN: usize = 3;
const MAX_TABLES_PER_DB:     usize = 30;
/// How many curated tables per database we run `system.columns` against
/// to inline their columns. Keeps the prompt growth bounded even on
/// tenants with hundreds of matching tables.
const MAX_DETAILED_TABLES_PER_DB: usize = 8;
const MAX_COLS_PER_TABLE:    usize = 24;
/// How many curated tables per database we ALSO sample one row from.
/// Smaller than the columns subset because each sample is a separate
/// ClickHouse round-trip. The sampled rows give the model concrete
/// evidence about what values actually live in the data (e.g. `JEWELRY`
/// vs `Jewelry`) so it doesn't have to guess filter casing.
const MAX_SAMPLED_TABLES_PER_DB: usize = 3;
/// Per-cell cap when serializing a sample row. Stops a wide column like
/// `description` or `json_blob` from blowing the hint by itself.
const MAX_CELL_CHARS:        usize = 40;

/// Final hard cap on the whole schema_hint. ~40 KB ≈ ~10K tokens — keeps
/// the rest of the context window (preamble, tool catalog, user prompt,
/// chat history) under the 128K limit even on hostile tenants. Anything
/// over this gets head-truncated with an elision marker so the model can
/// still discover via `SHOW TABLES` if it needs more.
const HINT_BYTE_BUDGET:      usize = 40_000;

pub async fn discover(state: Arc<AppState>, kind: WorkspaceKind) -> String {
    let raw = match kind {
        WorkspaceKind::Inventory => discover_inventory(&state).await,
        WorkspaceKind::Item      => discover_item(&state).await,
        _ => String::new(),
    };
    enforce_budget(raw)
}

/// Defensive cap on the whole hint. If the per-section limits still let
/// the result exceed `HINT_BYTE_BUDGET` (wide ClickHouse columns, many
/// databases, long sample values, …), head-truncate and tell the model
/// what's available beyond the cut via `SHOW TABLES` so it can recover.
fn enforce_budget(hint: String) -> String {
    if hint.len() <= HINT_BYTE_BUDGET {
        return hint;
    }
    // Try to cut on a newline so the head ends on a clean section.
    let mut cut = HINT_BYTE_BUDGET;
    if let Some(boundary) = hint[..cut].rfind('\n') {
        cut = boundary;
    }
    let elided_bytes = hint.len() - cut;
    let mut out = hint[..cut].to_string();
    out.push_str(&format!(
        "\n\n…[catalog truncated — {elided_bytes} bytes elided. \
         Use `SHOW DATABASES` / `SHOW TABLES FROM <db>` / `DESCRIBE <db>.<table>` \
         to explore anything not listed above.]\n"
    ));
    out
}

async fn discover_inventory(state: &AppState) -> String {
    let mut out = String::new();

    // Pre-index sources by id so we can resolve a DataView's source
    // binding without a second round-trip per DV.
    let sources_by_id: std::collections::HashMap<String, Value> = match service::sources::list(state).await {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from).map(|id| (id, r)))
            .collect(),
        Err(_) => std::collections::HashMap::new(),
    };

    if let Ok(rows) = service::dataviews::list(state).await {
        if !rows.is_empty() {
            out.push_str("## DataViews available in this tenant\n\n");
            for dv in rows.iter().take(MAX_DATAVIEWS) {
                let id   = sval(dv, "id");
                let name = sval(dv, "display_name");
                // Pull column names straight off the DataView row's
                // `columns` JSON. Avoids a second describe round-trip
                // per DataView at session start.
                let cols: Vec<String> = dv
                    .get("columns")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| c.get("name").and_then(|v| v.as_str()).map(String::from))
                            .take(MAX_COLS_PER_DATAVIEW)
                            .collect()
                    })
                    .unwrap_or_default();

                // Graph-source DataViews carry empty `columns: []` —
                // the schema is computed at read time from the graph
                // snapshot. Without a column hint here the agent has
                // no way to know `dv_articles_graph` exposes `brand`,
                // `lw_revenue`, etc. — it'll skip past it to other
                // DVs that DO list those columns, often the wrong
                // ones. Synthesize the hint from the loaded graph.
                if cols.is_empty() {
                    if let Some(extra) = graph_source_hint(state, dv, &sources_by_id).await {
                        out.push_str(&format!("- `{id}` — {name}: {extra}\n"));
                        continue;
                    }
                    out.push_str(&format!("- `{id}` — {name}\n"));
                } else {
                    let cols_str = cols
                        .iter()
                        .map(|c| format!("`{c}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!("- `{id}` — {name}: {cols_str}\n"));
                }
            }
            if rows.len() > MAX_DATAVIEWS {
                out.push_str(&format!("- … +{} more (use `list_dataviews` to enumerate)\n", rows.len() - MAX_DATAVIEWS));
            }
            out.push('\n');
        }
    }

    if let Ok(rows) = service::graphs::list(state).await {
        if !rows.is_empty() {
            out.push_str("## Graphs available\n\n");
            for g in rows.iter().take(MAX_GRAPHS) {
                let id   = sval(g, "id");
                let name = sval(g, "display_name");
                out.push_str(&format!("- `{id}` — {name}\n"));
            }
            out.push('\n');
        }
    }

    out
}

/// Synthesize a column/usage hint for a graph-source DataView whose
/// stored `columns: []` would otherwise leave the agent blind to its
/// real shape. Resolves the DataView's source → graph_id → loaded
/// snapshot, then lists the kinds (legal `node_kind` / `group_by`
/// values) and primary metric names that appear on every projected
/// row. Returns `None` when the source isn't graph-backed, the graph
/// isn't loaded, or anything else falls through — caller emits the
/// bare "- `id` — name" line in that case.
async fn graph_source_hint(
    state: &AppState,
    dv: &Value,
    sources_by_id: &std::collections::HashMap<String, Value>,
) -> Option<String> {
    let source_id = dv.get("source")?.get("config")?.get("source_id")?.as_str()?;
    let src = sources_by_id.get(source_id)?;
    if src.get("kind")?.as_str()? != "graph" {
        return None;
    }
    let cfg = src.get("config")?;
    let graph_id = cfg.get("graph_id")?.as_str()?;

    let slot = {
        let graphs = state.graphs.read().await;
        graphs.get(graph_id).cloned()?
    };
    let snap = slot.load();
    let g = snap.as_ref()?;

    // Distinct kind names (skip synthetic root) — these are the legal
    // values for `node_kind` / `group_by[0]`. Bealls has ~14 kinds, so
    // a flat list is readable; cap at 20 to keep the hint bounded.
    let kinds: Vec<String> = g
        .kinds
        .iter()
        .filter(|(_, k)| k.name != "__root__")
        .map(|(_, k)| k.name.clone())
        .take(20)
        .collect();

    // Distinct primary metric names. Cross-source duplicates (e.g.
    // inventory.oh + inventory_per_dc.oh) collapse via a set so the
    // hint doesn't repeat itself.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut metrics: Vec<String> = Vec::new();
    for mid in g.metrics.primary_metric_ids() {
        let m = g.metrics.get(mid);
        if seen.insert(m.name.clone()) {
            metrics.push(m.name.clone());
            if metrics.len() >= 20 {
                break;
            }
        }
    }

    let kinds_str   = kinds.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ");
    let metrics_str = metrics.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ");
    Some(format!(
        "graph-backed (id=`{graph_id}`). `node_kind` / `group_by`: {kinds_str}. Per-row metrics: {metrics_str}. Use `group_by=[\"<kind>\"]` (e.g. `brand`) to roll up to that grain."
    ))
}

async fn discover_item(state: &AppState) -> String {
    let conns = match service::connections::list(state).await {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let ch_conns: Vec<&Value> = conns
        .iter()
        .filter(|c| sval(c, "type") == "clickhouse")
        .collect();
    if ch_conns.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "## ClickHouse schema overview (use `clickhouse_query` for DESCRIBE / SELECT against these)\n",
    );

    for c in ch_conns {
        let conn_id  = sval(c, "id");
        let display  = c.get("display_name").and_then(|v| v.as_str()).unwrap_or(&conn_id).to_string();
        out.push_str(&format!("\n### Connection `{conn_id}` ({display})\n\n"));

        // SHOW DATABASES
        let dbs = run_ch(state, &conn_id, "SHOW DATABASES").await;
        let user_dbs: Vec<String> = dbs
            .into_iter()
            .filter(|d| !matches!(d.as_str(), "system" | "INFORMATION_SCHEMA" | "information_schema" | "default"))
            .collect();

        if user_dbs.is_empty() {
            out.push_str("(no user databases discovered)\n");
            continue;
        }

        out.push_str(&format!(
            "Databases: {}\n",
            user_dbs.iter().map(|d| format!("`{d}`")).collect::<Vec<_>>().join(", ")
        ));

        // For the first N user databases, enumerate tables.
        for db in user_dbs.iter().take(MAX_DATABASES_PER_CONN) {
            let tables = run_ch(state, &conn_id, &format!("SHOW TABLES FROM `{db}`")).await;
            if tables.is_empty() { continue; }
            let shown = tables.len().min(MAX_TABLES_PER_DB);
            let listed: Vec<String> = tables.iter().take(shown).map(|t| format!("`{t}`")).collect();
            out.push_str(&format!(
                "\n**`{db}`** ({} tables): {}",
                tables.len(),
                listed.join(", ")
            ));
            if tables.len() > shown {
                out.push_str(&format!(" … +{} more", tables.len() - shown));
            }
            out.push('\n');

            // Curated column inlining. Filter the table list to those
            // that look like "real" fact / metric / dimension tables
            // (heuristic on name patterns) and bulk-query their columns
            // from `system.columns` in one shot per database. Skipping
            // forecast / WP / temp tables keeps the noise down.
            let curated: Vec<&String> = tables
                .iter()
                .filter(|t| is_real_data_table(t))
                .take(MAX_DETAILED_TABLES_PER_DB)
                .collect();
            if curated.is_empty() { continue; }

            // Columns via bulk system.columns query (cheap, one round-trip).
            let in_list = curated
                .iter()
                .map(|t| format!("'{}'", t.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            let cols_sql = format!(
                "SELECT table, name, type FROM system.columns \
                 WHERE database = '{db}' AND table IN ({in_list}) \
                 ORDER BY table, position LIMIT 5000"
            );
            let col_rows = run_ch_rows(state, &conn_id, &cols_sql).await;

            // Group columns by table for easier composition with sample rows.
            let mut cols_by_table: std::collections::BTreeMap<String, Vec<(String, String)>> =
                std::collections::BTreeMap::new();
            for row in &col_rows {
                let t = row.get("table").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let n = row.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let ty = row.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !t.is_empty() && !n.is_empty() {
                    cols_by_table.entry(t).or_default().push((n, ty));
                }
            }

            // Sample rows. One concurrent `SELECT * LIMIT 1` per sampled
            // table — gives the model evidence about real values it can
            // filter on (e.g. `JEWELRY` vs `Jewelry`). Capped to a smaller
            // subset than columns since each is its own round-trip.
            let sampled: Vec<&String> = curated.iter().copied().take(MAX_SAMPLED_TABLES_PER_DB).collect();
            let conn_id_for_sample = conn_id.clone();
            let sample_futs = sampled
                .iter()
                .map(|t| {
                    let table = t.to_string();
                    let sql = format!("SELECT * FROM `{db}`.`{table}` LIMIT 1");
                    let cid = conn_id_for_sample.clone();
                    async move {
                        let rows = run_ch_rows(state, &cid, &sql).await;
                        (table, rows.into_iter().next())
                    }
                })
                .collect::<Vec<_>>();
            let samples: std::collections::HashMap<String, Option<Value>> = futures::future::join_all(sample_futs)
                .await
                .into_iter()
                .collect();

            if cols_by_table.is_empty() && samples.is_empty() { continue; }
            out.push_str("\nColumns + sample rows for selected tables:\n");
            for (table, cols) in &cols_by_table {
                let cols_str = cols
                    .iter()
                    .take(MAX_COLS_PER_TABLE)
                    .map(|(n, ty)| format!("`{n}` {ty}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let elided = if cols.len() > MAX_COLS_PER_TABLE {
                    format!(" … +{} more", cols.len() - MAX_COLS_PER_TABLE)
                } else {
                    String::new()
                };
                out.push_str(&format!("- `{db}.{table}`: {cols_str}{elided}\n"));
                if let Some(Some(sample_row)) = samples.get(table) {
                    out.push_str(&format!("   sample: {}\n", format_sample_row(sample_row)));
                }
            }
        }
    }

    out
}

// ── helpers ──────────────────────────────────────────────────────────────

fn sval(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// Render a single sample row as a compact `{col=value, col=value, ...}`
/// string for the schema hint. Long values are truncated per-cell so a
/// stray description column doesn't blow the whole hint. The goal is to
/// give the model concrete evidence about value casing + typical shape
/// (e.g. `l1_name=JEWELRY` shows the actual capitalization to filter on).
fn format_sample_row(row: &Value) -> String {
    let Some(obj) = row.as_object() else { return String::new(); };
    let parts: Vec<String> = obj
        .iter()
        .map(|(k, v)| {
            let raw = match v {
                Value::Null => "null".to_string(),
                Value::String(s) => format!("\"{s}\""),
                other => other.to_string(),
            };
            let val = if raw.chars().count() > MAX_CELL_CHARS {
                let head: String = raw.chars().take(MAX_CELL_CHARS).collect();
                format!("{head}…")
            } else {
                raw
            };
            format!("`{k}`={val}")
        })
        .collect();
    format!("{{ {} }}", parts.join(", "))
}

/// Heuristic: "is this table likely to hold real metric / fact / dimension
/// data the planner cares about?" Positive patterns dominate negative
/// patterns; a table excluded by a negative match (e.g. `_temp`) is
/// skipped even when it also matches a positive one.
fn is_real_data_table(name: &str) -> bool {
    let n = name.to_lowercase();
    let positive: &[&str] = &[
        "kpi_", "fact_", "f_actuals", "_actual", "_historical",
        "_daily", "_weekly", "_monthly", "_yearly",
        "sales", "revenue", "orders", "inventory", "items", "products", "customers",
        "calculated_",
    ];
    let negative: &[&str] = &[
        "_temp", "_tmp", "_test", "_backup", "_old", "_archive",
        "_staging", "_wp", "_forecast", "_expected", "_session", "_lock",
    ];
    let pos = positive.iter().any(|p| n.contains(p));
    let neg = negative.iter().any(|p| n.contains(p));
    pos && !neg
}

/// Run a ClickHouse query and return the raw `rows` array as JSON objects.
/// Used by the curated column-pre-discovery to bulk-select column metadata
/// from `system.columns` (multi-column shape, not the single-column shape
/// that `run_ch` flattens).
async fn run_ch_rows(state: &AppState, conn_id: &str, sql: &str) -> Vec<Value> {
    let args = service::connections::ClickhouseQueryArgs {
        sql: sql.to_string(),
        limit: Some(5000),
        offset: None,
    };
    let resp = match service::connections::clickhouse_query(state, conn_id, args).await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    resp.get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Run a `SHOW`-style ClickHouse query and return the single-column result
/// as a Vec<String>. Falls back to an empty vec on any error so the caller
/// gracefully degrades to the next discovery step.
async fn run_ch(state: &AppState, conn_id: &str, sql: &str) -> Vec<String> {
    let args = service::connections::ClickhouseQueryArgs {
        sql: sql.to_string(),
        limit: Some(500),
        offset: None,
    };
    let resp = match service::connections::clickhouse_query(state, conn_id, args).await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    resp.get("rows")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    // SHOW results put the value under `name` (e.g. database
                    // name, table name). Fall back to the first string field.
                    let obj = r.as_object()?;
                    if let Some(s) = obj.get("name").and_then(|v| v.as_str()) {
                        return Some(s.to_string());
                    }
                    obj.values().find_map(|v| v.as_str().map(String::from))
                })
                .collect()
        })
        .unwrap_or_default()
}
