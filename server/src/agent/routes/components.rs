//! Component HTTP routes. Thin wrappers over `agent::components`. The
//! responses include `placeholders` (derived from the template) so the UI
//! doesn't have to re-parse on every render.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::components::{self, CreateArgs, PatchArgs};
use crate::agent::routes::HttpError;
use crate::AppState;

fn enrich(mut row: Value) -> Value {
    if let Some(template) = row.get("prompt_template").and_then(|v| v.as_str()) {
        let ph = components::extract_placeholders(template);
        if let Some(obj) = row.as_object_mut() {
            obj.insert(
                "placeholders".into(),
                Value::Array(ph.into_iter().map(Value::String).collect()),
            );
        }
    }
    row
}

pub async fn list_for_workspace(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let rows = components::list_for_workspace(&state, &workspace_id)
        .map_err(HttpError::internal)?;
    let enriched: Vec<Value> = rows.into_iter().map(enrich).collect();
    Ok(Json(Value::Array(enriched)))
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub kind: String,
    pub prompt_template: String,
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, HttpError> {
    let row = components::create(&state, CreateArgs {
        workspace_id: &workspace_id,
        name: body.name.trim(),
        description: body.description.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()),
        kind: body.kind.trim(),
        prompt_template: &body.prompt_template,
    }).map_err(|e| HttpError::bad_request(e.to_string()))?;
    Ok(Json(enrich(row)))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    let row = components::get(&state, &id)
        .map_err(|_| HttpError::not_found("component not found"))?;
    Ok(Json(enrich(row)))
}

#[derive(Deserialize)]
pub struct PatchBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub prompt_template: Option<String>,
}

pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Value>, HttpError> {
    let row = components::patch(&state, &id, PatchArgs {
        name: body.name,
        description: body.description,
        kind: body.kind,
        prompt_template: body.prompt_template,
    }).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("not found") { HttpError::not_found(msg) }
        else { HttpError::bad_request(msg) }
    })?;
    Ok(Json(enrich(row)))
}

#[derive(Deserialize)]
pub struct PreviewBody {
    pub kind: String,
    pub prompt_template: String,
    #[serde(default)]
    pub placeholder_values: Option<Value>,
}

/// `POST /api/agent/workspaces/:id/components/preview` — run a template
/// with supplied placeholder values, return the renderer-ready payload.
/// Ad-hoc: no component row required, no widget_cache write. Used by the
/// ComponentForm's "Run preview" button.
pub async fn preview(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(body): Json<PreviewBody>,
) -> Result<Json<Value>, HttpError> {
    let values = match body.placeholder_values {
        Some(Value::Object(m)) => m,
        Some(Value::Null) | None => serde_json::Map::new(),
        Some(_) => return Err(HttpError::bad_request("`placeholder_values` must be an object")),
    };
    let data = components::preview(
        state, &workspace_id, &body.kind, &body.prompt_template, &values,
    ).await.map_err(|e| HttpError::bad_request(e.to_string()))?;
    Ok(Json(data))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, HttpError> {
    components::delete(&state, &id)
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") { HttpError::not_found(msg) }
            else { HttpError::internal(e) }
        })?;
    Ok(Json(json!({ "deleted": true })))
}
