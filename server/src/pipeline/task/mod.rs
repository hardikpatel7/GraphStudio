pub mod pg_extract;
pub mod write_parquet;
pub mod get_partitions;
pub mod loop_write_partitions;
pub mod register_snapshot;
pub mod bq_export;
pub mod duckdb_transform;

use crate::pipeline::context::PipelineContext;
use anyhow::Result;
use serde_json::{json, Value};

/// Output from a task execution.
pub struct TaskOutput {
    pub message: String,
    pub row_count: Option<i64>,
    pub output_path: Option<String>,
    pub extra: Option<Value>,
}

impl TaskOutput {
    pub fn success(message: &str) -> Self {
        TaskOutput { message: message.to_string(), row_count: None, output_path: None, extra: None }
    }

    pub fn with_rows(mut self, count: i64) -> Self { self.row_count = Some(count); self }
    pub fn with_path(mut self, path: &str) -> Self { self.output_path = Some(path.to_string()); self }

    pub fn to_json(&self) -> Value {
        let mut v = json!({"message": self.message});
        if let Some(rc) = self.row_count { v["row_count"] = json!(rc); }
        if let Some(ref p) = self.output_path { v["output_path"] = json!(p); }
        if let Some(ref e) = self.extra { v["extra"] = e.clone(); }
        v
    }
}

/// Predefined task types — enum dispatch avoids async trait object complexity.
pub enum Task {
    PgExtract(pg_extract::PgExtractTask),
    WriteParquet(write_parquet::WriteParquetTask),
    GetPartitions(get_partitions::GetPartitionsTask),
    LoopWritePartitions(loop_write_partitions::LoopWritePartitionsTask),
    RegisterSnapshot(register_snapshot::RegisterSnapshotTask),
    BqExport(bq_export::BqExportTask),
    DuckDbTransform(duckdb_transform::DuckDbTransformTask),
}

impl Task {
    pub fn task_type(&self) -> &str {
        match self {
            Task::PgExtract(_) => "pg_extract",
            Task::WriteParquet(_) => "write_parquet",
            Task::GetPartitions(_) => "get_partitions",
            Task::LoopWritePartitions(_) => "loop_write_partitions",
            Task::RegisterSnapshot(_) => "register_snapshot",
            Task::BqExport(_) => "bq_export",
            Task::DuckDbTransform(_) => "duckdb_transform",
        }
    }

    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<TaskOutput> {
        match self {
            Task::PgExtract(t) => t.execute(ctx).await,
            Task::WriteParquet(t) => t.execute(ctx).await,
            Task::GetPartitions(t) => t.execute(ctx).await,
            Task::LoopWritePartitions(t) => t.execute(ctx).await,
            Task::RegisterSnapshot(t) => t.execute(ctx).await,
            Task::BqExport(t) => t.execute(ctx).await,
            Task::DuckDbTransform(t) => t.execute(ctx).await,
        }
    }
}
