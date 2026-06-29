//! Instance config for the SmartStudio server.
//!
//! At startup the server reads a single required `environment.toml` describing the
//! tenant identity and where data lives. There is no fallback — if the file is missing
//! or invalid, the server exits.
//!
//! Folder layout: `<home_path>/smartstudio/<tenant_id>/data/...`
//! where `tenant_id = "{client}-{app_type}-{environment}"`.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const NAMESPACE_DIR: &str = "smartstudio";
const CONFIG_FILE: &str = "environment.toml";

#[derive(Deserialize)]
pub struct InstanceConfig {
    pub home_path: String,
    pub environment: String,
    pub client: String,
    pub app_type: String,
    #[serde(default)]
    pub is_new: bool,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub rcl: RclConfig,
    #[serde(default)]
    pub graphs: GraphsConfig,
    #[serde(default)]
    pub pipeline: PipelineConfig,
    /// Optional path overrides. Every field is `Option<String>`; anything
    /// you omit falls back to the home-relative default
    /// (`home_path/smartstudio/{tenant_id}/data/...`). Use to put
    /// individual artifacts on different volumes — e.g. parquet on a
    /// shared lake while keeping the SQLite metadata on local disk.
    /// Paths can be absolute, or relative to `home_path` (joined with
    /// `home_path` so per-environment configs only need to swap
    /// `home_path` to retarget everything).
    #[serde(default)]
    pub paths: PathOverrides,
    /// Agent runtime config — currently only the GCP Secret Manager
    /// pointer for fetching LLM API keys. Optional; when absent the
    /// server assumes the relevant env vars (`OPENAI_API_KEY`,
    /// `ANTHROPIC_API_KEY`) are already in the shell env (local dev,
    /// dotenv, k8s manifest, etc.).
    #[serde(default)]
    pub agent: AgentConfig,
}

#[derive(Deserialize, Default)]
pub struct AgentConfig {
    /// GCP project where the LLM-keys secret lives. Required to
    /// fetch from Secret Manager; leave unset for local dev.
    pub gcp_project_id: Option<String>,
    /// Secret name inside that project. The secret payload should
    /// be a JSON object — `{ "OPENAI_API_KEY": "...",
    /// "ANTHROPIC_API_KEY": "..." }` — which `SecretManager::load_env`
    /// reads and exports as process env vars. Rig's `Client::from_env`
    /// then picks them up automatically.
    pub llm_secret_name: Option<String>,
    /// Optional pinned secret version. Defaults to "latest".
    pub llm_secret_version: Option<u8>,
}

#[derive(Deserialize, Default)]
pub struct PathOverrides {
    /// Override the tenant data directory itself. Useful when an entire
    /// tenant lives on a separate volume but you don't want to touch
    /// `home_path` (which is also the search anchor for relative
    /// overrides).
    pub data_dir: Option<String>,
    /// SQLite metadata DB. Default: `<data_dir>/smartstudio.db`.
    pub db_path: Option<String>,
    /// Materialized data DuckDB. Default: `<data_dir>/tenant_data.duckdb`.
    pub duckdb_path: Option<String>,
    /// Pipeline / activity-log DuckDB. Default: `<data_dir>/log.duckdb`.
    pub log_db_path: Option<String>,
    /// Parquet root for source bindings. Default: `<data_dir>/parquet`.
    pub parquet_home: Option<String>,
    /// Trace artifacts dir. Default: `<data_dir>/traces`.
    pub traces_dir: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ServerConfig {
    pub port: Option<u16>,
    /// Tonic gRPC port. Defaults to 50051. Used for the RCL service (and any
    /// future in-process gRPC services) when [rcl].enabled = true.
    pub grpc_port: Option<u16>,
}

/// Optional RCL service config. When `enabled = true`, the server boots the
/// in-process Tonic [`RclService`](crate::services::rcl_grpc::RclGrpcService).
/// Disabled by default — only tenants with the RCL schema (`global.rcl_master`,
/// `inventory_smart.rcl_*`) should turn it on.
/// Pipeline runtime config.
///
/// `progress_interval_ms` is the global cadence (in ms) at which steps emit
/// intra-progress events (e.g., bytes downloaded during a long pg_extract).
/// `0` or absent disables intra-step progress entirely — only phase-boundary
/// events are sent. Default: `2000` (every 2s).
#[derive(Deserialize)]
pub struct PipelineConfig {
    pub progress_interval_ms: Option<u64>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self { progress_interval_ms: Some(2000) }
    }
}

#[derive(Deserialize, Default)]
pub struct RclConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Optional. When set, the resolved default-PG DSN's `port=…` is rewritten
    /// to this value before being handed to the RCL `RuleStore`. Useful when
    /// the RCL tables (and the trigger migration) live on a separate PG
    /// instance — typically a read-replica/MV server on the same host.
    pub port_override: Option<u16>,
}

/// Optional graph config. `default_id` names the graph row that
/// handlers should reach for when they need "the" graph (e.g. the
/// cross-filter endpoint) without a per-request id. Must match the
/// `id` field of a row in the `graphs` SQLite table.
#[derive(Deserialize, Default)]
pub struct GraphsConfig {
    pub default_id: Option<String>,
}

pub struct Resolved {
    pub config: InstanceConfig,
    pub tenant_id: String,
    pub namespace_dir: String,
    pub tenant_root: String,
    pub data_dir: String,
    pub db_path: String,
    pub parquet_home: String,
    pub traces_dir: String,
    pub log_db_path: String,
    pub duckdb_path: String,
    pub port: String,
    pub grpc_port: u16,
}

/// Search exe_dir/../../.., exe_dir, and cwd for `environment.toml`.
/// Returns the first existing path. Errors with all paths tried if none found.
pub fn discover() -> Result<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let candidates = vec![
        exe_dir.join("../../..").join(CONFIG_FILE),
        exe_dir.join(CONFIG_FILE),
        cwd.join(CONFIG_FILE),
    ];

    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }

    let tried = candidates
        .iter()
        .map(|p| format!("  {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(anyhow!("{} not found. Looked in:\n{}", CONFIG_FILE, tried))
}

pub fn load(path: &Path) -> Result<InstanceConfig> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str::<InstanceConfig>(&body)
        .with_context(|| format!("parsing {}", path.display()))
}

/// Validate `home_path` (absolute + exists), compute tenant_id and derived paths,
/// fill defaults (port).
pub fn resolve(cfg: InstanceConfig) -> Result<Resolved> {
    let home = Path::new(&cfg.home_path);
    if !home.is_absolute() {
        return Err(anyhow!(
            "home_path '{}' must be an absolute path",
            cfg.home_path
        ));
    }
    if !home.is_dir() {
        return Err(anyhow!(
            "home_path '{}' is not an existing directory",
            cfg.home_path
        ));
    }

    let tenant_id = format!("{}-{}-{}", cfg.client, cfg.app_type, cfg.environment);
    let namespace_dir = home.join(NAMESPACE_DIR);
    let tenant_root = namespace_dir.join(&tenant_id);

    // Helper: resolve an override, honoring absolute paths verbatim and
    // joining relative paths against `home_path`. Falls back to the
    // home-relative default when no override is set.
    let resolve_path = |override_value: &Option<String>, default: PathBuf| -> PathBuf {
        match override_value.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => {
                let p = Path::new(s);
                if p.is_absolute() { p.to_path_buf() } else { home.join(p) }
            }
            None => default,
        }
    };

    let data_dir = resolve_path(&cfg.paths.data_dir, tenant_root.join("data"));
    let db_path = resolve_path(&cfg.paths.db_path, data_dir.join("smartstudio.db"));
    let parquet_home = resolve_path(&cfg.paths.parquet_home, data_dir.join("parquet"));
    let traces_dir = resolve_path(&cfg.paths.traces_dir, data_dir.join("traces"));
    let log_db_path = resolve_path(&cfg.paths.log_db_path, data_dir.join("log.duckdb"));
    let duckdb_path = resolve_path(&cfg.paths.duckdb_path, data_dir.join("tenant_data.duckdb"));
    let port = cfg.server.port.map(|p| p.to_string()).unwrap_or_else(|| "3001".to_string());
    let grpc_port = cfg.server.grpc_port.unwrap_or(50051);

    Ok(Resolved {
        config: cfg,
        tenant_id,
        namespace_dir: namespace_dir.to_string_lossy().to_string(),
        tenant_root: tenant_root.to_string_lossy().to_string(),
        data_dir: data_dir.to_string_lossy().to_string(),
        db_path: db_path.to_string_lossy().to_string(),
        parquet_home: parquet_home.to_string_lossy().to_string(),
        traces_dir: traces_dir.to_string_lossy().to_string(),
        log_db_path: log_db_path.to_string_lossy().to_string(),
        duckdb_path: duckdb_path.to_string_lossy().to_string(),
        port,
        grpc_port,
    })
}

/// Creates the namespace dir if missing, then enforces the (db_path exists, is_new) match.
/// On bootstrap, scaffolds dirs and initializes the SQLite schema by opening it.
pub fn ensure_ready(r: &Resolved) -> Result<()> {
    std::fs::create_dir_all(&r.namespace_dir)
        .with_context(|| format!("creating namespace dir {}", r.namespace_dir))?;

    let db_exists = Path::new(&r.db_path).exists();
    match (db_exists, r.config.is_new) {
        (true, false) => Ok(()),
        (false, true) => {
            std::fs::create_dir_all(&r.parquet_home)
                .with_context(|| format!("creating {}", r.parquet_home))?;
            std::fs::create_dir_all(&r.traces_dir)
                .with_context(|| format!("creating {}", r.traces_dir))?;
            crate::db::Database::open(&r.db_path)
                .map_err(|e| anyhow!("initializing schema at {}: {}", r.db_path, e))?;
            copy_product_templates(r);
            tracing::info!(
                "Bootstrapped tenant '{}' at {}. Remove is_new from environment.toml before the next start.",
                r.tenant_id,
                r.tenant_root
            );
            Ok(())
        }
        (false, false) => Err(anyhow!(
            "Tenant '{}' not found at {}. Set is_new = true in environment.toml to bootstrap it.",
            r.tenant_id,
            r.tenant_root
        )),
        (true, true) => Err(anyhow!(
            "Tenant '{}' already exists at {}, but environment.toml has is_new = true. Remove is_new (or delete the tenant folder if you really intended a fresh bootstrap).",
            r.tenant_id,
            r.tenant_root
        )),
    }
}

/// On `is_new = true`, copy `templates/<app_type>/{dataviews,sources,
/// duckdb_views,graphs}/*` into the tenant data dir. Tenants own their
/// seed from that point on — operators edit the per-tenant copies, the
/// repo template is never read again at runtime. Missing templates dir
/// for the configured app_type is logged and skipped (an operator may
/// legitimately want an empty starting point).
fn copy_product_templates(r: &Resolved) {
    let Some(templates_root) = find_templates_root() else {
        tracing::info!(
            "[bootstrap] templates dir not found from CWD / exe-relative search — \
             skipping product seed copy"
        );
        return;
    };
    let product_dir = templates_root.join(&r.config.app_type);
    if !product_dir.is_dir() {
        tracing::info!(
            app_type = %r.config.app_type,
            "[bootstrap] no template for app_type at {} — tenant starts empty",
            product_dir.display()
        );
        return;
    }
    let data_dir = Path::new(&r.data_dir);
    let mut copied = 0usize;
    for sub in ["dataviews", "sources", "duckdb_views", "graphs"] {
        let src = product_dir.join(sub);
        if !src.is_dir() { continue; }
        let dst = data_dir.join(sub);
        if let Err(e) = std::fs::create_dir_all(&dst) {
            tracing::warn!(error = %e, dir = %dst.display(),
                "[bootstrap] could not create template subdir");
            continue;
        }
        match copy_dir_files(&src, &dst) {
            Ok(n) => copied += n,
            Err(e) => tracing::warn!(error = %e, from = %src.display(), to = %dst.display(),
                "[bootstrap] template copy failed"),
        }
    }
    tracing::info!(
        app_type = %r.config.app_type,
        files = copied,
        "[bootstrap] copied product templates into tenant data dir"
    );
}

fn find_templates_root() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = [
        cwd.join("templates"),
        exe_dir.join("../../../templates"),
        exe_dir.join("../../templates"),
        exe_dir.join("templates"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

fn copy_dir_files(src: &Path, dst: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let target = dst.join(entry.file_name());
            std::fs::copy(&path, &target)?;
            n += 1;
        }
    }
    Ok(n)
}
