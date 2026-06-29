use crate::pipeline::context::PipelineContext;
use crate::pipeline::task::{Task, TaskOutput};
use anyhow::Result;

/// A step in the pipeline. Binds a Task with an ID and description.
/// Steps are executed sequentially; each step reads/writes the shared PipelineContext.
pub struct Step {
    pub id: String,
    pub description: String,
    pub task: Task,
}

impl Step {
    pub fn new(id: &str, description: &str, task: Task) -> Self {
        Step {
            id: id.to_string(),
            description: description.to_string(),
            task,
        }
    }

    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<TaskOutput> {
        self.task.execute(ctx).await
    }
}
