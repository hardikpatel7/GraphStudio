//! Dashboard service layer.
//!
//! A dashboard owns a `layout_json` (composition tree of rows/columns/widget
//! leaves) plus a synthetic `session_id` — every widget run is a prompt in
//! that session, so cost tracking and the prompt-detail drawer come for
//! free.
//!
//! The widget runner ([`run_widget`]) is the workhorse:
//!   1. Walks the layout tree to find the requested widget node.
//!   2. Loads the workspace's pre-discovered `schema_hint`.
//!   3. Composes a kind-specific output contract ("reply with exactly one
//!      `chart` block of type X" for chart kinds; markdown contracts for
//!      table/text).
//!   4. Runs the prompt through the existing Rig pipeline against the
//!      dashboard's synthetic session, with the contract appended as
//!      `RunnerInput.addendum`.
//!   5. Extracts the structured payload from the response and upserts
//!      into `widget_cache`.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::agent::{
    llm::{self, Backend, ModelEntry, RunnerInput},
    meter::hook::SseEvent,
    meter::writer::{LlmUsageRow, MeterEvent},
    tools::WorkspaceKind,
};
use crate::AppState;

/// Public widget node kinds. Mirrored in the UI's `dashboards/types.ts`.
const KIND_KPI:         &str = "kpi";
const KIND_BAR:         &str = "bar";
const KIND_LINE:        &str = "line";
const KIND_PIE:         &str = "pie";
const KIND_STACKED_BAR: &str = "stacked_bar";
const KIND_BULLET:      &str = "bullet";
const KIND_PARETO:      &str = "pareto";
const KIND_FUNNEL:      &str = "funnel";
const KIND_GAUGE:       &str = "gauge";
const KIND_SPARKLINE:   &str = "sparkline";
const KIND_HEATMAP:     &str = "heatmap";
const KIND_TREEMAP:     &str = "treemap";
const KIND_HISTOGRAM:   &str = "histogram";
const KIND_SLOPE:       &str = "slope";
const KIND_BOXPLOT:     &str = "boxplot";
const KIND_WATERFALL:   &str = "waterfall";
const KIND_TABLE:       &str = "table";
const KIND_TEXT:        &str = "text";

/// Default layout for a freshly-created dashboard: an empty top-level column.
const EMPTY_LAYOUT: &str =
    r#"{"version":1,"root":{"type":"column","id":"root","children":[]}}"#;

/// Model the widget runner uses by default. Dashboards are read-many,
/// write-once workloads; cost discipline matters more than peak quality.
/// Falls back gracefully if the allowlist is missing this row.
// Dashboards run multi-turn tool-heavy prompts (schema discovery →
// dataview_read → parse → chart spec). gpt-4o-mini consistently
// stalls on these (multiple `streaming` prompts going 10+ minutes
// without producing a chart block). gpt-4.1 mini has 1M context
// AND a stronger tool-use loop; it's the right default for
// per-widget runs. Users can still pick gpt-4o-mini explicitly via
// `POST /sessions { model: ... }` for plain chat sessions.
const DEFAULT_WIDGET_MODEL: &str = "gpt-4.1-mini";

// ── CRUD ─────────────────────────────────────────────────────────────────

pub fn list_for_workspace(state: &AppState, workspace_id: &str) -> Result<Vec<Value>> {
    state.agent.db.query(
        "SELECT d.id, d.workspace_id, d.session_id, d.name, d.description, \
                d.created_at, d.updated_at, s.model AS model \
         FROM dashboard d JOIN session s ON s.id = d.session_id \
         WHERE d.workspace_id = ? ORDER BY d.updated_at DESC",
        &[&workspace_id],
    )
}

pub fn get(state: &AppState, id: &str) -> Result<Value> {
    state.agent.db.query_one(
        "SELECT d.id, d.workspace_id, d.session_id, d.name, d.description, \
                d.layout_json, d.created_at, d.updated_at, s.model AS model \
         FROM dashboard d JOIN session s ON s.id = d.session_id \
         WHERE d.id = ?",
        &[&id],
    ).context("dashboard not found")
}

/// Look up all cached widget rows for a dashboard, keyed by node_id. The
/// view route merges this into the response so the UI gets the layout
/// + every widget's last-known data in one round trip.
pub fn widget_cache(state: &AppState, dashboard_id: &str) -> Result<Vec<Value>> {
    state.agent.db.query(
        "SELECT node_id, spec_hash, data_json, fetched_at, prompt_id \
         FROM widget_cache WHERE dashboard_id = ?",
        &[&dashboard_id],
    )
}

/// Create a dashboard + its synthetic session in one shot. Returns the
/// new dashboard row.
pub fn create(
    state: &AppState,
    workspace_id: &str,
    name: &str,
    description: Option<&str>,
) -> Result<Value> {
    let now = Utc::now().timestamp_millis();
    let id  = format!("dash_{}", Uuid::new_v4().simple());
    let sid = format!("sess_{}", Uuid::new_v4().simple());

    // The synthetic session uses the workspace's default model + provider.
    // If the allowlist doesn't have an entry for that, fall back to the
    // first enabled row. We need any valid model; the actual widget runs
    // can override per-request later if we add a per-dashboard model.
    let model_row = state
        .agent
        .db
        .query_one(
            "SELECT provider, model FROM model_allowlist \
             WHERE enabled = 1 ORDER BY (model = ?) DESC, provider, model LIMIT 1",
            &[&DEFAULT_WIDGET_MODEL],
        )
        .context("no enabled models in allowlist")?;
    let provider = model_row.get("provider").and_then(|v| v.as_str()).unwrap_or("openai");
    let model    = model_row.get("model")   .and_then(|v| v.as_str()).unwrap_or(DEFAULT_WIDGET_MODEL);
    let title    = format!("{name} · widgets");

    state.agent.db.execute(
        "INSERT INTO session (id, workspace_id, provider, model, title, provider_state, created_at, last_active_at) \
         VALUES (?, ?, ?, ?, ?, NULL, ?, ?)",
        &[&sid, &workspace_id, &provider, &model, &title, &now, &now],
    )?;

    state.agent.db.execute(
        "INSERT INTO dashboard (id, workspace_id, session_id, name, description, layout_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        &[&id, &workspace_id, &sid, &name, &description, &EMPTY_LAYOUT, &now, &now],
    )?;

    get(state, &id)
}

#[derive(Default)]
pub struct PatchArgs {
    pub name: Option<String>,
    pub description: Option<Option<String>>, // outer Option = field present? inner = null vs value
    pub layout_json: Option<String>,
    /// When set, updates the dashboard's synthetic session's `model`
    /// column. Must be an enabled row in `model_allowlist` or the call
    /// errors out before any UPDATE runs. Cached widget payloads are
    /// left alone — the next per-widget ↻ picks up the new model.
    pub model: Option<String>,
}

pub fn patch(state: &AppState, id: &str, args: PatchArgs) -> Result<Value> {
    use rusqlite::types::ToSql;

    // Model change is independent of the dashboard row — it updates
    // the synthetic session. Validate + dispatch up front so we don't
    // half-apply a partial PATCH.
    if let Some(model) = args.model.as_ref() {
        let allowed = state.agent.db.query_one(
            "SELECT 1 AS ok FROM model_allowlist WHERE model = ? AND enabled = 1",
            &[&model],
        );
        if allowed.is_err() {
            return Err(anyhow!("model `{model}` is not enabled in the allowlist"));
        }
        let row = get(state, id)?;
        let session_id = row.get("session_id").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("dashboard has no session_id"))?
            .to_string();
        let now = Utc::now().timestamp_millis();
        state.agent.db.execute(
            "UPDATE session SET model = ?, last_active_at = ? WHERE id = ?",
            &[&model, &now, &session_id],
        )?;
    }

    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(name) = args.name {
        sets.push("name = ?");
        vals.push(Box::new(name));
    }
    if let Some(desc) = args.description {
        sets.push("description = ?");
        vals.push(Box::new(desc));
    }
    if let Some(layout) = args.layout_json {
        // Lightweight validation — must be valid JSON. Shape validation
        // happens UI-side; the server treats the tree as opaque.
        serde_json::from_str::<Value>(&layout).context("layout_json is not valid JSON")?;
        sets.push("layout_json = ?");
        vals.push(Box::new(layout));
    }
    if sets.is_empty() {
        return get(state, id);
    }
    let now = Utc::now().timestamp_millis();
    sets.push("updated_at = ?");
    vals.push(Box::new(now));

    let sql = format!("UPDATE dashboard SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.to_string()));

    let refs: Vec<&dyn ToSql> = vals.iter().map(|b| b.as_ref()).collect();
    let n = state.agent.db.execute(&sql, &refs)?;
    if n == 0 {
        anyhow::bail!("dashboard not found");
    }
    get(state, id)
}

/// Cascade delete with FOREIGN KEYS=ON. Order matters because of these
/// FK edges (parent ← child):
///   session   ← dashboard.session_id
///   prompt    ← widget_cache.prompt_id
///   prompt    ← api_call.prompt_id / llm_usage.prompt_id / response_chunk.prompt_id
///   session   ← prompt.session_id
///   dashboard ← widget_cache.dashboard_id
/// So we delete leaf-first toward the dashboard, drop the dashboard row
/// (which is what `session` is referenced by), then cascade the session
/// and its prompts. Earlier the session was being dropped before the
/// dashboard row, which tripped `FOREIGN KEY constraint failed`.
pub fn delete(state: &AppState, id: &str) -> Result<()> {
    let row = get(state, id).context("dashboard not found")?;
    let session_id = row.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();

    // widget_cache → dashboard / prompt
    state.agent.db.execute(
        "DELETE FROM widget_cache WHERE dashboard_id = ?",
        &[&id],
    )?;

    // Drop the dashboard row NOW so nothing references the session.
    state.agent.db.execute(
        "DELETE FROM dashboard WHERE id = ?",
        &[&id],
    )?;

    // Cascade the synthetic session in the same order as the sessions
    // DELETE route: children of prompt first, then prompt, then session.
    if !session_id.is_empty() {
        let sub = "(SELECT id FROM prompt WHERE session_id = ?)";
        state.agent.db.execute(
            &format!("DELETE FROM response_chunk WHERE prompt_id IN {sub}"),
            &[&session_id],
        )?;
        state.agent.db.execute(
            &format!("DELETE FROM api_call WHERE prompt_id IN {sub}"),
            &[&session_id],
        )?;
        state.agent.db.execute(
            &format!("DELETE FROM llm_usage WHERE prompt_id IN {sub}"),
            &[&session_id],
        )?;
        state.agent.db.execute(
            "DELETE FROM prompt WHERE session_id = ?",
            &[&session_id],
        )?;
        state.agent.db.execute(
            "DELETE FROM session WHERE id = ?",
            &[&session_id],
        )?;
    }

    Ok(())
}

// ── Tree helpers ─────────────────────────────────────────────────────────

/// Resolved widget definition pulled from the layout tree. A widget can
/// be either standalone (raw `kind` + `prompt`) or backed by a component
/// (look up `component_id`, substitute `placeholder_values` into its
/// template). [`find_widget`] returns the same shape either way so the
/// runner doesn't need to branch.
struct ResolvedWidget {
    kind: String,
    prompt: String,
}

/// Locate a widget leaf node by id and resolve it into a ready-to-run
/// `(kind, prompt)`. For component-backed widgets, the lookup also pulls
/// the component row and substitutes placeholder values. Errors when the
/// node doesn't exist, isn't a widget, or the bound component is missing.
pub fn find_widget(state: &AppState, layout: &Value, node_id: &str) -> Result<(String, String)> {
    find_widget_with_overrides(state, layout, node_id, &serde_json::Map::new())
}

/// Same as `find_widget` but lets the caller pass per-placeholder
/// values that override the widget node's stored `placeholder_values`.
/// Used by the drill-down path: a click in widget A populates widget
/// B's placeholder X without persisting that pick. Empty `overrides`
/// degenerates to the cache-friendly path.
pub fn find_widget_with_overrides(
    state: &AppState,
    layout: &Value,
    node_id: &str,
    overrides: &serde_json::Map<String, Value>,
) -> Result<(String, String)> {
    let root = layout.get("root").ok_or_else(|| anyhow!("layout missing root"))?;
    let mut raw  = find_widget_rec(root, node_id)
        .ok_or_else(|| anyhow!("node `{node_id}` not found or not a widget"))?;
    if !overrides.is_empty() {
        let obj = raw.as_object_mut().ok_or_else(|| anyhow!("widget node is not an object"))?;
        let existing = obj
            .remove("placeholder_values")
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        let mut merged = existing;
        for (k, v) in overrides {
            merged.insert(k.clone(), v.clone());
        }
        obj.insert("placeholder_values".into(), Value::Object(merged));
    }
    let resolved = resolve_widget(state, raw)?;
    Ok((resolved.kind, resolved.prompt))
}

/// Walk the tree looking for the widget node with the matching id.
/// Returns the entire node Value so the caller can decide how to resolve
/// it (component-backed vs standalone).
fn find_widget_rec(node: &Value, node_id: &str) -> Option<Value> {
    let ty = node.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let id = node.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if ty == "widget" && id == node_id {
        return Some(node.clone());
    }
    if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
        for c in children {
            if let Some(found) = find_widget_rec(c, node_id) {
                return Some(found);
            }
        }
    }
    None
}

/// Resolve a widget node into runner-ready (kind, prompt). When
/// `component_id` is present, look up the component, substitute
/// placeholder values into the template, and use the component's kind.
/// When absent, use the widget's inline `kind` + `prompt`.
fn resolve_widget(state: &AppState, node: Value) -> Result<ResolvedWidget> {
    let component_id = node.get("component_id").and_then(|v| v.as_str()).unwrap_or("");
    if component_id.is_empty() {
        let kind   = node.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let template = node.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        // Substitute `<placeholder>` tokens on inline prompts the
        // same way we do for component-backed widgets. This is what
        // lets a drill-down dashboard work with hand-written prompts
        // (no component required) — the widget's `placeholder_values`
        // map is merged with any drill override and the resulting
        // brand/article/etc. is splatted into the prompt before the
        // LLM ever sees it.
        let values = node
            .get("placeholder_values")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let (resolved, missing) = super::components::substitute(template, &values);
        if !missing.is_empty() {
            return Err(anyhow!(
                "missing values for placeholder(s): {}. Open the widget in Edit mode and fill them in.",
                missing.join(", ")
            ));
        }
        return Ok(ResolvedWidget { kind, prompt: resolved });
    }

    let component = super::components::get(state, component_id)
        .map_err(|_| anyhow!(
            "widget is backed by component `{component_id}` but that component no longer exists. \
             Re-open the dashboard in Edit mode and either re-bind the widget or delete it."
        ))?;
    let kind = component.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let template = component.get("prompt_template").and_then(|v| v.as_str()).unwrap_or("");

    // Pull the per-widget placeholder values; tolerate missing/empty.
    let values = node
        .get("placeholder_values")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let (resolved, missing) = super::components::substitute(template, &values);
    if !missing.is_empty() {
        return Err(anyhow!(
            "missing values for placeholder(s): {}. Open the widget in Edit mode and fill them in.",
            missing.join(", ")
        ));
    }
    Ok(ResolvedWidget { kind, prompt: resolved })
}

/// Collect every widget node id under a given subtree id (or under the
/// whole tree when `subtree_id` is `None` / matches the root). Used by
/// the refresh-all and refresh-subtree routes to fan out.
pub fn collect_widget_ids(layout: &Value, subtree_id: Option<&str>) -> Vec<String> {
    let root = match layout.get("root") {
        Some(r) => r,
        None    => return Vec::new(),
    };
    let target = match subtree_id {
        None | Some("root") => root,
        Some(id) => match find_subtree(root, id) {
            Some(n) => n,
            None    => return Vec::new(),
        },
    };
    let mut out = Vec::new();
    collect_rec(target, &mut out);
    out
}

fn find_subtree<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
    if node.get("id").and_then(|v| v.as_str()) == Some(id) {
        return Some(node);
    }
    node.get("children")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.iter().find_map(|c| find_subtree(c, id)))
}

fn collect_rec(node: &Value, out: &mut Vec<String>) {
    if node.get("type").and_then(|v| v.as_str()) == Some("widget") {
        if let Some(id) = node.get("id").and_then(|v| v.as_str()) {
            out.push(id.to_string());
        }
        return;
    }
    if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
        for c in children {
            collect_rec(c, out);
        }
    }
}

// ── Widget runner ────────────────────────────────────────────────────────

/// Run a single widget's prompt through the existing Rig pipeline, parse
/// the response into the renderer-ready payload, upsert into widget_cache.
/// Returns the parsed payload (caller can ship it directly to the UI).
pub async fn run_widget(
    state: Arc<AppState>,
    dashboard_id: &str,
    node_id: &str,
) -> Result<Value> {
    run_widget_with_overrides(state, dashboard_id, node_id, serde_json::Map::new()).await
}

/// Same as `run_widget` but with caller-supplied placeholder overrides.
/// When overrides are non-empty the run skips the cache upsert — drill
/// runs are transient (a brand X selection) and shouldn't pollute the
/// dashboard's saved state.
pub async fn run_widget_with_overrides(
    state: Arc<AppState>,
    dashboard_id: &str,
    node_id: &str,
    overrides: serde_json::Map<String, Value>,
) -> Result<Value> {
    let dashboard = get(&state, dashboard_id)?;
    let workspace_id = dashboard.get("workspace_id").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("dashboard missing workspace_id"))?
        .to_string();
    let session_id = dashboard.get("session_id").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("dashboard missing session_id"))?
        .to_string();
    let layout_str = dashboard.get("layout_json").and_then(|v| v.as_str())
        .or_else(|| dashboard.get("layout_json").map(|v| v.as_str().unwrap_or("")))
        .unwrap_or("");
    let layout: Value = if layout_str.is_empty() {
        dashboard.get("layout_json").cloned().unwrap_or_else(|| json!({}))
    } else {
        serde_json::from_str(layout_str).context("dashboard layout_json invalid")?
    };

    let is_drill = !overrides.is_empty();
    let (kind, prompt) = find_widget_with_overrides(&state, &layout, node_id, &overrides)?;
    if prompt.trim().is_empty() {
        anyhow::bail!("widget `{node_id}` has no prompt yet");
    }

    let started_at = std::time::Instant::now();
    let (data, prompt_id) = run_kind_prompt(state.clone(), &workspace_id, &session_id, &kind, &prompt).await?;
    let wall_ms = started_at.elapsed().as_millis() as i64;

    // Resolve cost + the canonical LLM latency from llm_usage so the
    // UI can show how long the prompt actually took on the provider
    // side. `wall_ms` is the total including tool dispatch + parse;
    // we expose both so a slow widget run is debuggable (LLM-bound
    // vs tool-bound).
    let cost_usd = crate::agent::meter::pricing::prompt_cost_usd(&state.agent.db, &prompt_id)
        .ok()
        .flatten();
    let llm_latency_ms = state.agent.db.query_one(
        "SELECT latency_ms FROM llm_usage WHERE prompt_id = ?",
        &[&prompt_id],
    )
    .ok()
    .and_then(|r| r.get("latency_ms").and_then(|v| v.as_i64()));

    // Tool-call telemetry. The agent often retries a single dataview
    // / duckdb_query several times with slight arg variations; each
    // failure is invisible to the user today. Surfacing
    // `tool_calls_total` + `tool_errors` + a sample first-error
    // message in the meta envelope means the widget footer can warn
    // "(3 of 8 tool calls errored)" instead of looking like a clean
    // run. The widget body still shows whatever the final response
    // produced; the footer is the diagnostic.
    let api_rows = state
        .agent
        .db
        .query(
            "SELECT status, error, tool_name FROM api_call WHERE prompt_id = ? ORDER BY started_at",
            &[&prompt_id],
        )
        .unwrap_or_default();
    let tool_calls_total = api_rows.len() as i64;
    let mut tool_errors  = 0i64;
    let mut first_error: Option<(String, String)> = None;
    for row in &api_rows {
        let status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status == "ok" || status == "cache_hit" {
            continue;
        }
        tool_errors += 1;
        if first_error.is_none() {
            let tool = row.get("tool_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let err  = row.get("error").and_then(|v| v.as_str()).unwrap_or("").to_string();
            first_error = Some((tool, err));
        }
    }

    let fetched_at = Utc::now().timestamp_millis();
    let meta = json!({
        "prompt_id":         prompt_id,
        "wall_ms":           wall_ms,
        "llm_ms":            llm_latency_ms,
        "cost_usd":          cost_usd,
        "fetched_at":        fetched_at,
        "from_cache":        false,
        "tool_calls_total":  tool_calls_total,
        "tool_errors":       tool_errors,
        "first_tool_error":  first_error.as_ref().map(|(t, e)| format!("{t}: {e}")),
    });
    let envelope = json!({ "data": data, "meta": meta });

    // Skip cache upsert on drill runs — they're parameterized by a
    // transient placeholder pick (a clicked brand or article) that
    // shouldn't overwrite the saved dashboard payload.
    if is_drill {
        return Ok(envelope);
    }

    // Upsert into widget_cache. Cache stores only the renderer-ready
    // payload (`data`); the meta envelope is per-run telemetry that
    // shouldn't be replayed on a cold open.
    let spec_hash  = hash_spec(&kind, &prompt);
    let data_str   = serde_json::to_string(&envelope.get("data").cloned().unwrap_or(Value::Null)).unwrap_or_default();
    state.agent.db.execute(
        "INSERT INTO widget_cache (dashboard_id, node_id, spec_hash, data_json, fetched_at, prompt_id) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(dashboard_id, node_id) DO UPDATE SET \
            spec_hash = excluded.spec_hash, \
            data_json = excluded.data_json, \
            fetched_at = excluded.fetched_at, \
            prompt_id = excluded.prompt_id",
        &[
            &dashboard_id,
            &node_id,
            &spec_hash,
            &data_str,
            &fetched_at,
            &prompt_id,
        ],
    )?;

    Ok(envelope)
}

/// Inner pipeline shared by [`run_widget`] and components::preview. Runs
/// a prompt against the workspace's tool catalog with the kind-specific
/// output addendum, parses the response into a renderer-ready payload,
/// and returns `(payload, prompt_id)`. The caller decides what to do
/// with the prompt_id (write to widget_cache for dashboards, ignore for
/// previews).
///
/// Side effects: inserts a `prompt` row, records `llm_usage` via the
/// meter writer, updates the prompt row's status. No widget_cache touch.
pub(super) async fn run_kind_prompt(
    state: Arc<AppState>,
    workspace_id: &str,
    session_id: &str,
    kind: &str,
    prompt: &str,
) -> Result<(Value, String)> {
    // Workspace kind + tool allowlist (catalog injection).
    let ws = state.agent.db.query_one(
        "SELECT kind FROM workspace WHERE id = ?",
        &[&workspace_id],
    ).context("workspace not found")?;
    let workspace_kind = ws.get("kind").and_then(|v| v.as_str())
        .and_then(WorkspaceKind::from_str)
        .ok_or_else(|| anyhow!("unknown workspace kind"))?;

    // Session model + schema hint. Schema hint may be NULL on a
    // freshly-created session — run inline as a fallback (same pattern
    // as the chat prompts/submit route).
    let session = state.agent.db.query_one(
        "SELECT model, schema_hint FROM session WHERE id = ?",
        &[&session_id],
    ).context("session not found")?;
    let model_name = session.get("model").and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_WIDGET_MODEL).to_string();
    let schema_hint: String = match session.get("schema_hint").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            let h = crate::agent::schema::discover(state.clone(), workspace_kind).await;
            if !h.is_empty() {
                let _ = state.agent.db.execute(
                    "UPDATE session SET schema_hint = ? WHERE id = ?",
                    &[&h, &session_id],
                );
            }
            h
        }
    };

    let model_row = state.agent.db.query_one(
        "SELECT provider, model, display_name, backend FROM model_allowlist \
         WHERE model = ? AND enabled = 1",
        &[&model_name],
    ).context("model not enabled in allowlist")?;
    let model_entry = ModelEntry {
        provider:     model_row.get("provider")    .and_then(|v| v.as_str()).unwrap_or("openai").to_string(),
        model:        model_row.get("model")       .and_then(|v| v.as_str()).unwrap_or(&model_name).to_string(),
        display_name: model_row.get("display_name").and_then(|v| v.as_str()).unwrap_or(&model_name).to_string(),
        backend:      Backend::from_str(model_row.get("backend").and_then(|v| v.as_str()).unwrap_or("rig")),
    };

    let prompt_id = format!("pmt_{}", Uuid::new_v4().simple());
    let now = Utc::now().timestamp_millis();
    state.agent.db.execute(
        "INSERT INTO prompt (id, session_id, parent_prompt_id, user_text, model, status, started_at) \
         VALUES (?, ?, NULL, ?, ?, 'streaming', ?)",
        &[&prompt_id, &session_id, &prompt, &model_entry.model, &now],
    )?;

    // Disposable SSE channel — neither widget nor preview streams to a
    // client. We still need a valid Sender so the metering hook can
    // fire; the receiver is dropped silently.
    let (sse_tx, _sse_rx) = mpsc::channel::<SseEvent>(8);

    let input = RunnerInput {
        state: state.clone(),
        workspace_kind,
        model: model_entry.clone(),
        prompt_id: prompt_id.clone(),
        sse_tx,
        meter_tx: state.agent.meter_tx.clone(),
        schema_hint,
        addendum: addendum_for(kind),
    };

    let runner  = llm::build_runner(&model_entry.backend);
    let summary = runner.run_turn(input, prompt).await;
    let finished_at = Utc::now().timestamp_millis();
    match &summary {
        Ok(s) => {
            state.agent.meter_tx.record(MeterEvent::LlmUsage(LlmUsageRow {
                prompt_id: prompt_id.clone(),
                model:     model_entry.model.clone(),
                tokens_in:  s.tokens_in,
                tokens_out: s.tokens_out,
                latency_ms: s.latency_ms,
            }));
            let _ = state.agent.db.execute(
                "UPDATE prompt SET status = 'done', response_text = ?, finished_at = ? WHERE id = ?",
                &[&s.final_text, &finished_at, &prompt_id],
            );
        }
        Err(e) => {
            let err_msg = e.to_string();
            let _ = state.agent.db.execute(
                "UPDATE prompt SET status = 'error', error = ?, finished_at = ? WHERE id = ?",
                &[&err_msg, &finished_at, &prompt_id],
            );
        }
    }
    let summary = summary?;
    let data = parse_widget_payload(kind, &summary.final_text)?;
    Ok((data, prompt_id))
}

// ── Per-kind output contract appended to the system prompt ───────────────

fn addendum_for(kind: &str) -> String {
    match kind {
        KIND_KPI => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                     of the form `{\"type\":\"kpi\",\"label\":\"...\",\"value\":<number>,\"hint\":\"...\",\
                     \"sparkline\":[<num>, <num>, ...]?}`. \
                     Single number. Optionally include a `sparkline` array of recent values \
                     (most recent last) to show a small trend line beside the number. \
                     No prose, no other content.".into(),
        KIND_BAR => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                     of the form `{\"type\":\"bar\",\"title\":\"...\",\"data\":[{\"label\":\"...\",\"value\":<num>}, ...]}`. \
                     Sort descending by value unless the user explicitly asked otherwise. <= 10 bars. \
                     No prose, no other content.".into(),
        KIND_LINE => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                      of the form `{\"type\":\"line\",\"title\":\"...\",\"data\":[{\"x\":<label>,\"y\":<num>}, ...]}`. \
                      Order data points by x. No prose, no other content.".into(),
        KIND_PIE => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                     of the form `{\"type\":\"pie\",\"title\":\"...\",\"data\":[{\"label\":\"...\",\"value\":<num>}, ...]}`. \
                     <= 6 slices. No prose, no other content.".into(),
        KIND_STACKED_BAR => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                              of the form `{\"type\":\"stacked_bar\",\"title\":\"...\",\
                              \"series\":[\"<seriesA>\",\"<seriesB>\", ...],\
                              \"data\":[{\"label\":\"<row1>\",\"values\":[<num>,<num>, ...]}, ...]}`. \
                              The `values` array per row has the same length and order as `series`. \
                              Use this when each row breaks down into a fixed set of categories \
                              (e.g. brand vs L2 mix). <= 10 rows. No prose.".into(),
        KIND_BULLET => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"bullet\",\"title\":\"...\",\
                         \"data\":[{\"label\":\"<metric>\",\"value\":<num>,\"target\":<num>?,\
                         \"ranges\":[<num>, <num>, ...]?}, ...]}`. \
                         Each row is one metric with the current `value`, an optional `target` \
                         marker, and an optional `ranges` array of qualitative bands (low→high). \
                         Use for actual-vs-policy reads (OH vs max_stock, in-stock % vs target). \
                         <= 6 rows. No prose.".into(),
        KIND_PARETO => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"pareto\",\"title\":\"...\",\
                         \"data\":[{\"label\":\"...\",\"value\":<num>}, ...]}`. \
                         Same shape as `bar` — sort descending by value. The renderer adds the \
                         cumulative-% line on top. Use for 80/20 concentration questions. \
                         <= 15 bars. No prose.".into(),
        KIND_FUNNEL => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"funnel\",\"title\":\"...\",\
                         \"data\":[{\"label\":\"<step>\",\"value\":<num>}, ...]}`. \
                         Each row is a successively narrower step (largest first). Use for \
                         filter cascades (all → matches filter A → AND filter B → actionable). \
                         <= 8 steps. No prose.".into(),
        KIND_GAUGE => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"gauge\",\"title\":\"...\",\"label\":\"...\"?,\
                         \"value\":<num>,\"target\":<num>,\"unit\":\"%\"?,\
                         \"thresholds\":[<low>,<high>]?}`. \
                         Half-arc rendering of `value` against `target`. Optional `thresholds` \
                         is two numbers in the same unit as `value` — below the first = red, \
                         between = amber, above the second = green. Use for service-level / \
                         in-stock / fill-rate reads. No prose.".into(),
        KIND_SPARKLINE => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"sparkline\",\"title\":\"...\",\
                         \"data\":[<num>, <num>, ...]}`. \
                         Compact line chart with no axes — just the trend. Most recent value \
                         last. Use for trend reads where the shape matters more than exact \
                         values. No prose.".into(),
        KIND_HEATMAP => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"heatmap\",\"title\":\"...\",\
                         \"x_labels\":[\"...\", ...],\"y_labels\":[\"...\", ...],\
                         \"data\":[[<num>, ...], [<num>, ...], ...]}`. \
                         `data` is a row-major 2D array sized `y_labels.length × x_labels.length`. \
                         Cells get colored by value (light→dark). Use for any 2D matrix read — \
                         L1 × DC, store-group × week, brand × L2. <= 12×12. No prose.".into(),
        KIND_TREEMAP => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"treemap\",\"title\":\"...\",\
                         \"data\":[{\"label\":\"...\",\"value\":<num>,\
                         \"children\":[{\"label\":\"...\",\"value\":<num>}, ...]?}, ...]}`. \
                         Each top-level entry is a rectangle sized by value; an optional \
                         `children` array subdivides it. One level of nesting, max ~20 \
                         top-level cells. Use for hierarchical share (L1 → L2 inventory, \
                         brand → category mix by value). No prose.".into(),
        KIND_HISTOGRAM => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"histogram\",\"title\":\"...\",\
                         \"x_label\":\"<dim>\"?,\"y_label\":\"count\"?,\
                         \"data\":[<count>, <count>, ...],\
                         \"bin_labels\":[\"<lo-hi>\", ...]?}`. \
                         `data` is bin counts, ordered left-to-right. `bin_labels` aligns \
                         with `data`. Use to show distribution shape — articles-per-brand, \
                         price-per-L2, etc. <= 30 bins. No prose.".into(),
        KIND_SLOPE => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"slope\",\"title\":\"...\",\
                         \"from_label\":\"<period A>\",\"to_label\":\"<period B>\",\
                         \"data\":[{\"label\":\"<entity>\",\"from\":<num>,\"to\":<num>}, ...]}`. \
                         Two-point comparison: each entity is a line from its `from` to `to` \
                         value. Slope direction colors the line (up = green, down = red). \
                         Use for week-over-week / period-over-period reads. <= 15 entities. \
                         No prose.".into(),
        KIND_BOXPLOT => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"boxplot\",\"title\":\"...\",\
                         \"data\":[{\"label\":\"<category>\",\"min\":<num>,\"q1\":<num>,\
                         \"median\":<num>,\"q3\":<num>,\"max\":<num>}, ...]}`. \
                         One horizontal box per category showing the 5-number summary. \
                         Use when the *spread* matters as much as the average — price \
                         distribution per L2, weeks-of-cover per brand. <= 12 boxes. No prose.".into(),
        KIND_WATERFALL => "Reply with EXACTLY one fenced block tagged `chart` containing a JSON object \
                         of the form `{\"type\":\"waterfall\",\"title\":\"...\",\
                         \"data\":[{\"label\":\"<step>\",\"value\":<num>,\"total\":<bool>?}, ...]}`. \
                         Vertical bars showing how a running total builds up. Set \
                         `total: true` on milestone bars (starting balance, ending total) — \
                         those plant on the zero baseline; non-total bars are increments \
                         (positive = green up, negative = red down) stacking on the previous \
                         running total. Use for inventory decomposition (OH − reserved − \
                         in-transit = available) or budget-style breakdowns. <= 12 steps. \
                         No prose.".into(),
        KIND_TABLE => "Reply with EXACTLY one Markdown table (pipe syntax, with a `---` header row). \
                       <= 25 rows. No prose before or after.".into(),
        KIND_TEXT => "Reply with a concise Markdown paragraph or two. Plain prose. No charts, no tables, no fenced blocks.".into(),
        _ => String::new(),
    }
}

// ── Response parsing ─────────────────────────────────────────────────────

/// Extract the renderer-ready payload from the model's final text.
/// - For chart kinds: find the first ```chart fenced block, parse it as
///   JSON, validate `type` matches `kind`.
/// - For `table` / `text`: return the response as-is (markdown).
fn parse_widget_payload(kind: &str, response: &str) -> Result<Value> {
    if kind == KIND_TABLE || kind == KIND_TEXT {
        let trimmed = response.trim();
        if trimmed.is_empty() {
            anyhow::bail!("widget response was empty");
        }
        return Ok(json!({ "markdown": trimmed }));
    }

    // Chart kinds — extract the chart block. The widget's declared
    // `kind` is a *hint* (used for the addendum and the UI title
    // chip); the renderer dispatches on the chart spec's actual
    // `type`. So a widget marked `kpi` whose prompt yields a
    // ranking (and thus a `bar` chart) renders fine, and we don't
    // 500 the run over a hint mismatch. The frontend `ChartBlock`
    // tolerates any of kpi / bar / line / pie regardless of which
    // one the widget was declared as.
    let block = extract_chart_block(response)
        .ok_or_else(|| anyhow!("response did not contain a ```chart fenced block"))?;
    let spec: Value = serde_json::from_str(&block)
        .context("chart block was not valid JSON")?;
    let got_type = spec.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if got_type.is_empty() {
        anyhow::bail!("chart block is missing the required `type` field");
    }
    if got_type != kind {
        tracing::info!(
            widget_kind = %kind,
            chart_type = %got_type,
            "chart type differs from widget kind; rendering by spec type",
        );
    }
    Ok(spec)
}

/// Pull the first ```chart … ``` block out of an LLM response. Returns
/// the body (without the fence) when found. Handles a trailing newline
/// before the closing fence.
fn extract_chart_block(s: &str) -> Option<String> {
    let open = s.find("```chart")?;
    let after_open = &s[open + "```chart".len()..];
    // Skip the rest of the opening fence line.
    let body_start = after_open.find('\n').map(|i| i + 1).unwrap_or(0);
    let body_area = &after_open[body_start..];
    let close = body_area.find("```")?;
    Some(body_area[..close].trim().to_string())
}

/// Stable hash for (kind, prompt) used to invalidate widget_cache when
/// the user edits either via the designer.
fn hash_spec(kind: &str, prompt: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    kind.hash(&mut h);
    prompt.hash(&mut h);
    format!("{:x}", h.finish())
}
