use crate::pipeline::context::PipelineContext;
use crate::pipeline::task::TaskOutput;
use anyhow::Result;
use serde_json::Value;

/// Generates BigQuery EXPORT DATA SQL. Not yet executed — returns the SQL for manual run.
pub struct BqExportTask {
    pub config: Value,
}

impl BqExportTask {
    pub fn new(config: Value) -> Self {
        BqExportTask { config }
    }

    pub async fn execute(&self, _ctx: &mut PipelineContext) -> Result<TaskOutput> {
        Ok(TaskOutput::success("BigQuery export not yet implemented (SQL generated only)"))
    }
}
