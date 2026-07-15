//! Inert scaffold stub of the private `pipeline` crate (the ETL pipeline engine).
//!
//! This is NOT the real crate — it exists so the public GraphStudio repo
//! compiles without SSH access to `rust-shared-utils`. It reproduces the shapes
//! the server references (builder, step/config types, execution context,
//! events) with no-op execution: `Pipeline::execute` returns an empty
//! `PipelineResult` and no data is extracted, loaded, or materialized.
//!
//! The one exception is `PipelineTrigger`: its serde representation and the
//! `cdc_source_ids` / `listens_for_rcl` accessors are ordinary trigger-parsing
//! plumbing (not proprietary logic), and the scheduler parses stored trigger
//! JSON, so they are reproduced faithfully. See CLAUDE.md.

use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

// ── Identifiers ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StepId(pub String);

impl From<String> for StepId {
    fn from(s: String) -> Self {
        StepId(s)
    }
}
impl From<&str> for StepId {
    fn from(s: &str) -> Self {
        StepId(s.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectionRefId(pub String);

impl From<String> for ConnectionRefId {
    fn from(s: String) -> Self {
        ConnectionRefId(s)
    }
}
impl From<&str> for ConnectionRefId {
    fn from(s: &str) -> Self {
        ConnectionRefId(s.to_string())
    }
}

// ── Enums ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    #[default]
    DuckDbOnly,
    DuckDbAndInMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Parallel,
    Sequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnFailure {
    #[default]
    Abort,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    Start,
    Progress,
    Success,
    Skipped,
    Failed,
}

impl std::fmt::Display for StepStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StepStatus::Start => "start",
            StepStatus::Progress => "progress",
            StepStatus::Success => "success",
            StepStatus::Skipped => "skipped",
            StepStatus::Failed => "failed",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum ChangeKey {
    Column {
        column: String,
        lookup_sql: String,
    },
    #[default]
    None,
}

// ── Step configs ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PgExtractConfig {
    pub id: StepId,
    pub label: String,
    pub query: String,
    pub output_path: String,
    pub connection_ref: Option<ConnectionRefId>,
    pub pg_table: String,
    pub duckdb_table: String,
    pub primary_key: Vec<String>,
    pub change_key: ChangeKey,
    pub target_source_id: Option<String>,
    pub partition_column: Option<String>,
    pub partition_values_sql: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuckDbLoadConfig {
    pub id: StepId,
    pub label: String,
    pub table_name: String,
    pub source_parquet: String,
    pub hive_partitioning: bool,
    pub target_source_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuckDbQueryConfig {
    pub id: StepId,
    pub label: String,
    pub query: String,
    pub scoped_delete: Option<String>,
    pub scoped_insert: Option<String>,
    pub output_table: Option<String>,
    pub target_source_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRustConfig {
    pub id: StepId,
    pub label: String,
    pub assembly_id: String,
    pub config: Value,
    pub output_table: Option<String>,
    pub target_source_id: Option<String>,
}

// ── Step ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Step {
    PgExtract {
        config: PgExtractConfig,
        retries: u32,
        on_failure: OnFailure,
    },
    DuckDbLoad {
        config: DuckDbLoadConfig,
        retries: u32,
        on_failure: OnFailure,
    },
    DuckDbQuery {
        config: DuckDbQueryConfig,
        retries: u32,
        on_failure: OnFailure,
    },
    CustomRust {
        config: CustomRustConfig,
        retries: u32,
        on_failure: OnFailure,
    },
    Group {
        kind: GroupKind,
        children: Vec<Step>,
    },
}

impl Step {
    /// A fresh sequential group with no children.
    pub fn sequence() -> Step {
        Step::Group {
            kind: GroupKind::Sequence,
            children: Vec::new(),
        }
    }

    /// Append a child to a group step (no-op on leaf steps).
    pub fn add_step<T: Into<Step>>(mut self, step: T) -> Self {
        if let Step::Group { children, .. } = &mut self {
            children.push(step.into());
        }
        self
    }
}

impl From<PgExtractConfig> for Step {
    fn from(config: PgExtractConfig) -> Self {
        Step::PgExtract {
            config,
            retries: 0,
            on_failure: OnFailure::Abort,
        }
    }
}
impl From<DuckDbLoadConfig> for Step {
    fn from(config: DuckDbLoadConfig) -> Self {
        Step::DuckDbLoad {
            config,
            retries: 0,
            on_failure: OnFailure::Abort,
        }
    }
}
impl From<DuckDbQueryConfig> for Step {
    fn from(config: DuckDbQueryConfig) -> Self {
        Step::DuckDbQuery {
            config,
            retries: 0,
            on_failure: OnFailure::Abort,
        }
    }
}
impl From<CustomRustConfig> for Step {
    fn from(config: CustomRustConfig) -> Self {
        Step::CustomRust {
            config,
            retries: 0,
            on_failure: OnFailure::Abort,
        }
    }
}

// ── Events ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StepEvent {
    pub id: String,
    pub step_type: String,
    pub label: String,
    pub status: StepStatus,
    pub message: String,
    pub row_count: i64,
    pub duration_ms: u64,
}

// ── Assembly dispatch ────────────────────────────────────────────────────────

/// Dependencies handed to a custom-rust assembly. Constructed by the server and
/// cloned into each assembly invocation.
#[derive(Clone)]
pub struct AssemblyDeps {
    pub pg_pool_name: String,
    pub connection_map: HashMap<ConnectionRefId, String>,
    pub event_tx: UnboundedSender<StepEvent>,
    pub step_id: String,
    pub label: String,
    pub partial_recompute_keys: Vec<String>,
    pub cancel: CancellationToken,
}

/// Implemented by the server (`pipeline_assemblies.rs`). Signature must match
/// the real trait exactly so the server's `impl` block compiles.
pub trait AssemblyDispatcher: Send + Sync {
    fn dispatch<'a>(
        &'a self,
        assembly_id: &'a str,
        config: &'a Value,
        deps: AssemblyDeps,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<i64>> + Send + 'a>>;
}

// ── Execution context & result ───────────────────────────────────────────────

/// All the runtime handles an execution needs. Constructed field-by-field by
/// the server; consumed (ignored) by the inert `execute`.
pub struct ExecutionContext {
    pub pg_pool_name: String,
    pub parquet_home: String,
    pub data_dir: String,
    pub event_tx: UnboundedSender<StepEvent>,
    pub connection_map: HashMap<ConnectionRefId, String>,
    pub progress_interval: Option<Duration>,
    pub quantify: bool,
    pub assembly_dispatcher: Option<Arc<dyn AssemblyDispatcher>>,
    pub partial_recompute_keys: Vec<String>,
    pub tenant_attach_path: Option<String>,
    pub cancel: CancellationToken,
}

/// Handle to the DuckDB output of a run. The real crate also carries a live
/// `duckdb::Connection`; the server never reads it, so it is omitted to avoid
/// the `duckdb` dependency.
pub struct DuckDbWriter {
    pub path: PathBuf,
}

/// Manages DuckDB output files. The server only calls the associated
/// `checkpoint` function.
pub struct DuckDbManager {
    _private: (),
}

impl DuckDbManager {
    /// No-op: reports the writer's path as the checkpointed file.
    pub fn checkpoint(writer: DuckDbWriter) -> anyhow::Result<PathBuf> {
        Ok(writer.path)
    }
}

/// What `execute` returns. The server reads only `writer`, `placement`,
/// `total_rows`, and `steps_skipped`.
pub struct PipelineResult {
    pub total_rows: i64,
    pub steps_skipped: usize,
    pub writer: Option<DuckDbWriter>,
    pub placement: Placement,
}

// ── Pipeline builder (typestate) ─────────────────────────────────────────────

pub struct Empty;
pub struct Configured;
pub struct WithSteps;
pub struct Ready;

pub struct Pipeline<S> {
    name: String,
    duckdb_path: String,
    placement: Placement,
    steps: Vec<Step>,
    _state: PhantomData<S>,
}

impl<S> Pipeline<S> {
    fn retype<T>(self) -> Pipeline<T> {
        Pipeline {
            name: self.name,
            duckdb_path: self.duckdb_path,
            placement: self.placement,
            steps: self.steps,
            _state: PhantomData,
        }
    }
}

impl Pipeline<Empty> {
    pub fn new(name: &str) -> Pipeline<Empty> {
        Pipeline {
            name: name.to_string(),
            duckdb_path: String::new(),
            placement: Placement::default(),
            steps: Vec::new(),
            _state: PhantomData,
        }
    }

    pub fn with_duckdb_output(mut self, path: &str) -> Pipeline<Configured> {
        self.duckdb_path = path.to_string();
        self.placement = Placement::default();
        self.retype()
    }
}

impl Pipeline<Configured> {
    pub fn with_output_placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    pub fn add_step<T: Into<Step>>(mut self, step: T) -> Pipeline<WithSteps> {
        self.steps.push(step.into());
        self.retype()
    }
}

impl Pipeline<WithSteps> {
    pub fn add_step<T: Into<Step>>(mut self, step: T) -> Pipeline<WithSteps> {
        self.steps.push(step.into());
        self
    }

    pub fn build(self) -> Pipeline<Ready> {
        self.retype()
    }
}

impl Pipeline<Ready> {
    /// Inert: does no extraction/loading, returns an empty result.
    pub async fn execute(&self, _ctx: &ExecutionContext) -> anyhow::Result<PipelineResult> {
        Ok(PipelineResult {
            total_rows: 0,
            steps_skipped: 0,
            writer: None,
            placement: self.placement,
        })
    }
}

// ── Triggers (faithful — plumbing, not proprietary logic) ────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PipelineTrigger {
    #[default]
    Manual,
    Scheduled {
        cron: String,
    },
    Cdc {
        source_ids: Vec<String>,
    },
    RclChange,
    Composed {
        triggers: Vec<PipelineTrigger>,
    },
}

impl PipelineTrigger {
    /// All CDC source ids this trigger listens to, recursing into `Composed`.
    pub fn cdc_source_ids(&self) -> Vec<&str> {
        match self {
            PipelineTrigger::Cdc { source_ids } => {
                source_ids.iter().map(|s| s.as_str()).collect()
            }
            PipelineTrigger::Composed { triggers } => {
                triggers.iter().flat_map(|t| t.cdc_source_ids()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Whether this trigger fires on RCL rule changes, recursing into `Composed`.
    pub fn listens_for_rcl(&self) -> bool {
        match self {
            PipelineTrigger::RclChange => true,
            PipelineTrigger::Composed { triggers } => {
                triggers.iter().any(|t| t.listens_for_rcl())
            }
            _ => false,
        }
    }

    /// The cron expression if this is a `Scheduled` trigger, else `None`.
    /// Matches the real crate's accessor (used by the scheduled-trigger path,
    /// which is currently disabled in `pipeline_scheduler.rs`).
    pub fn cron(&self) -> Option<&str> {
        match self {
            PipelineTrigger::Scheduled { cron } => Some(cron),
            _ => None,
        }
    }
}
