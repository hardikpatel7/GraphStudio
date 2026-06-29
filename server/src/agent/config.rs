//! Boot-time seeding for the agent DB.
//!
//! - `seed_pricing_config`: inserts the initial pricing-weights row if the
//!   table is empty. Source is `agent::config::default_weights()` — kept in
//!   code rather than a TOML file so a fresh tenant works without external
//!   files; admin edits replace the row via `PATCH /api/agent/pricing`.
//! - `seed_model_allowlist`: idempotently upserts the default model entries
//!   (OpenAI v1; Anthropic stays out until the adapter ships).

use anyhow::Result;
use chrono::Utc;
use serde_json::json;

use super::db::AgentDb;

pub fn default_weights() -> serde_json::Value {
    json!({
        "per_call_usd":      0.0005,
        "per_ms_usd":        0.000001,
        "per_byte_out_usd":  1e-9,
        "tool_multipliers":  {
            "graph_cross_filter": 3.0,
            "graph_traverse":     2.0,
            "dataview_read":      2.0,
            "clickhouse_query":   5.0,
            "duckdb_query":       2.0,
            "default":            1.0
        },
        "model_rates": {
            // 128K-context legacy. Prone to context_length_exceeded on
            // tool-heavy multi-turn runs — prefer gpt-4.1 / gpt-5 family.
            "gpt-4o":          { "in_per_1k": 0.005,   "out_per_1k": 0.015 },
            "gpt-4o-mini":     { "in_per_1k": 0.00015, "out_per_1k": 0.0006 },
            // 1M-context GPT-4.1 family.
            "gpt-4.1":         { "in_per_1k": 0.002,   "out_per_1k": 0.008 },
            "gpt-4.1-mini":    { "in_per_1k": 0.0004,  "out_per_1k": 0.0016 },
            "gpt-4.1-nano":    { "in_per_1k": 0.0001,  "out_per_1k": 0.0004 },
            // GPT-5 family (400K ctx). Rates from OpenAI's Aug 2025 price
            // sheet, converted from per-1M to per-1K.
            "gpt-5":           { "in_per_1k": 0.00125, "out_per_1k": 0.01   },
            "gpt-5-mini":      { "in_per_1k": 0.00025, "out_per_1k": 0.002  },
            "gpt-5-nano":      { "in_per_1k": 0.00005, "out_per_1k": 0.0004 },
            "gpt-5-pro":       { "in_per_1k": 0.015,   "out_per_1k": 0.12   },
            // Anthropic Claude 4.x family. 200K ctx (1M ctx beta on
            // Sonnet 4.6 / Opus 4.7 via the long-context header).
            // Rates from Anthropic's published per-1M list prices.
            "claude-opus-4-7":   { "in_per_1k": 0.015,   "out_per_1k": 0.075  },
            "claude-opus-4-6":   { "in_per_1k": 0.015,   "out_per_1k": 0.075  },
            "claude-sonnet-4-6": { "in_per_1k": 0.003,   "out_per_1k": 0.015  },
            "claude-haiku-4-5":  { "in_per_1k": 0.001,   "out_per_1k": 0.005  }
        },
        "cpu_per_ms_usd":   0,
        "mem_per_mbms_usd": 0
    })
}

pub fn seed_pricing_config(db: &AgentDb) -> Result<()> {
    let existing = db.query(
        "SELECT id, weights FROM pricing_config ORDER BY effective_from DESC LIMIT 1",
        &[],
    )?;
    let now = Utc::now().timestamp_millis();

    // Empty table → first-boot seed.
    if existing.is_empty() {
        db.execute(
            "INSERT INTO pricing_config (effective_from, weights, notes) VALUES (?, ?, ?)",
            &[&now, &default_weights().to_string(), &"seed"],
        )?;
        return Ok(());
    }

    // Latest row exists. If it's missing the newest family rates we ship
    // now, append a new pricing_config row (append-only history pattern)
    // with the merged weights so cost tracking works for the new models on
    // tenants that booted before this change. The canary is the newest
    // model we expect to be present — bump it whenever default_weights
    // gains a new family.
    let latest = &existing[0];
    let weights_raw = latest.get("weights");
    let mut weights: serde_json::Value = match weights_raw {
        Some(serde_json::Value::Object(_)) => weights_raw.cloned().unwrap_or(json!({})),
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str(s).unwrap_or_else(|_| json!({}))
        }
        _ => json!({}),
    };
    let needs_patch = !weights
        .get("model_rates")
        .and_then(|v| v.get("claude-opus-4-7"))
        .is_some();
    if needs_patch {
        let defaults = default_weights();
        if let (Some(defaults_rates), Some(rates_slot)) = (
            defaults.get("model_rates").cloned(),
            weights.get_mut("model_rates").and_then(|v| v.as_object_mut()),
        ) {
            if let Some(defaults_obj) = defaults_rates.as_object() {
                for (k, v) in defaults_obj {
                    rates_slot.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        } else {
            // Existing row didn't even have a model_rates object.
            if let Some(obj) = weights.as_object_mut() {
                if let Some(defaults_rates) = defaults.get("model_rates") {
                    obj.insert("model_rates".into(), defaults_rates.clone());
                }
            }
        }
        db.execute(
            "INSERT INTO pricing_config (effective_from, weights, notes) VALUES (?, ?, ?)",
            &[
                &now,
                &weights.to_string(),
                &"auto-patch: refresh model_rates with latest defaults",
            ],
        )?;
    }
    Ok(())
}

/// Idempotently seed one workspace per kind. v1 only wires Inventory to a
/// real backend; the other four exist so the UI can render their landing
/// cards as "Backend not yet configured" empty states. Names are stable
/// strings the UI reads directly.
pub fn seed_workspaces(db: &AgentDb) -> Result<()> {
    let kinds = [
        ("ws_inventory", "inventory", "Inventory"),
        ("ws_item",      "item",      "Item"),
        ("ws_pricing",   "pricing",   "Pricing"),
        ("ws_assort",    "assort",    "Assort"),
        ("ws_plan",      "plan",      "Plan"),
    ];
    let now = Utc::now().timestamp_millis();
    for (id, kind, name) in kinds {
        db.execute(
            "INSERT OR IGNORE INTO workspace (id, kind, name, config_json, created_at) \
             VALUES (?, ?, ?, '{}', ?)",
            &[&id, &kind, &name, &now],
        )?;
    }
    Ok(())
}

/// Default tool sets per workspace kind. `INSERT OR IGNORE` — once a row
/// exists it's not overwritten on subsequent boots, so admins who narrow
/// or widen the mapping via SQL keep their edits across restarts. To reset,
/// delete the rows for the kind and restart.
///
/// Defaults:
/// - inventory: dataview/graph/source/duckdb/filter tools (NOT clickhouse_*)
/// - item:      clickhouse_query + clickhouse_dictionary + list_connections
/// - pricing/assort/plan: empty (UI shows "Backend not yet configured")
pub fn seed_workspace_kind_tools(db: &AgentDb) -> Result<()> {
    let rows: &[(&str, &str)] = &[
        ("inventory", "list_dataviews"),
        ("inventory", "describe_dataview"),
        ("inventory", "introspect_dataview"),
        ("inventory", "dataview_read"),
        ("inventory", "list_graphs"),
        ("inventory", "describe_graph"),
        ("inventory", "graph_node"),
        ("inventory", "graph_traverse"),
        ("inventory", "graph_cross_filter"),
        ("inventory", "list_sources"),
        ("inventory", "describe_source"),
        ("inventory", "duckdb_query"),
        ("inventory", "resolve_filter_values"),

        ("item", "list_connections"),
        ("item", "clickhouse_query"),
        // `clickhouse_dictionary` intentionally omitted from the v1 Item
        // workspace. Even with a `database` arg the output can blow the
        // 60K-byte cap on tenants with many tables — the model then
        // operates from a truncated schema and queries blindly. Use the
        // SHOW DATABASES → SHOW TABLES → DESCRIBE → SELECT LIMIT 3
        // chain instead; each step returns small, targeted output.
        // Admins can re-enable per-kind via an `INSERT INTO
        // workspace_kind_tools (kind, tool_name) VALUES ('item',
        // 'clickhouse_dictionary')` row.
    ];
    for (kind, tool) in rows {
        db.execute(
            "INSERT OR IGNORE INTO workspace_kind_tools (kind, tool_name) VALUES (?, ?)",
            &[kind, tool],
        )?;
    }
    Ok(())
}

/// Rewrite `<name>` placeholder tokens to the new `{{name}}` syntax in
/// every `component.prompt_template` and every widget node inside
/// `dashboard.layout_json`. Runs every boot — idempotent because the
/// re-applied regex only catches `<name>` patterns that match the
/// strict grammar, and after a successful migration there are none
/// left. Behind the change: the old `<name>` syntax collided with
/// literal angle brackets in SQL operators / JSON shape examples
/// (e.g. a prompt that documented JSON as `{"label":"<l1_name>"}`
/// silently turned `<l1_name>` into a required placeholder).
pub fn migrate_placeholders_to_braces(db: &AgentDb) -> Result<()> {
    // Strict-grammar rewrite: `<` + `[A-Za-z_][A-Za-z0-9_]*` + `>`.
    fn rewrite(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'<' { out.push(bytes[i] as char); i += 1; continue; }
            let start = i + 1;
            let mut j = start;
            if j >= bytes.len() || !(bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
                out.push(bytes[i] as char); i += 1; continue;
            }
            j += 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'>' {
                let name = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
                if !name.is_empty() {
                    out.push_str("{{");
                    out.push_str(name);
                    out.push_str("}}");
                    i = j + 1;
                    continue;
                }
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    // Components: rewrite prompt_template in-place.
    let comps = db.query("SELECT id, prompt_template FROM component", &[])?;
    for c in comps {
        let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let tmpl = c.get("prompt_template").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() || tmpl.is_empty() { continue; }
        let next = rewrite(tmpl);
        if next != tmpl {
            db.execute(
                "UPDATE component SET prompt_template = ?, updated_at = ? WHERE id = ?",
                &[&next, &chrono::Utc::now().timestamp_millis(), &id],
            )?;
        }
    }

    // Dashboards: rewrite every widget node's `prompt` field inside
    // layout_json. The substitution only touches strings inside `prompt`
    // properties so it's safe against unrelated `<` in titles, etc.
    let dashes = db.query("SELECT id, layout_json FROM dashboard", &[])?;
    for d in dashes {
        let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let raw = match d.get("layout_json") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(v)                            => v.to_string(),
            None                               => continue,
        };
        if id.is_empty() || raw.is_empty() { continue; }
        let mut parsed: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v, Err(_) => continue,
        };
        let mut changed = false;
        fn walk(v: &mut serde_json::Value, changed: &mut bool, rewrite: &dyn Fn(&str) -> String) {
            match v {
                serde_json::Value::Object(m) => {
                    let is_widget = m.get("type").and_then(|t| t.as_str()) == Some("widget");
                    if is_widget {
                        if let Some(serde_json::Value::String(p)) = m.get_mut("prompt") {
                            let next = rewrite(p);
                            if next != *p { *p = next; *changed = true; }
                        }
                        if let Some(serde_json::Value::String(t)) = m.get_mut("title") {
                            let next = rewrite(t);
                            if next != *t { *t = next; *changed = true; }
                        }
                    }
                    for (_, child) in m.iter_mut() { walk(child, changed, rewrite); }
                }
                serde_json::Value::Array(a) => { for x in a.iter_mut() { walk(x, changed, rewrite); } }
                _ => {}
            }
        }
        walk(&mut parsed, &mut changed, &rewrite);
        if changed {
            let next = serde_json::to_string(&parsed).unwrap_or(raw);
            db.execute(
                "UPDATE dashboard SET layout_json = ?, updated_at = ? WHERE id = ?",
                &[&next, &chrono::Utc::now().timestamp_millis(), &id],
            )?;
        }
    }
    Ok(())
}

/// One-time cleanup of deprecated workspace_kind_tools rows. Idempotent —
/// each row is a `DELETE` that's a no-op when already absent. Add new
/// entries here when retiring a tool from the default seed; admins who
/// want to keep an entry can re-insert it manually after restart.
pub fn cleanup_deprecated_tools(db: &AgentDb) -> Result<()> {
    // `clickhouse_dictionary` retired from the default Item seed (see
    // `seed_workspace_kind_tools`). Pulls the live row so existing
    // tenants stop offering it after the next boot.
    db.execute(
        "DELETE FROM workspace_kind_tools \
         WHERE kind = 'item' AND tool_name = 'clickhouse_dictionary'",
        &[],
    )?;
    Ok(())
}

pub fn seed_model_allowlist(db: &AgentDb) -> Result<()> {
    // INSERT OR IGNORE — leaves edits made via the API alone on subsequent boots.
    // Listed top-down by recency / preference:
    //   - Claude 4 family (Anthropic) — strongest tool use, 200K ctx
    //   - GPT-5 family (OpenAI, 400K ctx)
    //   - GPT-4.1 family (1M ctx — still useful for very long transcripts)
    //   - 128K legacy GPT-4o
    let rows = [
        ("anthropic", "claude-opus-4-7",   "Claude Opus 4.7 (200K ctx)",   "rig"),
        ("anthropic", "claude-opus-4-6",   "Claude Opus 4.6 (200K ctx)",   "rig"),
        ("anthropic", "claude-sonnet-4-6", "Claude Sonnet 4.6 (200K ctx)", "rig"),
        ("anthropic", "claude-haiku-4-5",  "Claude Haiku 4.5 (200K ctx)",  "rig"),
        ("openai",    "gpt-5",             "GPT-5 (400K ctx)",             "rig"),
        ("openai",    "gpt-5-mini",        "GPT-5 mini (400K ctx)",        "rig"),
        ("openai",    "gpt-5-nano",        "GPT-5 nano (400K ctx)",        "rig"),
        ("openai",    "gpt-5-pro",         "GPT-5 pro (400K ctx)",         "rig"),
        ("openai",    "gpt-4.1",           "GPT-4.1 (1M ctx)",             "rig"),
        ("openai",    "gpt-4.1-mini",      "GPT-4.1 mini (1M ctx)",        "rig"),
        ("openai",    "gpt-4.1-nano",      "GPT-4.1 nano (1M ctx)",        "rig"),
        ("openai",    "gpt-4o",            "GPT-4o (128K ctx)",            "rig"),
        ("openai",    "gpt-4o-mini",       "GPT-4o mini (128K ctx)",       "rig"),
    ];
    for (provider, model, display, backend) in rows {
        db.execute(
            "INSERT OR IGNORE INTO model_allowlist (provider, model, display_name, backend, enabled) \
             VALUES (?, ?, ?, ?, 1)",
            &[&provider, &model, &display, &backend],
        )?;
    }
    // Refresh display labels on existing rows so the (ctx) hints show up
    // for tenants that booted before this change.
    let label_updates: &[(&str, &str)] = &[
        ("claude-opus-4-7",   "Claude Opus 4.7 (200K ctx)"),
        ("claude-opus-4-6",   "Claude Opus 4.6 (200K ctx)"),
        ("claude-sonnet-4-6", "Claude Sonnet 4.6 (200K ctx)"),
        ("claude-haiku-4-5",  "Claude Haiku 4.5 (200K ctx)"),
        ("gpt-5",             "GPT-5 (400K ctx)"),
        ("gpt-5-mini",        "GPT-5 mini (400K ctx)"),
        ("gpt-5-nano",        "GPT-5 nano (400K ctx)"),
        ("gpt-5-pro",         "GPT-5 pro (400K ctx)"),
        ("gpt-4.1",           "GPT-4.1 (1M ctx)"),
        ("gpt-4.1-mini",      "GPT-4.1 mini (1M ctx)"),
        ("gpt-4.1-nano",      "GPT-4.1 nano (1M ctx)"),
        ("gpt-4o",            "GPT-4o (128K ctx)"),
        ("gpt-4o-mini",       "GPT-4o mini (128K ctx)"),
    ];
    for (model, display) in label_updates {
        db.execute(
            "UPDATE model_allowlist SET display_name = ? WHERE model = ?",
            &[display, model],
        )?;
    }
    Ok(())
}
