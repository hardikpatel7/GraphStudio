/// TOML config file editor.
///
/// Reads/writes AppConfig TOML files from disk with three layers:
/// 1. default.toml — base defaults
/// 2. {tenant}.{env}.toml — environment-specific
/// 3. local.toml — local developer overrides
///
/// Each file is edited independently. A merged view shows the final result.

use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::err;

/// Return the schema describing all config groups and their field types.
pub async fn schema() -> Json<Value> {
    Json(json!({
        "groups": [
            { "key": "all", "label": "General", "type": "hashmap", "description": "Arbitrary tenant key-value pairs" },
            { "key": "server", "label": "Server", "type": "fixed", "fields": [
                { "key": "host", "label": "Host", "type": "string", "default": "0.0.0.0" },
                { "key": "port", "label": "Port", "type": "number", "default": 8080 },
            ]},
            { "key": "databases", "label": "Databases", "type": "databases", "description": "Named database connection pools", "fields": [
                { "key": "source", "label": "Source", "type": "select", "options": ["direct", "secretmanager"], "default": "direct" },
                { "key": "gcp_project_id", "label": "GCP Project ID", "type": "string" },
                { "key": "gcp_secret_name", "label": "GCP Secret Name", "type": "string" },
                { "key": "gcp_secret_version", "label": "GCP Secret Version", "type": "number" },
            ], "prefix_fields": [
                { "key": "prefix", "label": "Prefix", "type": "string" },
                { "key": "gcp_project_id", "label": "GCP Project ID (override)", "type": "string" },
                { "key": "gcp_secret_name", "label": "GCP Secret Name (override)", "type": "string" },
                { "key": "gcp_secret_version", "label": "GCP Secret Version (override)", "type": "number" },
            ]},
            { "key": "gcp", "label": "GCP", "type": "fixed", "fields": [
                { "key": "secret_id", "label": "Secret ID", "type": "string" },
                { "key": "project_id", "label": "Project ID", "type": "string" },
                { "key": "region", "label": "Region", "type": "string" },
                { "key": "gbq_dataset", "label": "BigQuery Dataset", "type": "string" },
                { "key": "gbq_schema", "label": "BigQuery Schema", "type": "string" },
                { "key": "firebase_web_api_key", "label": "Firebase Web API Key", "type": "string" },
                { "key": "gbq_project_id", "label": "BigQuery Project ID", "type": "string" },
                { "key": "auth_token_iss", "label": "Auth Token Issuer", "type": "string" },
            ]},
            { "key": "cache", "label": "Cache", "type": "fixed", "fields": [
                { "key": "enabled", "label": "Enabled", "type": "bool", "default": false },
            ]},
            { "key": "log", "label": "Logging", "type": "fixed", "fields": [
                { "key": "level", "label": "Level", "type": "select", "options": ["trace", "debug", "info", "warn", "error"], "default": "info" },
                { "key": "api_info", "label": "API Info Logging", "type": "bool", "default": false },
            ]},
            { "key": "telemetry", "label": "Telemetry", "type": "fixed", "fields": [
                { "key": "dd_agent_connection_url", "label": "Datadog Agent URL", "type": "string" },
            ]},
            { "key": "frontend", "label": "Frontend", "type": "fixed", "fields": [
                { "key": "build_path", "label": "Build Path", "type": "string" },
            ]},
            { "key": "cors", "label": "CORS", "type": "fixed", "fields": [
                { "key": "allowed_origins", "label": "Allowed Origins", "type": "list" },
                { "key": "allow_credentials", "label": "Allow Credentials", "type": "bool", "default": true },
                { "key": "max_age_secs", "label": "Max Age (seconds)", "type": "number", "default": 3600 },
            ]},
            { "key": "tenant", "label": "Tenant", "type": "fixed", "fields": [
                { "key": "code", "label": "Code", "type": "number", "default": 0 },
                { "key": "name", "label": "Name", "type": "string" },
                { "key": "url", "label": "URL", "type": "string" },
                { "key": "google_tenant_identity", "label": "Google Tenant Identity", "type": "string" },
                { "key": "sign_in_options", "label": "Sign-In Options", "type": "list" },
            ]},
        ]
    }))
}

/// List config files for the running tenant and their existence status.
pub async fn list_files(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let base_path = resolve_config_path(&state, &tenant_id)?;
    let (tenant, env) = parse_tenant_env(&tenant_id);

    let env_file = format!("{}.{}.toml", tenant, env);
    let files = vec![
        file_info(&base_path, "default.toml"),
        file_info(&base_path, &env_file),
        file_info(&base_path, "local.toml"),
    ];

    Ok(Json(json!({
        "base_path": base_path,
        "tenant": tenant,
        "environment": env,
        "env_file": env_file,
        "files": files,
    })))
}

/// Read and parse a single TOML config file.
pub async fn read_file(
    State(state): State<Arc<AppState>>,
    Path(filename): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let base_path = resolve_config_path(&state, &tenant_id)?;
    let (tenant, env) = parse_tenant_env(&tenant_id);

    let actual_filename = resolve_filename(&filename, &tenant, &env);
    let file_path = format!("{}/{}", base_path, actual_filename);

    let content = match std::fs::read_to_string(&file_path) {
        Ok(s) => s,
        Err(_) => return Ok(Json(json!({ "filename": actual_filename, "exists": false, "content": {} }))),
    };

    let parsed: Value = toml::from_str(&content)
        .map_err(|e| err(400, &format!("TOML parse error: {}", e)))?;

    Ok(Json(json!({ "filename": actual_filename, "exists": true, "content": parsed })))
}

/// Write a TOML config file to disk.
pub async fn write_file(
    State(state): State<Arc<AppState>>,
    Path(filename): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let base_path = resolve_config_path(&state, &tenant_id)?;
    let (tenant, env) = parse_tenant_env(&tenant_id);

    let actual_filename = resolve_filename(&filename, &tenant, &env);
    let file_path = format!("{}/{}", base_path, actual_filename);

    let content = body.get("content").ok_or_else(|| err(400, "content required"))?;

    // Convert JSON → TOML string
    let toml_value: toml::Value = serde_json::from_value(content.clone())
        .map_err(|e| err(400, &format!("Invalid config structure: {}", e)))?;
    let toml_str = toml::to_string_pretty(&toml_value)
        .map_err(|e| err(500, &format!("TOML serialization failed: {}", e)))?;

    // Ensure directory exists
    std::fs::create_dir_all(&base_path).map_err(|e| err(500, &format!("Failed to create dir: {}", e)))?;
    std::fs::write(&file_path, &toml_str).map_err(|e| err(500, &format!("Failed to write file: {}", e)))?;

    // Log activity
    state.traces.log_activity(&tenant_id, "config", &actual_filename, "success",
        &format!("Config file {} updated", actual_filename), None, None).ok();

    Ok(Json(json!({ "success": true, "filename": actual_filename, "path": file_path })))
}

/// Return merged config (all three layers combined, read-only).
pub async fn merged(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let base_path = resolve_config_path(&state, &tenant_id)?;
    let (tenant, env) = parse_tenant_env(&tenant_id);
    let env_file = format!("{}.{}.toml", tenant, env);

    let mut result = json!({});

    // Load in precedence order: default < tenant.env < local
    for filename in &["default.toml", &env_file, "local.toml"] {
        let file_path = format!("{}/{}", base_path, filename);
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            if let Ok(parsed) = toml::from_str::<Value>(&content) {
                deep_merge(&mut result, &parsed);
            }
        }
    }

    Ok(Json(json!({ "merged": result })))
}

/// List available database connections from merged TOML config.
/// Returns connection names + source type (direct/secretmanager) + masked details.
pub async fn db_connections(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let tenant_id = state.tenant_id.clone();
    let base_path = resolve_config_path(&state, &tenant_id)?;
    let (tenant, env) = parse_tenant_env(&tenant_id);
    let env_file = format!("{}.{}.toml", tenant, env);

    // Merge configs
    let mut merged = json!({});
    for filename in &["default.toml", &env_file, "local.toml"] {
        let file_path = format!("{}/{}", base_path, filename);
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            if let Ok(parsed) = toml::from_str::<Value>(&content) {
                deep_merge(&mut merged, &parsed);
            }
        }
    }

    let databases = merged.get("databases").cloned().unwrap_or(json!({}));
    let source = databases.get("source").and_then(|v| v.as_str()).unwrap_or("direct");
    let mut connections = Vec::new();

    if source == "direct" {
        if let Some(conns) = databases.get("connections").and_then(|c| c.as_object()) {
            for (name, cfg) in conns {
                let conn_type = cfg.get("type").and_then(|v| v.as_str()).unwrap_or("pg");
                connections.push(json!({
                    "name": name,
                    "source": "direct",
                    "type": conn_type,
                    "host": cfg.get("host").and_then(|v| v.as_str()).unwrap_or(""),
                    "port": cfg.get("port").and_then(|v| v.as_str())
                        .or_else(|| cfg.get("port").and_then(|v| v.as_i64()).map(|_| ""))
                        .unwrap_or("5432"),
                    "database": cfg.get("database").and_then(|v| v.as_str()).unwrap_or(""),
                    "username": cfg.get("username").or(cfg.get("user")).and_then(|v| v.as_str()).unwrap_or(""),
                    "password": "••••••••",
                }));
            }
        }
    } else if source == "secretmanager" {
        let gcp_project = databases.get("gcp_project_id").and_then(|v| v.as_str()).unwrap_or("");
        let gcp_secret = databases.get("gcp_secret_name").and_then(|v| v.as_str()).unwrap_or("");

        if let Some(prefixes) = databases.get("prefixes").and_then(|p| p.as_object()) {
            for (name, cfg) in prefixes {
                let prefix = cfg.get("prefix").and_then(|v| v.as_str()).unwrap_or(name.as_str());
                let proj = cfg.get("gcp_project_id").and_then(|v| v.as_str()).unwrap_or(gcp_project);
                let secret = cfg.get("gcp_secret_name").and_then(|v| v.as_str()).unwrap_or(gcp_secret);
                let conn_type = cfg.get("type").and_then(|v| v.as_str()).unwrap_or("pg");
                connections.push(json!({
                    "name": name,
                    "source": "secretmanager",
                    "type": conn_type,
                    "prefix": prefix,
                    "gcp_project_id": proj,
                    "gcp_secret_name": secret,
                }));
            }
        }
        // If no prefixes defined, show a default entry
        if connections.is_empty() && !gcp_project.is_empty() {
            connections.push(json!({
                "name": "primary",
                "source": "secretmanager",
                "type": "pg",
                "prefix": "PRIMARY",
                "gcp_project_id": gcp_project,
                "gcp_secret_name": gcp_secret,
            }));
        }
    }

    // Type defaults: [databases.defaults] { pg = "primary", bq = "bq-prod" }
    let defaults = databases.get("defaults").cloned().unwrap_or(json!({}));

    Ok(Json(json!({
        "source": source,
        "connections": connections,
        "defaults": defaults,
    })))
}

// ── Helpers ──

fn resolve_config_path(state: &AppState, tenant_id: &str) -> Result<String, (axum::http::StatusCode, Json<Value>)> {
    // Try env_setting first, then fall back to a default
    state.traces.get_setting(tenant_id, "config_base_path")
        .ok()
        .flatten()
        .or_else(|| {
            // Fall back: look for a config dir relative to parquet_home
            let p = format!("{}/../config", state.parquet_home);
            Some(p)
        })
        .ok_or_else(|| err(500, "No config_base_path configured"))
}

fn parse_tenant_env(tenant_id: &str) -> (String, String) {
    // tenant_id format: {client}-{apptype}-{env}
    // We need to split into tenant name (for file naming) and env
    let parts: Vec<&str> = tenant_id.rsplitn(2, '-').collect();
    if parts.len() == 2 {
        (parts[1].to_string(), parts[0].to_string())
    } else {
        ("environment".to_string(), "dev".to_string())
    }
}

fn resolve_filename(name: &str, tenant: &str, env: &str) -> String {
    match name {
        "default" => "default.toml".to_string(),
        "local" => "local.toml".to_string(),
        "env" | "tenant_env" => format!("{}.{}.toml", tenant, env),
        other => other.to_string(),
    }
}

fn file_info(base_path: &str, filename: &str) -> Value {
    let path = format!("{}/{}", base_path, filename);
    let meta = std::fs::metadata(&path);
    json!({
        "name": filename,
        "exists": meta.is_ok(),
        "size_bytes": meta.as_ref().map(|m| m.len()).unwrap_or(0),
    })
}

fn deep_merge(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (k, v) in overlay_map {
                let entry = base_map.entry(k.clone()).or_insert(Value::Null);
                deep_merge(entry, v);
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}
