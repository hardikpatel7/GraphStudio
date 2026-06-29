use serde_json::Value;
use std::collections::HashMap;

/// Shared state passed between pipeline steps.
/// Output of one step becomes input for the next.
pub struct PipelineContext {
    /// SQL query produced by extract step, consumed by write step
    pub source_sql: Option<String>,
    /// PG connection string (set by extract step or pipeline builder)
    pub pg_conn_str: Option<String>,
    /// Path where parquet was written (set by write step)
    pub output_path: Option<String>,
    /// Row count from the last step that produced rows
    pub row_count: Option<i64>,
    /// Arbitrary metadata for task communication
    pub metadata: HashMap<String, Value>,
}

impl PipelineContext {
    pub fn new() -> Self {
        PipelineContext {
            source_sql: None,
            pg_conn_str: None,
            output_path: None,
            row_count: None,
            metadata: HashMap::new(),
        }
    }
}
