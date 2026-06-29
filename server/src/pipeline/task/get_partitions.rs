use crate::pipeline::context::PipelineContext;
use crate::pipeline::task::TaskOutput;
use anyhow::{Result, anyhow};
use serde_json::json;

/// Discovers distinct partition values directly from PG using a lightweight query.
/// Does NOT use the full DataView source query — queries the base table directly.
/// Stores values in ctx.metadata["partition_values"] for the loop step.
///
/// Reusable: the same partition values (e.g., l1_name from product_attributes_filter)
/// can be shared across multiple DataView pipelines.
pub struct GetPartitionsTask {
    pub partition_col: String,
    /// Direct PG query to get distinct values. Should be lightweight:
    /// e.g., "SELECT DISTINCT l1_name FROM global.product_attributes_filter"
    pub partition_query: Option<String>,
}

impl GetPartitionsTask {
    pub fn new(partition_col: String) -> Self {
        GetPartitionsTask { partition_col, partition_query: None }
    }

    pub fn with_query(mut self, query: String) -> Self {
        self.partition_query = Some(query);
        self
    }

    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<TaskOutput> {
        let conn_str = ctx.pg_conn_str.as_deref()
            .ok_or_else(|| anyhow!("No PG connection in context"))?;

        let col = self.partition_col.clone();

        // Use explicit partition query if provided, otherwise derive from source SQL
        let sql = if let Some(ref pq) = self.partition_query {
            pq.clone()
        } else if let Some(ref source_sql) = ctx.source_sql {
            // Fallback: wrap source query (less efficient)
            format!("SELECT DISTINCT \"{}\" FROM ({}) AS _src WHERE \"{}\" IS NOT NULL ORDER BY 1", col, source_sql, col)
        } else {
            return Err(anyhow!("No partition_query or source_sql available"));
        };

        // Direct PG query — lightweight, no DuckDB overhead
        let dsn = conn_str.to_string();
        let values = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            // Use a simple synchronous PG connection for this lightweight query
            // But since we need async, we'll use DuckDB postgres_scanner
            let db = duckdb::Connection::open_in_memory()?;
            db.execute_batch("INSTALL postgres; LOAD postgres;")
                .map_err(|e| anyhow!("postgres extension: {}", e))?;

            let escaped_dsn = dsn.replace('\'', "''");
            db.execute_batch(&format!("ATTACH '{}' AS _pg (TYPE postgres, READ_ONLY)", escaped_dsn))
                .map_err(|e| anyhow!("ATTACH: {}", e))?;

            let escaped_sql = sql.replace('\'', "''");
            let query = format!("SELECT * FROM postgres_query('_pg', '{}')", escaped_sql);

            let mut stmt = db.prepare(&query)?;
            let mut rows = stmt.query([])?;
            let mut result = Vec::new();
            while let Some(row) = rows.next()? {
                if let Some(v) = row.get::<_, Option<String>>(0)? {
                    result.push(v);
                }
            }
            db.execute_batch("DETACH _pg").ok();
            Ok(result)
        }).await??;

        let count = values.len();
        ctx.metadata.insert("partition_col".to_string(), json!(self.partition_col));
        ctx.metadata.insert("partition_values".to_string(), json!(values));

        let mut output = TaskOutput::success(&format!("{} partition values for '{}'", count, self.partition_col))
            .with_rows(count as i64);
        output.extra = Some(json!({"partition_col": self.partition_col, "count": count}));
        Ok(output)
    }
}
