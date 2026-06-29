//! Cost derivation — joins raw measurements against the latest
//! `pricing_config` row whose `effective_from <= started_at`. Re-pricing is
//! always non-destructive: a `PATCH /api/agent/pricing` inserts a new row,
//! and historical prompts reprice automatically on next read.

use anyhow::Result;
use serde_json::{json, Value};

use super::super::db::AgentDb;

/// Total cost of a single prompt (tokens + API calls). `None` if the prompt
/// id doesn't exist; `Some(0.0)` is a legitimate value (no calls + no tokens).
pub fn prompt_cost_usd(db: &AgentDb, prompt_id: &str) -> Result<Option<f64>> {
    let usage = db.query(
        "SELECT model, tokens_in, tokens_out FROM llm_usage WHERE prompt_id = ?",
        &[&prompt_id],
    )?;
    let prompt_meta = db.query(
        "SELECT started_at FROM prompt WHERE id = ?",
        &[&prompt_id],
    )?;
    let Some(meta) = prompt_meta.first() else { return Ok(None); };
    let started_at = meta.get("started_at").and_then(|v| v.as_i64()).unwrap_or(0);

    let weights = latest_weights_at(db, started_at)?;

    let mut total = 0.0_f64;

    if let Some(u) = usage.first() {
        let model = u.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let tok_in = u.get("tokens_in").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
        let tok_out = u.get("tokens_out").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
        if let Some(rate) = weights.get("model_rates").and_then(|m| m.get(model)) {
            let in_rate  = rate.get("in_per_1k").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let out_rate = rate.get("out_per_1k").and_then(|v| v.as_f64()).unwrap_or(0.0);
            total += (tok_in / 1000.0) * in_rate;
            total += (tok_out / 1000.0) * out_rate;
        }
    }

    let calls = db.query(
        "SELECT tool_name, duration_ms, bytes_out, status \
         FROM api_call WHERE prompt_id = ?",
        &[&prompt_id],
    )?;
    let per_call    = weights.get("per_call_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let per_ms      = weights.get("per_ms_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let per_byte    = weights.get("per_byte_out_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let multipliers = weights.get("tool_multipliers").cloned().unwrap_or_else(|| Value::Object(Default::default()));
    let default_mult = multipliers.get("default").and_then(|v| v.as_f64()).unwrap_or(1.0);

    for c in calls {
        let tool = c.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
        let dur  = c.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
        let bout = c.get("bytes_out").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
        let status = c.get("status").and_then(|v| v.as_str()).unwrap_or("ok");
        // Cache-hit calls can be charged differently via a dedicated multiplier
        // key (`__cache_hit__`); falls back to the tool's own multiplier.
        let mult_key = if status == "cache_hit" { "__cache_hit__" } else { tool };
        let mult = multipliers
            .get(mult_key)
            .and_then(|v| v.as_f64())
            .or_else(|| multipliers.get(tool).and_then(|v| v.as_f64()))
            .unwrap_or(default_mult);
        let call_cost = (per_call + per_ms * dur + per_byte * bout) * mult;
        total += call_cost;
    }

    Ok(Some(total))
}

/// Component-by-component breakdown of how a prompt's cost was derived.
/// Mirrors `prompt_cost_usd` arithmetic but returns every intermediate so
/// the UI can render "tokens contributed X, tool call Y contributed Z".
/// Returns `None` only when the prompt id doesn't exist.
pub fn prompt_cost_breakdown(db: &AgentDb, prompt_id: &str) -> Result<Option<Value>> {
    let prompt_meta = db.query(
        "SELECT started_at FROM prompt WHERE id = ?",
        &[&prompt_id],
    )?;
    let Some(meta) = prompt_meta.first() else { return Ok(None); };
    let started_at = meta.get("started_at").and_then(|v| v.as_i64()).unwrap_or(0);
    let weights = latest_weights_at(db, started_at)?;

    let per_call    = weights.get("per_call_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let per_ms      = weights.get("per_ms_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let per_byte    = weights.get("per_byte_out_usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let multipliers = weights.get("tool_multipliers").cloned().unwrap_or_else(|| Value::Object(Default::default()));
    let default_mult = multipliers.get("default").and_then(|v| v.as_f64()).unwrap_or(1.0);

    // ── tokens ─────────────────────────────────────────────────────────
    let usage = db.query(
        "SELECT model, tokens_in, tokens_out FROM llm_usage WHERE prompt_id = ?",
        &[&prompt_id],
    )?;
    let mut tokens_section: Value = Value::Null;
    let mut tokens_total = 0.0_f64;
    if let Some(u) = usage.first() {
        let model = u.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let tok_in  = u.get("tokens_in").and_then(|v| v.as_i64()).unwrap_or(0);
        let tok_out = u.get("tokens_out").and_then(|v| v.as_i64()).unwrap_or(0);
        let rate = weights.get("model_rates").and_then(|m| m.get(&model));
        let in_rate  = rate.and_then(|r| r.get("in_per_1k")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let out_rate = rate.and_then(|r| r.get("out_per_1k")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let in_cost  = (tok_in as f64 / 1000.0) * in_rate;
        let out_cost = (tok_out as f64 / 1000.0) * out_rate;
        tokens_total = in_cost + out_cost;
        tokens_section = json!({
            "model":           model,
            "tokens_in":       tok_in,
            "tokens_out":      tok_out,
            "in_per_1k_usd":   in_rate,
            "out_per_1k_usd":  out_rate,
            "rate_found":      rate.is_some(),
            "input_cost_usd":  in_cost,
            "output_cost_usd": out_cost,
            "subtotal_usd":    tokens_total,
        });
    }

    // ── tool calls ─────────────────────────────────────────────────────
    let calls = db.query(
        "SELECT id, tool_name, duration_ms, bytes_out, status \
         FROM api_call WHERE prompt_id = ? ORDER BY started_at",
        &[&prompt_id],
    )?;
    let mut calls_breakdown: Vec<Value> = Vec::new();
    let mut calls_total = 0.0_f64;
    for c in calls {
        let id   = c.get("id").cloned().unwrap_or(Value::Null);
        let tool = c.get("tool_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let dur  = c.get("duration_ms").and_then(|v| v.as_i64()).unwrap_or(0);
        let bout = c.get("bytes_out").and_then(|v| v.as_i64()).unwrap_or(0);
        let status = c.get("status").and_then(|v| v.as_str()).unwrap_or("ok").to_string();

        let mult_key = if status == "cache_hit" { "__cache_hit__" } else { tool.as_str() };
        let mult = multipliers
            .get(mult_key)
            .and_then(|v| v.as_f64())
            .or_else(|| multipliers.get(&tool).and_then(|v| v.as_f64()))
            .unwrap_or(default_mult);

        let base_call   = per_call;
        let ms_cost     = per_ms * dur as f64;
        let bytes_cost  = per_byte * bout as f64;
        let pre_mult    = base_call + ms_cost + bytes_cost;
        let post_mult   = pre_mult * mult;
        calls_total += post_mult;

        calls_breakdown.push(json!({
            "api_call_id":      id,
            "tool":             tool,
            "status":           status,
            "duration_ms":      dur,
            "bytes_out":        bout,
            "multiplier":       mult,
            "multiplier_key":   mult_key,
            "base_call_usd":    base_call,
            "ms_cost_usd":      ms_cost,
            "bytes_cost_usd":   bytes_cost,
            "pre_multiplier_usd":  pre_mult,
            "post_multiplier_usd": post_mult,
        }));
    }

    Ok(Some(json!({
        "total_usd": tokens_total + calls_total,
        "tokens":    tokens_section,
        "tokens_subtotal_usd": tokens_total,
        "calls":     calls_breakdown,
        "calls_subtotal_usd":  calls_total,
        "weights": {
            "per_call_usd":     per_call,
            "per_ms_usd":       per_ms,
            "per_byte_out_usd": per_byte,
            "default_multiplier": default_mult,
        },
        "pricing_effective_at": started_at,
    })))
}

fn latest_weights_at(db: &AgentDb, ts_ms: i64) -> Result<Value> {
    let row = db.query(
        "SELECT weights FROM pricing_config \
         WHERE effective_from <= ? \
         ORDER BY effective_from DESC LIMIT 1",
        &[&ts_ms],
    )?;
    if let Some(r) = row.first() {
        if let Some(w) = r.get("weights") {
            return Ok(w.clone());
        }
    }
    Ok(Value::Object(Default::default()))
}
