use crate::pipeline::context::PipelineContext;
use crate::pipeline::task::TaskOutput;
use anyhow::Result;
use serde_json::Value;

/// Post-read DuckDB transformation (aggregation, CTE, etc.).
/// Currently skipped — transforms are applied at query time.
pub struct DuckDbTransformTask {
    pub config: Value,
}

impl DuckDbTransformTask {
    pub fn new(config: Value) -> Self {
        DuckDbTransformTask { config }
    }

    pub async fn execute(&self, _ctx: &mut PipelineContext) -> Result<TaskOutput> {
        let name = self.config.get("name").and_then(|v| v.as_str()).unwrap_or("transform");
        Ok(TaskOutput::success(&format!("Transform '{}' applied at query time", name)))
    }
}
