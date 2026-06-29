use crate::pipeline::context::PipelineContext;
use crate::pipeline::task::TaskOutput;
use anyhow::Result;

/// Registers the output path as a snapshot in the trace DB.
/// This task is added by the snapshot handler, not by Pipeline::from_workflow.
pub struct RegisterSnapshotTask {
    pub dataview_id: String,
    pub step: String,  // "gcs" or "local"
}

impl RegisterSnapshotTask {
    pub fn new(dataview_id: String, step: String) -> Self {
        RegisterSnapshotTask { dataview_id, step }
    }

    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<TaskOutput> {
        let path = ctx.output_path.as_deref().unwrap_or("unknown");
        let row_count = ctx.row_count.unwrap_or(0);
        Ok(TaskOutput::success(&format!("Snapshot registered: {} ({} rows)", path, row_count))
            .with_rows(row_count)
            .with_path(path))
    }
}
