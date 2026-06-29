//! Reusable widget components.
//!
//! A component bundles a widget `kind` with a prompt TEMPLATE — a string
//! containing zero or more `<placeholder>` tokens. Dashboard widgets can
//! reference a component by id and provide per-instance values for each
//! placeholder, so the same component renders different views when used in
//! different dashboards (e.g. "Top 10 articles by <metric> for <brand>"
//! reused for brand A revenue, brand B units, etc.).
//!
//! Storage is per-workspace because prompts are workspace-shaped (each
//! kind has its own catalog + tools). Components are *not* mutated by
//! widgets — dashboards store the component_id + their own placeholder
//! values; editing the template propagates to every dashboard using it on
//! the next widget run.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::AppState;

// ── CRUD ─────────────────────────────────────────────────────────────────

pub fn list_for_workspace(state: &AppState, workspace_id: &str) -> Result<Vec<Value>> {
    state.agent.db.query(
        "SELECT id, workspace_id, name, description, kind, prompt_template, created_at, updated_at \
         FROM component WHERE workspace_id = ? ORDER BY updated_at DESC",
        &[&workspace_id],
    )
}

pub fn get(state: &AppState, id: &str) -> Result<Value> {
    state.agent.db.query_one(
        "SELECT * FROM component WHERE id = ?",
        &[&id],
    )
}

pub struct CreateArgs<'a> {
    pub workspace_id: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub kind: &'a str,
    pub prompt_template: &'a str,
}

pub fn create(state: &AppState, args: CreateArgs<'_>) -> Result<Value> {
    if args.name.trim().is_empty() {
        return Err(anyhow!("name is required"));
    }
    if args.prompt_template.trim().is_empty() {
        return Err(anyhow!("prompt_template is required"));
    }
    if !KNOWN_KINDS.contains(&args.kind) {
        return Err(anyhow!(
            "kind must be one of {} (got `{}`)",
            KNOWN_KINDS.join(", "),
            args.kind
        ));
    }
    let id = format!("cmp_{}", Uuid::new_v4().simple());
    let now = Utc::now().timestamp_millis();
    state.agent.db.execute(
        "INSERT INTO component (id, workspace_id, name, description, kind, prompt_template, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            &id, &args.workspace_id, &args.name, &args.description, &args.kind,
            &args.prompt_template, &now, &now,
        ],
    )?;
    get(state, &id)
}

#[derive(Default)]
pub struct PatchArgs {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub kind: Option<String>,
    pub prompt_template: Option<String>,
}

pub fn patch(state: &AppState, id: &str, args: PatchArgs) -> Result<Value> {
    use rusqlite::types::ToSql;
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(n) = args.name {
        if n.trim().is_empty() { return Err(anyhow!("name cannot be empty")); }
        sets.push("name = ?"); vals.push(Box::new(n));
    }
    if let Some(d) = args.description {
        sets.push("description = ?"); vals.push(Box::new(d));
    }
    if let Some(k) = args.kind {
        if !KNOWN_KINDS.contains(&k.as_str()) {
            return Err(anyhow!("invalid kind `{k}`"));
        }
        sets.push("kind = ?"); vals.push(Box::new(k));
    }
    if let Some(t) = args.prompt_template {
        if t.trim().is_empty() { return Err(anyhow!("prompt_template cannot be empty")); }
        sets.push("prompt_template = ?"); vals.push(Box::new(t));
    }
    if sets.is_empty() {
        return get(state, id);
    }
    let now = Utc::now().timestamp_millis();
    sets.push("updated_at = ?"); vals.push(Box::new(now));
    let sql = format!("UPDATE component SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.to_string()));
    let refs: Vec<&dyn ToSql> = vals.iter().map(|b| b.as_ref()).collect();
    let n = state.agent.db.execute(&sql, &refs)?;
    if n == 0 { return Err(anyhow!("component not found")); }
    get(state, id)
}

pub fn delete(state: &AppState, id: &str) -> Result<()> {
    // Dashboards reference components by id but the binding is soft —
    // they store component_id + values in their layout JSON. Deleting a
    // component doesn't cascade; widgets that referenced it surface a
    // "component missing" error at run time instead.
    let n = state.agent.db.execute(
        "DELETE FROM component WHERE id = ?",
        &[&id],
    )?;
    if n == 0 { return Err(anyhow!("component not found")); }
    Ok(())
}

// ── Placeholder utilities ────────────────────────────────────────────────

/// Pull every `{{name}}` placeholder out of a template string. Returns
/// distinct names in first-occurrence order. The grammar is strict:
/// `{{` followed by `[A-Za-z_][A-Za-z0-9_]*` followed by `}}`. Anything
/// else (literal JSON braces, Rust format strings, etc.) is ignored.
///
/// Switched from `<name>` (which collided with SQL `<`, HTML, JSON
/// shape examples that contained `<l1_name>` etc.) to Handlebars-style
/// `{{name}}` because that grammar doesn't appear naturally in prompt
/// bodies. Existing dashboards / components were migrated in-place at
/// boot by `migrate_placeholders_to_braces`.
pub fn extract_placeholders(template: &str) -> Vec<String> {
    let bytes = template.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'{' || bytes[i + 1] != b'{' { i += 1; continue; }
        let start = i + 2;
        let mut j = start;
        if j >= bytes.len() || !(bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
            i += 1; continue;
        }
        j += 1;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        if j + 1 < bytes.len() && bytes[j] == b'}' && bytes[j + 1] == b'}' {
            let name = std::str::from_utf8(&bytes[start..j]).unwrap_or("").to_string();
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
            i = j + 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Substitute `{{name}}` placeholders in a template with values from a map.
/// Unknown placeholders are left as-is and reported in the returned vec
/// (caller can decide to error or proceed with the partial substitution).
pub fn substitute(template: &str, values: &serde_json::Map<String, Value>) -> (String, Vec<String>) {
    let names = extract_placeholders(template);
    let mut out = template.to_string();
    let mut missing: Vec<String> = Vec::new();
    for name in names {
        let token = format!("{{{{{name}}}}}");
        let val = values.get(&name).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b)   => Some(b.to_string()),
            Value::Null      => None,
            _                => Some(v.to_string()),
        });
        match val {
            Some(replacement) => { out = out.replace(&token, &replacement); }
            None              => { missing.push(name); }
        }
    }
    (out, missing)
}

const KNOWN_KINDS: &[&str] = &[
    "kpi", "bar", "line", "pie",
    "stacked_bar", "bullet", "pareto", "funnel",
    "gauge", "sparkline",
    "heatmap", "treemap", "histogram", "slope",
    "boxplot", "waterfall",
    "table", "text",
];

// ── Preview ──────────────────────────────────────────────────────────────

/// Stable title for the per-workspace "preview" session. One session per
/// workspace; every component preview run becomes a prompt in it, so the
/// cost shows up in the workspace's stats dashboard and the usual
/// prompt-detail drawer can be used to debug a preview.
const PREVIEW_SESSION_TITLE: &str = "_component_preview · widgets";
const DEFAULT_PREVIEW_MODEL: &str = "gpt-4o-mini";

/// Run a component template with placeholder values supplied at the call
/// site — *without* persisting a component row or writing to widget_cache.
/// Used by the ComponentForm's "Run preview" button so users can iterate
/// on templates before saving and verify a saved component against real
/// values without binding it to a dashboard.
///
/// Returns the same payload shape as `dashboards::run_widget`: a chart
/// spec (`kpi`/`bar`/`line`/`pie`) or `{ "markdown": "..." }` for `table`
/// and `text`.
pub async fn preview(
    state: Arc<AppState>,
    workspace_id: &str,
    kind: &str,
    prompt_template: &str,
    placeholder_values: &Map<String, Value>,
) -> Result<Value> {
    if !KNOWN_KINDS.contains(&kind) {
        return Err(anyhow!("invalid kind `{kind}`"));
    }
    let (resolved_prompt, missing) = substitute(prompt_template, placeholder_values);
    if !missing.is_empty() {
        return Err(anyhow!(
            "missing values for placeholder(s): {}. Fill them in and run preview again.",
            missing.join(", ")
        ));
    }
    if resolved_prompt.trim().is_empty() {
        return Err(anyhow!("prompt template is empty"));
    }

    let session_id = get_or_create_preview_session(&state, workspace_id)?;

    // Reuse the dashboard runner's core pipeline so previews share
    // metering, schema injection, output contract, tool catalog, etc.
    let started_at = std::time::Instant::now();
    let (data, prompt_id) =
        super::dashboards::run_kind_prompt(state.clone(), workspace_id, &session_id, kind, &resolved_prompt)
            .await?;
    let wall_ms = started_at.elapsed().as_millis() as i64;
    let cost_usd = super::meter::pricing::prompt_cost_usd(&state.agent.db, &prompt_id)
        .ok()
        .flatten();
    let llm_latency_ms = state.agent.db.query_one(
        "SELECT latency_ms FROM llm_usage WHERE prompt_id = ?",
        &[&prompt_id],
    )
    .ok()
    .and_then(|r| r.get("latency_ms").and_then(|v| v.as_i64()));
    let fetched_at = chrono::Utc::now().timestamp_millis();
    Ok(serde_json::json!({
        "data": data,
        "meta": {
            "prompt_id":  prompt_id,
            "wall_ms":    wall_ms,
            "llm_ms":     llm_latency_ms,
            "cost_usd":   cost_usd,
            "fetched_at": fetched_at,
            "from_cache": false,
        },
    }))
}

/// Find-or-create the workspace's preview session. Stable on title so
/// concurrent first-use is idempotent (worst case: two sessions get
/// created and the older one is reused on next call). We don't need a
/// CHECK constraint — title collisions are fine; the session is purely
/// internal.
fn get_or_create_preview_session(state: &AppState, workspace_id: &str) -> Result<String> {
    if let Ok(row) = state.agent.db.query_one(
        "SELECT id FROM session WHERE workspace_id = ? AND title = ? \
         ORDER BY created_at DESC LIMIT 1",
        &[&workspace_id, &PREVIEW_SESSION_TITLE],
    ) {
        if let Some(id) = row.get("id").and_then(|v| v.as_str()) {
            return Ok(id.to_string());
        }
    }
    // None yet — create one. Pick any enabled model; the preview session
    // model is informational only (`run_kind_prompt` falls back to it
    // when nothing else specifies a model).
    let model_row = state.agent.db.query_one(
        "SELECT provider, model FROM model_allowlist \
         WHERE enabled = 1 ORDER BY (model = ?) DESC, provider, model LIMIT 1",
        &[&DEFAULT_PREVIEW_MODEL],
    ).context("no enabled models in allowlist")?;
    let provider = model_row.get("provider").and_then(|v| v.as_str()).unwrap_or("openai");
    let model    = model_row.get("model")   .and_then(|v| v.as_str()).unwrap_or(DEFAULT_PREVIEW_MODEL);

    let id = format!("sess_{}", Uuid::new_v4().simple());
    let now = Utc::now().timestamp_millis();
    state.agent.db.execute(
        "INSERT INTO session (id, workspace_id, provider, model, title, provider_state, created_at, last_active_at) \
         VALUES (?, ?, ?, ?, ?, NULL, ?, ?)",
        &[&id, &workspace_id, &provider, &model, &PREVIEW_SESSION_TITLE, &now, &now],
    )?;
    Ok(id)
}

use anyhow::Context;
