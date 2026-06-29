use crate::pipeline::context::PipelineContext;
use crate::pipeline::task::TaskOutput;
use anyhow::{Result, anyhow};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use tokio::sync::mpsc;

/// Loop step: iterates over partition values, running read+write per partition.
/// Each partition does: PG query (base table + WHERE partition_col = X) → parquet.
/// Reports per-iteration progress via an optional channel.
pub struct LoopWritePartitionsTask {
    pub output_path: String,
    pub max_concurrency: usize,
    /// Optional: explicit extract query for each partition.
    /// If set, the partition filter is appended to this query.
    /// If not set, uses ctx.source_sql (the full DataView query — less efficient).
    pub extract_query: Option<String>,
}

/// Progress event emitted per partition iteration.
#[derive(Debug, Clone)]
pub struct PartitionProgress {
    pub partition_value: String,
    pub index: usize,
    pub total: usize,
    pub status: &'static str,  // "running", "success", "failed"
    pub row_count: i64,
    pub duration_ms: u64,
    pub message: String,
}

impl LoopWritePartitionsTask {
    pub fn new(output_path: String, max_concurrency: usize) -> Self {
        LoopWritePartitionsTask { output_path, max_concurrency, extract_query: None }
    }

    pub fn with_extract_query(mut self, query: String) -> Self {
        self.extract_query = Some(query);
        self
    }

    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<TaskOutput> {
        self.execute_with_progress(ctx, None).await
    }

    /// Execute with optional progress channel for SSE streaming.
    pub async fn execute_with_progress(
        &self,
        ctx: &mut PipelineContext,
        progress_tx: Option<mpsc::Sender<PartitionProgress>>,
    ) -> Result<TaskOutput> {
        let conn_str = ctx.pg_conn_str.as_deref()
            .ok_or_else(|| anyhow!("No PG connection in context"))?;
        // Use explicit extract query (base table) if provided, otherwise fall back to source_sql (full join)
        let extract_sql = self.extract_query.as_deref()
            .or(ctx.source_sql.as_deref())
            .ok_or_else(|| anyhow!("No extract query or source SQL in context"))?;

        let partition_col = ctx.metadata.get("partition_col")
            .and_then(|v| v.as_str()).ok_or_else(|| anyhow!("No partition_col in context (did GetPartitions step run?)"))?
            .to_string();
        let partition_values: Vec<String> = ctx.metadata.get("partition_values")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        if partition_values.is_empty() {
            ctx.output_path = Some(self.output_path.clone());
            ctx.row_count = Some(0);
            return Ok(TaskOutput::success("No partition values to write").with_rows(0));
        }

        let total = partition_values.len();
        let total_rows = Arc::new(AtomicI64::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.max_concurrency));

        // Clear output directory
        if std::path::Path::new(&self.output_path).exists() {
            std::fs::remove_dir_all(&self.output_path).ok();
        }
        std::fs::create_dir_all(&self.output_path)?;

        let mut handles = Vec::new();

        for (i, value) in partition_values.iter().enumerate() {
            let sem = semaphore.clone();
            let rows_counter = total_rows.clone();
            let done_counter = completed.clone();
            let dsn = conn_str.to_string();
            let sql = extract_sql.to_string();
            let col = partition_col.clone();
            let val = value.clone();
            let out_dir = self.output_path.clone();
            let ptx = progress_tx.clone();
            let total_partitions = total;

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                // Report start
                if let Some(ref tx) = ptx {
                    tx.send(PartitionProgress {
                        partition_value: val.clone(), index: i, total: total_partitions,
                        status: "running", row_count: 0, duration_ms: 0, message: String::new(),
                    }).await.ok();
                }

                let start = std::time::Instant::now();
                let result = write_single_partition(&dsn, &sql, &col, &val, &out_dir).await;
                let duration_ms = start.elapsed().as_millis() as u64;

                match result {
                    Ok(row_count) => {
                        rows_counter.fetch_add(row_count, Ordering::SeqCst);
                        let done = done_counter.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Some(ref tx) = ptx {
                            tx.send(PartitionProgress {
                                partition_value: val.clone(), index: i, total: total_partitions,
                                status: "success", row_count, duration_ms,
                                message: format!("{}/{} done", done, total_partitions),
                            }).await.ok();
                        }
                        Ok(())
                    }
                    Err(e) => {
                        done_counter.fetch_add(1, Ordering::SeqCst);
                        if let Some(ref tx) = ptx {
                            tx.send(PartitionProgress {
                                partition_value: val.clone(), index: i, total: total_partitions,
                                status: "failed", row_count: 0, duration_ms,
                                message: e.to_string(),
                            }).await.ok();
                        }
                        Err(e)
                    }
                }
            });
            handles.push((value.clone(), handle));
        }

        // Collect results
        let mut errors = Vec::new();
        for (value, handle) in handles {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => errors.push(format!("{}={}: {}", partition_col, value, e)),
                Err(e) => errors.push(format!("{}={}: join: {}", partition_col, value, e)),
            }
        }

        let row_count = total_rows.load(Ordering::SeqCst);
        ctx.output_path = Some(self.output_path.clone());
        ctx.row_count = Some(row_count);

        if !errors.is_empty() {
            return Err(anyhow!("{} partitions failed:\n{}", errors.len(), errors.join("\n")));
        }

        let mut output = TaskOutput::success(&format!(
            "{} rows across {} partitions ({}x parallel)",
            row_count, total, self.max_concurrency
        )).with_rows(row_count).with_path(&self.output_path);
        output.extra = Some(json!({"partitions": total, "concurrency": self.max_concurrency}));
        Ok(output)
    }
}

/// Read from PG + write parquet for a single partition value
async fn write_single_partition(
    conn_str: &str, source_sql: &str, partition_col: &str, partition_value: &str, output_dir: &str,
) -> Result<i64> {
    let dsn = conn_str.to_string();
    let sql = source_sql.to_string();
    let col = partition_col.to_string();
    let val = partition_value.to_string();
    let out = output_dir.to_string();

    tokio::task::spawn_blocking(move || -> Result<i64> {
        let db = duckdb::Connection::open_in_memory()?;
        db.execute_batch("INSTALL postgres; LOAD postgres;")
            .map_err(|e| anyhow!("postgres ext: {}", e))?;

        let escaped_dsn = dsn.replace('\'', "''");
        db.execute_batch(&format!("ATTACH '{}' AS _pg (TYPE postgres, READ_ONLY)", escaped_dsn))
            .map_err(|e| anyhow!("ATTACH: {}", e))?;

        let escaped_sql = sql.replace('\'', "''");
        let escaped_val = val.replace('\'', "''");

        // Hive partition directory
        let partition_dir = format!("{}/{}={}", out, col, val);
        std::fs::create_dir_all(&partition_dir)?;
        let out_file = format!("{}/data.parquet", partition_dir);

        let copy_sql = format!(
            "COPY (SELECT * FROM postgres_query('_pg', '{}') WHERE \"{}\" = '{}') TO '{}' (FORMAT PARQUET, COMPRESSION SNAPPY)",
            escaped_sql, col, escaped_val, out_file
        );
        db.execute_batch(&copy_sql)
            .map_err(|e| anyhow!("COPY {}={}: {}", col, val, e))?;

        let row_count: i64 = db.query_row(
            &format!("SELECT COUNT(*) FROM read_parquet('{}')", out_file), [], |r| r.get(0)
        ).unwrap_or(0);

        db.execute_batch("DETACH _pg").ok();
        Ok(row_count)
    }).await?
}
