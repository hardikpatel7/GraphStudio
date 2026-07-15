//! Inert scaffold stub of the private `secret_manager` crate.
//!
//! This is NOT the real crate — it exists so the public GraphStudio repo
//! compiles without SSH access to `rust-shared-utils`, and without pulling in
//! the GCP Secret Manager client. It carries NO credentials and talks to NO
//! backend. Every fetch returns an error, which callers already treat as
//! "fall back to shell environment variables". See CLAUDE.md.

use std::collections::HashMap;
use std::error::Error;

use serde_json::Value;

/// Mirror of the real crate's params (field-for-field). Constructed by the
/// server in `main.rs`; never used to reach a backend here.
#[derive(Debug)]
pub struct SecretManagerParams {
    pub project_id: String,
    pub secret_name: String,
    pub version: Option<u8>,
}

/// Inert stand-in. Construction succeeds so boot proceeds; every accessor
/// returns an error so the server logs its "relying on shell env" fallback.
#[derive(Debug)]
pub struct SecretManager {
    _config: SecretManagerParams,
}

impl SecretManager {
    pub async fn new(config: SecretManagerParams) -> Result<Self, Box<dyn Error>> {
        Ok(Self { _config: config })
    }

    fn unavailable() -> Box<dyn Error> {
        "secret_manager stub: no GCP backend in the public scaffold build".into()
    }

    pub async fn get_secret_value(&self) -> Result<String, Box<dyn Error>> {
        Err(Self::unavailable())
    }

    pub async fn get_secret_json(&self) -> Result<Value, Box<dyn Error>> {
        Err(Self::unavailable())
    }

    pub async fn get_secret_map(&self) -> Result<HashMap<String, String>, Box<dyn Error>> {
        Err(Self::unavailable())
    }

    pub async fn load_env(&self) -> Result<(), Box<dyn Error>> {
        Err(Self::unavailable())
    }
}
