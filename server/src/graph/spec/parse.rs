//! TOML → `GraphSpec` deserialization.
//!
//! Thin wrapper around `toml::from_str` that preserves `Display` output
//! (which already includes the `line N, column M` span from `toml`'s
//! parser). Returning `anyhow::Error` matches the rest of the server.

use anyhow::{Context, Result};

use super::GraphSpec;

/// Parse a graph TOML document.
///
/// Errors from the `toml` crate already render with positional context
/// (`expected …, at line X column Y`). We layer one `anyhow` context on
/// top so the call site (e.g. `POST /api/graphs/:id/validate`) gets a
/// clear "what was the operation" frame in addition to the parser's
/// "where did it fail" frame.
pub fn from_toml(text: &str) -> Result<GraphSpec> {
    toml::from_str::<GraphSpec>(text).context("parse graph TOML")
}
