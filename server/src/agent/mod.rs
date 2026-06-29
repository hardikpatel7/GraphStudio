//! Agent — prompt-driven planner that exposes SmartStudio capabilities to an LLM
//! and streams responses back to the UI via SSE.
//!
//! Architecture:
//! - `db`        — SQLite for workspaces, sessions, prompts, usage, pricing.
//! - `config`    — model allowlist + pricing seed.
//! - `cache`     — short-TTL LRU for idempotent tool results.
//! - `meter`     — single-writer task for `api_call` inserts + derive-on-read pricing.
//! - `tools`     — Rig `Tool` impls wrapping `crate::service::*` functions.
//! - `llm`       — `LlmRunner` facade; Rig backend for v1, async-openai stubbed.
//! - `routes`    — Axum routes mounted under `/api/agent/*`.

pub mod cache;
pub mod components;
pub mod config;
pub mod dashboards;
pub mod db;
pub mod llm;
pub mod meter;
pub mod routes;
pub mod schema;
pub mod tools;

use std::sync::Arc;

use axum::Router;

use crate::AppState;

/// Per-process state owned by the agent module. Held inside `AppState.agent`
/// so existing handlers and agent routes both reach it through the same Arc.
pub struct AgentState {
    pub db: Arc<db::AgentDb>,
    pub cache: Arc<cache::ToolCache>,
    pub meter_tx: meter::writer::MeterTx,
}

impl AgentState {
    pub fn new(
        db: Arc<db::AgentDb>,
        cache: Arc<cache::ToolCache>,
        meter_tx: meter::writer::MeterTx,
    ) -> Self {
        Self { db, cache, meter_tx }
    }
}

/// Build the `/api/agent/*` sub-router. Called from `main.rs` and merged
/// into the existing `/api` nest with `.merge(...)`.
pub fn router() -> Router<Arc<AppState>> {
    routes::router()
}
