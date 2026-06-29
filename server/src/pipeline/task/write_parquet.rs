use crate::pipeline::context::PipelineContext;
use crate::pipeline::task::TaskOutput;
use anyhow::{Result, anyhow};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// Write mode: single-pass DuckDB COPY vs parallel per-partition threads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WriteMode {
    /// DuckDB handles partitioning in a single COPY with PARTITION_BY
    SinglePass,
    /// One thread per partition value, each with its own DuckDB+PG connection
    Parallel,
}

/// Reads from PG (via DuckDB postgres_scanner) and writes Hive-partitioned parquet.
pub struct WriteParquetTask {
    pub output_path: String,
    pub partition_by: Vec<String>,
    pub mode: WriteMode,
}

impl WriteParquetTask {
    pub fn new(output_path: String, partition_by: Vec<String>) -> Self {
        // Default: parallel if partitioned, single-pass otherwise
        let mode = if partition_by.is_empty() { WriteMode::SinglePass } else { WriteMode::Parallel };
        WriteParquetTask { output_path, partition_by, mode }
    }

    pub fn with_mode(mut self, mode: WriteMode) -> Self {
        self.mode = mode;
        self
    }

    pub async fn execute(&self, ctx: &mut PipelineContext) -> Result<TaskOutput> {
        let conn_str = ctx.pg_conn_str.as_deref()
            .ok_or_else(|| anyhow!("No PG connection in context (did PgExtract step run?)"))?;
        let source_sql = ctx.source_sql.as_deref()
            .ok_or_else(|| anyhow!("No source SQL in context (did PgExtract step run?)"))?;

        // Clear and create output directory
        if std::path::Path::new(&self.output_path).exists() {
            std::fs::remove_dir_all(&self.output_path).ok();
        }
        std::fs::create_dir_all(&self.output_path)?;

        if self.partition_by.is_empty() || self.mode == WriteMode::SinglePass {
            // Single-pass: DuckDB handles everything (including PARTITION_BY if set)
            let (out_file, row_count) = write_single_pass(
                conn_str, source_sql, &self.output_path, &self.partition_by,
            ).await?;
            let mode_label = if self.partition_by.is_empty() { "single-pass" } else { "single-pass with PARTITION_BY" };
            ctx.output_path = Some(out_file.clone());
            ctx.row_count = Some(row_count);
            return Ok(TaskOutput::success(&format!("Wrote {} rows to parquet ({})", row_count, mode_label))
                .with_rows(row_count).with_path(&out_file));
        }

        // Parallel mode: one thread per partition value
        let partition_col = &self.partition_by[0];

        // Step 1: Get distinct partition values from PG
        let distinct_values = get_distinct_values(conn_str, source_sql, partition_col).await?;
        let num_partitions = distinct_values.len();

        if num_partitions == 0 {
            ctx.output_path = Some(self.output_path.clone());
            ctx.row_count = Some(0);
            return Ok(TaskOutput::success("Source returned 0 partition values").with_rows(0));
        }

        tracing::info!("Parallel write: {} partitions on '{}' with {} threads",
            num_partitions, partition_col, num_partitions.min(8));

        // Step 2: Spawn parallel tasks — one per partition value
        let total_rows = Arc::new(AtomicI64::new(0));
        let mut handles = Vec::new();

        // Limit concurrency to 8 threads
        let semaphore = Arc::new(tokio::sync::Semaphore::new(8));

        for value in distinct_values {
            let sem = semaphore.clone();
            let rows_counter = total_rows.clone();
            let dsn = conn_str.to_string();
            let sql = source_sql.to_string();
            let col = partition_col.to_string();
            let val = value.clone();
            let out_dir = self.output_path.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                write_partition(&dsn, &sql, &col, &val, &out_dir, &rows_counter).await
            });
            handles.push((value, handle));
        }

        // Step 3: Collect results
        let mut errors = Vec::new();
        for (value, handle) in handles {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => errors.push(format!("{}={}: {}", partition_col, value, e)),
                Err(e) => errors.push(format!("{}={}: join error: {}", partition_col, value, e)),
            }
        }

        let row_count = total_rows.load(Ordering::SeqCst);

        if !errors.is_empty() {
            return Err(anyhow!("Parallel write failed for {} partitions:\n{}", errors.len(), errors.join("\n")));
        }

        ctx.output_path = Some(self.output_path.clone());
        ctx.row_count = Some(row_count);

        let mut output = TaskOutput::success(&format!(
            "Wrote {} rows across {} partitions (parallel)", row_count, num_partitions
        )).with_rows(row_count).with_path(&self.output_path);
        output.extra = Some(json!({"partitions": num_partitions, "parallel": true}));
        Ok(output)
    }
}

/// Single-pass write — DuckDB handles partitioning via PARTITION_BY if columns provided
async fn write_single_pass(conn_str: &str, source_sql: &str, output_path: &str, partition_by: &[String]) -> Result<(String, i64)> {
    let pg_dsn = conn_str.to_string();
    let sql = source_sql.to_string();
    let output = output_path.to_string();
    let partition = partition_by.to_vec();

    tokio::task::spawn_blocking(move || -> Result<(String, i64)> {
        let db = duckdb::Connection::open_in_memory()?;
        db.execute_batch("INSTALL postgres; LOAD postgres;")
            .map_err(|e| anyhow!("Failed to load postgres extension: {}", e))?;

        let escaped_sql = sql.replace('\'', "''");
        let escaped_dsn = pg_dsn.replace('\'', "''");

        db.execute_batch(&format!("ATTACH '{}' AS _pg (TYPE postgres, READ_ONLY)", escaped_dsn))
            .map_err(|e| anyhow!("ATTACH failed: {}", e))?;

        let out_file = if partition.is_empty() {
            format!("{}/data.parquet", output)
        } else {
            output.clone()
        };
        let part_clause = if partition.is_empty() {
            String::new()
        } else {
            format!(", PARTITION_BY ({})", partition.join(", "))
        };

        db.execute_batch(&format!(
            "COPY (SELECT * FROM postgres_query('_pg', '{}')) TO '{}' (FORMAT PARQUET, COMPRESSION SNAPPY{})",
            escaped_sql, out_file, part_clause
        )).map_err(|e| anyhow!("COPY failed: {}", e))?;

        db.execute_batch("DETACH _pg").ok();

        let glob = format!("{}/**/*.parquet", output);
        let count_sql = if partition.is_empty() {
            format!("SELECT COUNT(*) FROM read_parquet('{}')", out_file)
        } else {
            format!("SELECT COUNT(*) FROM read_parquet('{}', hive_partitioning=true)", glob)
        };
        let row_count: i64 = db.query_row(&count_sql, [], |r| r.get(0)).unwrap_or(0);

        Ok((out_file, row_count))
    }).await?
}

/// Get distinct values for the partition column from the source query
async fn get_distinct_values(conn_str: &str, source_sql: &str, partition_col: &str) -> Result<Vec<String>> {
    let dsn = conn_str.to_string();
    let sql = source_sql.to_string();
    let col = partition_col.to_string();

    tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        let db = duckdb::Connection::open_in_memory()?;
        db.execute_batch("INSTALL postgres; LOAD postgres;")
            .map_err(|e| anyhow!("Failed to load postgres extension: {}", e))?;

        let escaped_dsn = dsn.replace('\'', "''");
        db.execute_batch(&format!("ATTACH '{}' AS _pg (TYPE postgres, READ_ONLY)", escaped_dsn))
            .map_err(|e| anyhow!("ATTACH failed: {}", e))?;

        let escaped_sql = sql.replace('\'', "''");
        let distinct_sql = format!(
            "SELECT DISTINCT \"{}\" FROM postgres_query('_pg', '{}') WHERE \"{}\" IS NOT NULL ORDER BY 1",
            col, escaped_sql, col
        );

        let mut stmt = db.prepare(&distinct_sql)
            .map_err(|e| anyhow!("DISTINCT query failed: {}", e))?;
        let mut rows = stmt.query([])
            .map_err(|e| anyhow!("DISTINCT execution failed: {}", e))?;

        let mut values = Vec::new();
        while let Some(row) = rows.next()? {
            if let Some(v) = row.get::<_, Option<String>>(0)? {
                values.push(v);
            }
        }
        db.execute_batch("DETACH _pg").ok();
        Ok(values)
    }).await?
}

/// Write a single partition: query PG for rows matching partition value → write to hive-partitioned folder
async fn write_partition(
    conn_str: &str,
    source_sql: &str,
    partition_col: &str,
    partition_value: &str,
    output_dir: &str,
    total_rows: &Arc<AtomicI64>,
) -> Result<()> {
    let dsn = conn_str.to_string();
    let sql = source_sql.to_string();
    let col = partition_col.to_string();
    let val = partition_value.to_string();
    let out = output_dir.to_string();
    let counter = total_rows.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let db = duckdb::Connection::open_in_memory()?;
        db.execute_batch("INSTALL postgres; LOAD postgres;")
            .map_err(|e| anyhow!("postgres extension: {}", e))?;

        let escaped_dsn = dsn.replace('\'', "''");
        db.execute_batch(&format!("ATTACH '{}' AS _pg (TYPE postgres, READ_ONLY)", escaped_dsn))
            .map_err(|e| anyhow!("ATTACH: {}", e))?;

        // Build filtered query: wrap source query and add WHERE partition_col = 'value'
        let escaped_val = val.replace('\'', "''''");
        let escaped_sql = sql.replace('\'', "''");
        let filtered_sql = format!(
            "SELECT * FROM postgres_query(''_pg'', ''{}'') WHERE \"{}\" = ''{}''",
            escaped_sql, col, escaped_val
        );

        // Hive partition directory: output_dir/col=value/data.parquet
        let partition_dir = format!("{}/{}={}", out, col, val);
        std::fs::create_dir_all(&partition_dir)?;
        let out_file = format!("{}/data.parquet", partition_dir);

        let copy_sql = format!(
            "COPY (SELECT * FROM postgres_query('_pg', '{}') WHERE \"{}\" = '{}') TO '{}' (FORMAT PARQUET, COMPRESSION SNAPPY)",
            escaped_sql, col, escaped_val, out_file
        );

        db.execute_batch(&copy_sql)
            .map_err(|e| anyhow!("COPY partition {}={}: {}", col, val, e))?;

        // Count rows
        let row_count: i64 = db.query_row(
            &format!("SELECT COUNT(*) FROM read_parquet('{}')", out_file), [], |r| r.get(0)
        ).unwrap_or(0);

        counter.fetch_add(row_count, Ordering::SeqCst);
        db.execute_batch("DETACH _pg").ok();

        tracing::debug!("Partition {}={}: {} rows", col, val, row_count);
        Ok(())
    }).await?
}
