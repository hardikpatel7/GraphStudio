use crate::pipeline::context::PipelineContext;
use crate::pipeline::task::TaskOutput;
use anyhow::{Result, anyhow};

/// Connects to PostgreSQL, validates the query, and stores the SQL + connection
/// in the context for the next step (WriteParquet) to consume.
pub struct PgExtractTask {
    pub conn_str: String,
    pub sql: String,
}

impl PgExtractTask {
    pub fn new(conn_str: String, sql: String) -> Self {
        PgExtractTask { conn_str, sql }
    }

    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<TaskOutput> {
        if self.conn_str.is_empty() {
            return Err(anyhow!("No PostgreSQL connection string"));
        }
        if self.sql.is_empty() {
            return Err(anyhow!("No source SQL configured"));
        }

        // Validate by counting rows
        let (client, conn) = tokio_postgres::connect(&self.conn_str, tokio_postgres::NoTls).await
            .map_err(|e| anyhow!("PG connection failed: {}", e))?;
        tokio::spawn(async move { conn.await.ok(); });

        let count_sql = format!("SELECT COUNT(*) FROM ({}) AS _c", self.sql);
        let row = client.query_one(&count_sql, &[]).await
            .map_err(|e| {
                let detail = if let Some(db_err) = e.as_db_error() {
                    format!("{}: {}", db_err.severity(), db_err.message())
                } else {
                    e.to_string()
                };
                anyhow!("PG query failed: {} (query: {}...)", detail, &self.sql[..self.sql.len().min(100)])
            })?;
        let count: i64 = row.get(0);

        // Store in context for next step
        ctx.source_sql = Some(self.sql.clone());
        ctx.pg_conn_str = Some(self.conn_str.clone());
        ctx.row_count = Some(count);

        Ok(TaskOutput::success(&format!("Query OK, {} rows", count)).with_rows(count))
    }
}
