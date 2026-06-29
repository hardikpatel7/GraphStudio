//! Metering: records every tool invocation's measured facts (latency, bytes,
//! status) and derives cost on read. Three submodules:
//!
//! - `writer`  — single async task that drains an mpsc channel and batches
//!               `api_call` / `llm_usage` inserts into `agent.db`.
//! - `wrap`    — per-call wrapper used by `tools.rs`. Emits SSE start/finish
//!               events, checks the LRU cache, enforces a per-call timeout
//!               and a response-size cap, records the measurement.
//! - `pricing` — joins `api_call` + `llm_usage` against the latest
//!               `pricing_config` row whose `effective_from <= started_at` to
//!               produce per-prompt and aggregate cost figures.

pub mod hook;
pub mod pricing;
pub mod wrap;
pub mod writer;
