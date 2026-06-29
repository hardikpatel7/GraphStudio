//! Agent HTTP routes mounted under `/api/agent/*` from `main.rs`.

use std::sync::Arc;

use axum::{
    extract::State,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde_json::json;

use crate::AppState;

pub mod components;
pub mod dashboards;
pub mod models;
pub mod pricing;
pub mod prompts;
pub mod sessions;
pub mod workspaces;

/// Tuple-shaped HTTP error returned by every route in this module. Mirrors
/// the convention in `handlers::*` but lives here so the agent surface is
/// self-contained. `IntoResponse` is derived via the tuple impl axum
/// provides for `(StatusCode, Json<Value>)`.
pub struct HttpError(pub axum::http::StatusCode, pub Json<serde_json::Value>);

impl HttpError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self(axum::http::StatusCode::BAD_REQUEST, Json(json!({ "error": msg.into() })))
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self(axum::http::StatusCode::NOT_FOUND, Json(json!({ "error": msg.into() })))
    }
    pub fn internal<E: std::fmt::Display>(e: E) -> Self {
        Self(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> axum::response::Response {
        (self.0, self.1).into_response()
    }
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agent/health", get(health))

        .route("/agent/models", get(models::list))
        .route("/agent/pricing", get(pricing::get_current).patch(pricing::patch))

        .route(
            "/agent/workspaces",
            get(workspaces::list).post(workspaces::create),
        )
        .route("/agent/workspaces/{id}", get(workspaces::get_one))
        .route("/agent/workspaces/{id}/stats", get(workspaces::stats))
        .route(
            "/agent/workspaces/{id}/sessions",
            get(sessions::list_for_workspace).post(sessions::create),
        )

        .route(
            "/agent/sessions/{id}",
            get(sessions::get_one).patch(sessions::update).delete(sessions::delete),
        )
        .route(
            "/agent/sessions/{id}/prompts",
            get(prompts::list_for_session).post(prompts::submit),
        )

        .route("/agent/prompts/{id}", get(prompts::detail))

        // Dashboards
        .route(
            "/agent/workspaces/{id}/dashboards",
            get(dashboards::list_for_workspace).post(dashboards::create),
        )
        .route(
            "/agent/dashboards/{id}",
            get(dashboards::get_one).patch(dashboards::patch).delete(dashboards::delete),
        )
        .route(
            "/agent/dashboards/{id}/widgets/{node_id}/run",
            post(dashboards::run_widget),
        )
        .route(
            "/agent/dashboards/{id}/refresh",
            post(dashboards::refresh),
        )

        // Components — reusable widget definitions with placeholder templates.
        .route(
            "/agent/workspaces/{id}/components",
            get(components::list_for_workspace).post(components::create),
        )
        .route(
            "/agent/workspaces/{id}/components/preview",
            post(components::preview),
        )
        .route(
            "/agent/components/{id}",
            get(components::get_one).patch(components::patch).delete(components::delete),
        )
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pricing = state
        .agent
        .db
        .query("SELECT COUNT(*) AS n FROM pricing_config", &[])
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|v| v.get("n").cloned())
        .unwrap_or(json!(0));
    let workspaces = state
        .agent
        .db
        .query("SELECT COUNT(*) AS n FROM workspace", &[])
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|v| v.get("n").cloned())
        .unwrap_or(json!(0));
    Json(json!({
        "ok": true,
        "pricing_config_rows": pricing,
        "workspaces": workspaces,
    }))
}

// Keep the route-method imports used in `router()` even if not referenced
// directly inside this file.
#[allow(unused)]
use {delete as _delete, patch as _patch, post as _post};
