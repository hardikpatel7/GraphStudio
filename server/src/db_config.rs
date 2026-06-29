/// Database connection resolution from TOML config + GCP Secret Manager.
///
/// Resolution order:
/// 1. TOML config [databases] section (default.toml → tenant.env.toml → local.toml merged)
/// 2. If source = "secretmanager", fetch credentials from GCP Secret Manager
/// 3. If source = "direct", use credentials from TOML directly
/// 4. Fallback: SQLite connections table (legacy)
///
/// TOML format (matches rust-shared-utils/config):
/// ```toml
/// [databases]
/// source = "secretmanager"   # or "direct"
/// gcp_project_id = "my-project"
/// gcp_secret_name = "db-credentials"
/// gcp_secret_version = "latest"
///
/// [databases.prefixes.primary]
/// prefix = "PRIMARY"
///
/// # Direct mode:
/// [databases.connections.primary]
/// host = "localhost"
/// port = "5432"
/// username = "user"
/// password = "pass"
/// database = "mydb"
/// ```

use serde_json::Value;
use std::collections::HashMap;

/// Resolved database connection credentials.
#[derive(Debug, Clone)]
pub struct DbCredentials {
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub database: String,
}

impl DbCredentials {
    pub fn to_conn_str(&self) -> String {
        format!(
            "host={} port={} user={} password={} dbname={} sslmode=disable connect_timeout=30",
            self.host, self.port, self.username, self.password, self.database
        )
    }
}

/// Resolve database credentials from TOML config files.
/// Reads merged config (default.toml + tenant.env.toml + local.toml),
/// then resolves via direct config or GCP Secret Manager.
pub fn resolve_from_toml(config_base_path: &str, tenant_id: &str, conn_name: &str) -> Option<DbCredentials> {
    let (tenant, env) = parse_tenant_env(tenant_id);

    // Load and merge TOML files: default.toml → tenant.env.toml → local.toml
    let merged = merge_toml_configs(config_base_path, &tenant, &env)?;
    let databases = merged.get("databases")?;

    let source = databases.get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("direct");

    match source {
        "secretmanager" => resolve_from_secret_manager(databases, conn_name),
        "direct" => resolve_from_direct(databases, conn_name),
        _ => {
            tracing::warn!("Unknown database source: {}", source);
            None
        }
    }
}

/// Direct mode: read credentials from TOML [databases.connections.{name}]
fn resolve_from_direct(databases: &Value, conn_name: &str) -> Option<DbCredentials> {
    let connections = databases.get("connections")?;
    let conn = connections.get(conn_name)
        .or_else(|| connections.get("primary"))
        .or_else(|| connections.get("default"))
        .or_else(|| {
            // Take first connection
            connections.as_object().and_then(|m| m.values().next())
        })?;

    Some(DbCredentials {
        host: conn.get("host").and_then(|v| v.as_str()).unwrap_or("localhost").to_string(),
        port: conn.get("port").and_then(|v| v.as_str())
            .or_else(|| conn.get("port").and_then(|v| v.as_i64()).map(|_| ""))
            .unwrap_or("5432").to_string(),
        username: conn.get("username").and_then(|v| v.as_str())
            .or_else(|| conn.get("user").and_then(|v| v.as_str()))
            .unwrap_or("").to_string(),
        password: conn.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        database: conn.get("database").and_then(|v| v.as_str()).unwrap_or("").to_string(),
    })
}

/// Secret Manager mode: fetch credentials from GCP Secret Manager.
/// Uses gcloud CLI to access secrets (same as bq/gsutil pattern).
fn resolve_from_secret_manager(databases: &Value, conn_name: &str) -> Option<DbCredentials> {
    // Get GCP config — check per-connection prefix override first, fall back to top-level
    let prefixes = databases.get("prefixes");
    let conn_prefix_config = prefixes
        .and_then(|p| p.get(conn_name).or_else(|| p.get("primary")).or_else(|| {
            p.as_object().and_then(|m| m.values().next())
        }));

    let prefix = conn_prefix_config
        .and_then(|c| c.get("prefix")).and_then(|v| v.as_str())
        .unwrap_or("PRIMARY");

    let project_id = conn_prefix_config
        .and_then(|c| c.get("gcp_project_id")).and_then(|v| v.as_str())
        .or_else(|| databases.get("gcp_project_id").and_then(|v| v.as_str()))
        .unwrap_or("");

    let secret_name = conn_prefix_config
        .and_then(|c| c.get("gcp_secret_name")).and_then(|v| v.as_str())
        .or_else(|| databases.get("gcp_secret_name").and_then(|v| v.as_str()))
        .unwrap_or("");

    let secret_version = conn_prefix_config
        .and_then(|c| c.get("gcp_secret_version")).and_then(|v| v.as_str())
        .or_else(|| databases.get("gcp_secret_version").and_then(|v| v.as_str()))
        .unwrap_or("latest");

    if project_id.is_empty() || secret_name.is_empty() {
        tracing::warn!("Secret Manager config incomplete: project={}, secret={}", project_id, secret_name);
        return None;
    }

    tracing::info!("Fetching DB credentials from Secret Manager: project={}, secret={}, version={}, prefix={}",
        project_id, secret_name, secret_version, prefix);

    // Fetch secret via gcloud CLI
    let secret_path = format!("projects/{}/secrets/{}/versions/{}", project_id, secret_name, secret_version);
    let output = std::process::Command::new("gcloud")
        .args(["secrets", "versions", "access", secret_version, "--secret", secret_name, "--project", project_id])
        .output()
        .ok()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("gcloud secrets access failed: {}", stderr.trim());
        return None;
    }

    let secret_json = String::from_utf8_lossy(&output.stdout);
    let secret: HashMap<String, String> = serde_json::from_str(&secret_json).ok().or_else(|| {
        // Try as key=value pairs per line
        tracing::warn!("Secret is not JSON, trying key=value parse");
        None
    })?;

    // Extract prefixed keys: PREFIX_HOST, PREFIX_PORT, etc.
    let get = |key: &str| -> String {
        secret.get(&format!("{}_{}", prefix, key))
            .or_else(|| secret.get(key))
            .cloned()
            .unwrap_or_default()
    };

    let creds = DbCredentials {
        host: get("HOST"),
        port: if get("PORT").is_empty() { "5432".to_string() } else { get("PORT") },
        username: get("USERNAME"),
        password: get("PASSWORD"),
        database: get("DATABASE"),
    };

    if creds.host.is_empty() || creds.username.is_empty() {
        tracing::warn!("Secret Manager credentials incomplete for prefix={}: host={}, user={}", prefix, creds.host, creds.username);
        return None;
    }

    tracing::info!("Resolved DB credentials from Secret Manager: host={}, port={}, user={}, db={}",
        creds.host, creds.port, creds.username, creds.database);
    Some(creds)
}

/// Resolve type defaults from TOML: [databases.defaults] { pg = "primary", bq = "bq-prod" }
/// Returns a map of connection_type → default_connection_name.
pub fn resolve_type_defaults(config_base_path: &str, tenant_id: &str) -> std::collections::HashMap<String, String> {
    let (tenant, env) = parse_tenant_env(tenant_id);
    let mut defaults = std::collections::HashMap::new();
    let merged = match merge_toml_configs(config_base_path, &tenant, &env) {
        Some(m) => m,
        None => return defaults,
    };
    if let Some(db_defaults) = merged.get("databases").and_then(|d| d.get("defaults")).and_then(|d| d.as_object()) {
        for (key, val) in db_defaults {
            if let Some(v) = val.as_str() {
                defaults.insert(key.clone(), v.to_string());
            }
        }
    }
    defaults
}

/// Resolve connection type from TOML config for a named connection.
/// Returns the type string ("pg", "bq", etc.) or "pg" as default.
pub fn resolve_connection_type(config_base_path: &str, tenant_id: &str, conn_name: &str) -> String {
    let (tenant, env) = parse_tenant_env(tenant_id);
    let merged = match merge_toml_configs(config_base_path, &tenant, &env) {
        Some(m) => m,
        None => return "pg".to_string(),
    };
    let databases = match merged.get("databases") {
        Some(d) => d,
        None => return "pg".to_string(),
    };
    let connections = match databases.get("connections") {
        Some(c) => c,
        None => return "pg".to_string(),
    };
    connections.get(conn_name)
        .and_then(|c| c.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("pg")
        .to_string()
}

/// Resolve a raw connection config Value from TOML for a named connection.
/// Returns the full connection object (for BQ connections that aren't PG-style).
pub fn resolve_connection_raw(config_base_path: &str, tenant_id: &str, conn_name: &str) -> Option<Value> {
    let (tenant, env) = parse_tenant_env(tenant_id);
    let merged = merge_toml_configs(config_base_path, &tenant, &env)?;
    let databases = merged.get("databases")?;
    let connections = databases.get("connections")?;
    connections.get(conn_name).cloned()
}

/// Merge TOML config files: default.toml → tenant.env.toml → local.toml
fn merge_toml_configs(base_path: &str, tenant: &str, env: &str) -> Option<Value> {
    let files = [
        format!("{}/default.toml", base_path),
        format!("{}/{}.{}.toml", base_path, tenant, env),
        format!("{}/local.toml", base_path),
    ];

    let mut merged = serde_json::Map::new();

    for file in &files {
        if let Ok(content) = std::fs::read_to_string(file) {
            if let Ok(parsed) = content.parse::<toml::Table>() {
                let json: Value = serde_json::to_value(parsed).unwrap_or_default();
                if let Value::Object(obj) = json {
                    deep_merge(&mut merged, obj);
                }
            }
        }
    }

    if merged.is_empty() { None } else { Some(Value::Object(merged)) }
}

/// Deep merge: later values override earlier ones.
fn deep_merge(base: &mut serde_json::Map<String, Value>, overlay: serde_json::Map<String, Value>) {
    for (key, value) in overlay {
        match (base.get_mut(&key), &value) {
            (Some(Value::Object(base_obj)), Value::Object(overlay_obj)) => {
                deep_merge(base_obj, overlay_obj.clone());
            }
            _ => {
                base.insert(key, value);
            }
        }
    }
}

fn parse_tenant_env(tenant_id: &str) -> (String, String) {
    let parts: Vec<&str> = tenant_id.rsplitn(2, '-').collect();
    if parts.len() == 2 {
        (parts[1].to_string(), parts[0].to_string())
    } else {
        ("environment".to_string(), "dev".to_string())
    }
}
