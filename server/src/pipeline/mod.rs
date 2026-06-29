// ────────────────────────────────────────────────────────────────────────────
// LEGACY-PIPELINE: in-tree pipeline executor. New shared-pipeline runs go
// through `handlers::pipeline_v2`, which delegates to the `pipeline` crate
// from rust-shared-utils. This module survives because three callers still
// use the lightweight `execute` / `exec_write_parquet_public` wrappers
// below: `handlers::snapshots`, `handlers::ingest`, `handlers::parquet_browse`.
// Drop this module after those three callers are migrated to the new crate.
// ────────────────────────────────────────────────────────────────────────────
pub mod task;
mod step;
mod context;

pub use context::PipelineContext;
pub use step::Step;

use anyhow::Result;
use serde_json::{json, Value};
use std::time::Instant;

use task::Task;
use task::pg_extract::PgExtractTask;
use task::write_parquet::{WriteParquetTask, WriteMode};
use task::get_partitions::GetPartitionsTask;
use task::loop_write_partitions::LoopWritePartitionsTask;
use task::bq_export::BqExportTask;
use task::duckdb_transform::DuckDbTransformTask;

/// Pipeline = ordered list of Steps. Each Step executes one Task.
pub struct Pipeline {
    pub steps: Vec<Step>,
    /// PG connection string — set in context before step execution
    pub pg_conn_str: Option<String>,
    /// Source SQL — set in context for tasks that need it
    pub source_sql: Option<String>,
}

impl Pipeline {
    /// Build a Pipeline from a DataView's backend_workflow metadata.
    pub fn from_workflow(workflow: &Value, pg_conn_str: Option<&str>, parquet_home: &str) -> Self {
        let mut steps = Vec::new();
        let source = workflow.get("source");
        let parquet = workflow.get("parquet");

        let src = source.cloned().unwrap_or(json!({}));
        let src_type = src.get("type").and_then(|v| v.as_str()).unwrap_or("");

        // Derive the source SQL (DataView query — used for single-pass or DuckDB read-time join)
        let source_sql = {
            let explicit = src.get("query").and_then(|v| v.as_str()).unwrap_or("");
            if !explicit.is_empty() {
                explicit.to_string()
            } else if src_type == "pg_sp" {
                let sp = src.get("sp_name").and_then(|v| v.as_str()).unwrap_or("");
                if !sp.is_empty() { format!("SELECT * FROM {}()", sp) } else { String::new() }
            } else {
                String::new()
            }
        };

        // Derive the extract query — the base table query WITHOUT joins.
        // This is what gets executed per-partition for extraction.
        // Falls back to source_sql if not specified separately.
        let extract_query = src.get("extract_query").and_then(|v| v.as_str())
            .unwrap_or("").to_string();

        // Derive the partition discovery query — lightweight query to get distinct values.
        // e.g., "SELECT DISTINCT l1_name FROM global.product_attributes_filter ORDER BY 1"
        let partition_query = src.get("partition_query").and_then(|v| v.as_str())
            .unwrap_or("").to_string();

        // Write parquet
        if let Some(pq) = parquet {
            let rel_path = pq.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let resolved = if rel_path.starts_with('/') || rel_path.starts_with("gs://") {
                rel_path.to_string()
            } else {
                format!("{}/{}", parquet_home.trim_end_matches('/'), rel_path)
            };
            let partition_by: Vec<String> = pq.get("partition_by")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let write_mode = match pq.get("write_mode").and_then(|v| v.as_str()) {
                Some("single_pass") => WriteMode::SinglePass,
                Some("parallel") => WriteMode::Parallel,
                _ => if partition_by.is_empty() { WriteMode::SinglePass } else { WriteMode::Parallel },
            };

            match src_type {
                "bq_export" => {
                    steps.push(Step::new(
                        "extract",
                        src.get("description").and_then(|v| v.as_str()).unwrap_or("Export from BigQuery"),
                        Task::BqExport(BqExportTask::new(src.clone())),
                    ));
                }
                "pg_query" | "pg_sp" | _ if !source_sql.is_empty() => {
                    if write_mode == WriteMode::Parallel && !partition_by.is_empty() {
                        // Parallel: get_partitions → loop per partition (read+write each)
                        let mut get_parts = GetPartitionsTask::new(partition_by[0].clone());
                        if !partition_query.is_empty() {
                            get_parts = get_parts.with_query(partition_query);
                        }
                        // Set PG connection in context via a setup step is not needed —
                        // we'll set it in the PipelineContext before execution.

                        steps.push(Step::new(
                            "get_partitions",
                            &format!("Discover '{}' values from PG", partition_by[0]),
                            Task::GetPartitions(get_parts),
                        ));

                        let mut loop_task = LoopWritePartitionsTask::new(resolved, 8);
                        if !extract_query.is_empty() {
                            loop_task = loop_task.with_extract_query(extract_query);
                        }
                        steps.push(Step::new(
                            "write_partitions",
                            &format!("Read+Write per partition to {} (8x parallel)", rel_path),
                            Task::LoopWritePartitions(loop_task),
                        ));
                    } else {
                        // Single-pass: PgExtract validates, then WriteParquet streams via DuckDB
                        steps.push(Step::new(
                            "extract",
                            src.get("description").and_then(|v| v.as_str()).unwrap_or("Extract from PostgreSQL"),
                            Task::PgExtract(PgExtractTask::new(pg_conn_str.unwrap_or("").to_string(), source_sql.clone())),
                        ));
                        steps.push(Step::new(
                            "write_parquet",
                            &format!("Write parquet to {} (single-pass)", rel_path),
                            Task::WriteParquet(WriteParquetTask::new(resolved, partition_by).with_mode(WriteMode::SinglePass)),
                        ));
                    }
                }
                _ => {}
            }
        }

        // Step 3+: Transforms
        if let Some(transforms) = workflow.get("transform").and_then(|v| v.as_array()) {
            for (i, t) in transforms.iter().enumerate() {
                let name = t.get("name").and_then(|v| v.as_str())
                    .unwrap_or(&format!("transform_{}", i + 1)).to_string();
                let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("DuckDB transform");
                steps.push(Step::new(
                    &name,
                    desc,
                    Task::DuckDbTransform(DuckDbTransformTask::new(t.clone())),
                ));
            }
        }

        Pipeline {
            steps,
            pg_conn_str: pg_conn_str.map(String::from),
            source_sql: if source_sql.is_empty() { None } else { Some(source_sql) },
        }
    }

    /// Execute all steps sequentially, passing context between them.
    pub async fn execute(&self, dataview_id: &str) -> Result<Value> {
        let start = Instant::now();

        if self.steps.is_empty() {
            return Ok(json!({
                "dataview_id": dataview_id,
                "status": "failed",
                "steps": [{"step_id": "none", "status": "failed", "message": "No pipeline steps"}],
                "total_time_ms": 0
            }));
        }

        let mut ctx = PipelineContext::new();
        // Seed context with pipeline-level config
        ctx.pg_conn_str = self.pg_conn_str.clone();
        ctx.source_sql = self.source_sql.clone();
        let mut step_results = Vec::new();
        let mut pipeline_status = "success";

        for step in &self.steps {
            let step_start = Instant::now();
            let output = step.execute(&mut ctx).await;
            let duration_ms = step_start.elapsed().as_millis() as u64;

            let mut result = match output {
                Ok(out) => {
                    let mut r = out.to_json();
                    r["status"] = json!("success");
                    r
                }
                Err(e) => {
                    pipeline_status = "failed";
                    json!({"status": "failed", "message": e.to_string()})
                }
            };

            result["step_id"] = json!(step.id);
            result["description"] = json!(step.description);
            result["task_type"] = json!(step.task.task_type());
            result["duration_ms"] = json!(duration_ms);

            let failed = result["status"] == "failed";
            step_results.push(result);
            if failed { break; }
        }

        Ok(json!({
            "dataview_id": dataview_id,
            "status": pipeline_status,
            "steps": step_results,
            "total_time_ms": start.elapsed().as_millis() as u64,
        }))
    }
}

/// Execute pipeline. Used by `parquet_browse`'s materialize wrapper.
pub async fn execute(
    dataview_id: &str,
    workflow: &Value,
    pg_conn_str: Option<&str>,
    parquet_home: &str,
) -> Result<Value> {
    let pipeline = Pipeline::from_workflow(workflow, pg_conn_str, parquet_home);
    let result = pipeline.execute(dataview_id).await?;
    // Map "steps" → "tasks" for backward compat
    let mut compat = result.clone();
    if let Some(steps) = result.get("steps") {
        compat["tasks"] = steps.clone();
    }
    Ok(compat)
}

/// Public wrapper for direct PG → parquet (used by snapshots handler).
pub async fn exec_write_parquet_public(conn_str: &str, source_sql: &str, output_path: &str) -> Result<Value> {
    let mut ctx = PipelineContext::new();

    // Step 1: PG Extract
    let extract = PgExtractTask::new(conn_str.to_string(), source_sql.to_string());
    extract.execute(&mut ctx).await?;

    // Step 2: Write Parquet
    let write = WriteParquetTask::new(output_path.to_string(), vec![]);
    let output = write.execute(&mut ctx).await?;

    Ok(output.to_json())
}
