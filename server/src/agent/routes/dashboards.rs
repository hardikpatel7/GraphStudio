//! Dashboard HTTP routes — thin wrappers over `agent::dashboards`. All
//! routes return `Json<Value>`; errors translate through the existing
//! `HttpError` shape so the UI's response handling stays uniform.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use futures::future::join_all;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::dashboards::{self, PatchArgs};
use crate::agent::routes::HttpError;
use crate::AppState;

// ── list / create under /workspaces/:id/dashboards ───────────────────────

pub async fn list_for_workspace(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let rows = dashboards::list_for_workspace(&state, &workspace_id)
        .map_err(HttpError::internal)?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, HttpError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(HttpError::bad_request("`name` is required"));
    }
    let row = dashboards::create(&state, &workspace_id, name, body.description.as_deref())
        .map_err(HttpError::internal)?;
    Ok(Json(row))
}

// ── single dashboard CRUD under /dashboards/:id ──────────────────────────

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let row = dashboards::get(&state, &id)
        .map_err(|_| HttpError::not_found("dashboard not found"))?;
    // Bundle in widget cache so the view route is one round trip.
    let cache = dashboards::widget_cache(&state, &id).unwrap_or_default();
    let mut out = row;
    if let Some(obj) = out.as_object_mut() {
        obj.insert("widgets".into(), Value::Array(cache));
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct PatchBody {
    #[serde(default)]
    pub name: Option<String>,
    // Two-level Option so the client can explicitly null the description.
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub layout_json: Option<Value>,
    /// Swap the model used for every future widget run. Validated
    /// against `model_allowlist` server-side; mismatched values come
    /// back as 400.
    #[serde(default)]
    pub model: Option<String>,
}

// Honor `description: null` vs the field being absent — only the former
// should clear the value in the row.
fn deserialize_optional_field<'de, D>(deserializer: D) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    Option::<Option<String>>::deserialize(deserializer)
}

pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Value>, HttpError> {
    let args = PatchArgs {
        name: body.name.and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }),
        description: body.description,
        layout_json: match body.layout_json {
            Some(v) => Some(serde_json::to_string(&v).map_err(HttpError::internal)?),
            None    => None,
        },
        model: body.model.and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }),
    };
    let row = dashboards::patch(&state, &id, args)
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") { HttpError::not_found(msg) }
            else if msg.contains("invalid JSON") { HttpError::bad_request(msg) }
            else if msg.contains("not enabled in the allowlist") { HttpError::bad_request(msg) }
            else { HttpError::internal(e) }
        })?;
    Ok(Json(row))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    dashboards::delete(&state, &id)
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") { HttpError::not_found(msg) }
            else { HttpError::internal(e) }
        })?;
    Ok(Json(json!({ "deleted": true })))
}

// ── widget runner under /dashboards/:id/widgets/:node_id/run ─────────────

#[derive(Deserialize, Default)]
pub struct RunWidgetBody {
    /// Drill-down overrides. Merged on top of the widget node's
    /// stored `placeholder_values` for this single run; never
    /// persisted. Empty → behaves identically to a normal refresh
    /// (cache write included).
    #[serde(default)]
    pub placeholder_overrides: serde_json::Map<String, Value>,
}

pub async fn run_widget(
    State(state): State<Arc<AppState>>,
    Path((dashboard_id, node_id)): Path<(String, String)>,
    body: Option<Json<RunWidgetBody>>,
) -> Result<Json<Value>, HttpError> {
    let overrides = body.map(|b| b.0.placeholder_overrides).unwrap_or_default();
    let data = dashboards::run_widget_with_overrides(state, &dashboard_id, &node_id, overrides)
        .await
        .map_err(|e| HttpError::internal(e))?;
    Ok(Json(data))
}

// ── refresh-all / refresh-subtree under /dashboards/:id/refresh ──────────

#[derive(Deserialize, Default)]
pub struct RefreshBody {
    /// Optional subtree root. When set, only widgets under that node are
    /// refreshed. Omit (or pass `"root"`) to refresh every widget in the
    /// dashboard.
    #[serde(default)]
    pub subtree_id: Option<String>,
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Path(dashboard_id): Path<String>,
    Json(body): Json<RefreshBody>,
) -> Result<Json<Value>, HttpError> {
    let dash = dashboards::get(&state, &dashboard_id)
        .map_err(|_| HttpError::not_found("dashboard not found"))?;
    let layout_str = dash.get("layout_json").and_then(|v| v.as_str()).unwrap_or("");
    let layout: Value = if layout_str.is_empty() {
        dash.get("layout_json").cloned().unwrap_or(json!({}))
    } else {
        serde_json::from_str(layout_str).map_err(HttpError::internal)?
    };

    let node_ids = dashboards::collect_widget_ids(&layout, body.subtree_id.as_deref());
    if node_ids.is_empty() {
        return Ok(Json(json!({ "ran": 0, "errors": [] })));
    }

    // Cap fan-out to 4 to play nice with the upstream provider's
    // per-account rate limits; surplus widgets queue.
    let chunks = node_ids.chunks(4);
    let mut errors: Vec<Value> = Vec::new();
    let mut ran = 0usize;
    for chunk in chunks {
        let futs = chunk.iter().map(|id| {
            let state = state.clone();
            let dash_id = dashboard_id.clone();
            let nid = id.clone();
            async move {
                let res = dashboards::run_widget(state, &dash_id, &nid).await;
                (nid, res)
            }
        });
        for (nid, res) in join_all(futs).await {
            match res {
                Ok(_) => ran += 1,
                Err(e) => errors.push(json!({ "node_id": nid, "error": e.to_string() })),
            }
        }
    }

    Ok(Json(json!({
        "ran":    ran,
        "errors": errors,
    })))
}
