//! Feedback queue — LLM-originated requests for SmartStudio capability gaps.
//!
//! Persisted in DuckDB (`feedback` table on the tenant data file). Each entry
//! captures the planner's prompt that triggered it, what tools the LLM tried,
//! and what fell short. Lifecycle: a `status` column with three values
//! (`pending` / `partial` / `addressed`) lets the planner mark entries off
//! as work lands — mutated via PATCH /api/feedback/:id.

use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;

use super::err;
use crate::AppState;

const TABLE_DDL: &str = "
    CREATE TABLE IF NOT EXISTS feedback (
        id VARCHAR PRIMARY KEY,
        created_at TIMESTAMP DEFAULT current_timestamp,
        category VARCHAR NOT NULL,
        summary VARCHAR NOT NULL,
        example_question VARCHAR,
        attempted_path VARCHAR,
        what_was_painful VARCHAR,
        workaround VARCHAR,
        proposed_solution VARCHAR,
        status VARCHAR NOT NULL DEFAULT 'pending'
    );
";

const VALID_STATUSES: &[&str] = &["pending", "partial", "addressed"];

/// Create the feedback table if missing, and back-fill the `status` column on
/// older tenants. Called at boot from main.rs so the first
/// `GET /api/feedback` doesn't 500 against a fresh tenant DuckDB.
pub fn ensure_table(duckdb_path: &str) -> Result<(), String> {
    let db = duckdb::Connection::open(duckdb_path)
        .map_err(|e| format!("DuckDB open: {}", e))?;
    db.execute_batch(TABLE_DDL).map_err(|e| e.to_string())?;

    // Idempotent migration for tenants that already had the table before the
    // status lifecycle landed. DuckDB rejects `ADD COLUMN IF NOT EXISTS`, so
    // we check the schema explicitly and add the column when missing.
    let has_status: bool = db
        .query_row(
            "SELECT count(*) > 0 FROM information_schema.columns \
             WHERE table_name = 'feedback' AND column_name = 'status'",
            [],
            |r| r.get::<_, bool>(0),
        )
        .map_err(|e| e.to_string())?;
    if !has_status {
        // DuckDB rejects `NOT NULL DEFAULT` in ALTER TABLE ADD COLUMN
        // ("Adding columns with constraints not yet supported"), so we add
        // the column with the default only and rely on the application to
        // never write NULL into it. New rows go through `create` which sets
        // 'pending' explicitly.
        db.execute_batch(
            "ALTER TABLE feedback ADD COLUMN status VARCHAR DEFAULT 'pending';"
        ).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `POST /api/feedback` — insert one entry.
///
/// Body: `{ category, summary, example_question?, attempted_path?,
/// what_was_painful?, workaround?, proposed_solution? }`. `attempted_path`
/// may be a JSON array; stored as its JSON string form.
pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();

    let category = body["category"].as_str().unwrap_or("").trim().to_string();
    let summary = body["summary"].as_str().unwrap_or("").trim().to_string();
    if category.is_empty() {
        return Err(err(400, "category is required"));
    }
    if summary.is_empty() {
        return Err(err(400, "summary is required"));
    }

    let example_question = body["example_question"].as_str().unwrap_or("").to_string();
    let what_was_painful = body["what_was_painful"].as_str().unwrap_or("").to_string();
    let workaround = body["workaround"].as_str().unwrap_or("").to_string();
    let proposed_solution = body["proposed_solution"].as_str().unwrap_or("").to_string();
    let attempted_path = match body.get("attempted_path") {
        Some(v) if !v.is_null() => v.to_string(),
        _ => String::new(),
    };

    let now = chrono::Utc::now();
    let id = format!(
        "fb_{}",
        now.timestamp_nanos_opt()
            .unwrap_or_else(|| now.timestamp_millis() * 1_000_000)
    );

    let duckdb_path = state.duckdb_path.clone();
    let id_clone = id.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let db = duckdb::Connection::open(&duckdb_path)
            .map_err(|e| format!("DuckDB open: {}", e))?;
        db.execute(
            "INSERT INTO feedback \
             (id, category, summary, example_question, attempted_path, \
              what_was_painful, workaround, proposed_solution) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            duckdb::params![
                id_clone,
                category,
                summary,
                example_question,
                attempted_path,
                what_was_painful,
                workaround,
                proposed_solution,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| err(500, &format!("Task: {}", e)))?
    .map_err(|e| err(500, &e))?;

    Ok(Json(json!({
        "id": id,
        "duration_ms": t.elapsed().as_millis() as i64,
    })))
}

/// `GET /api/feedback` — newest first.
pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let duckdb_path = state.duckdb_path.clone();
    let rows = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, String> {
        let db = duckdb::Connection::open(&duckdb_path)
            .map_err(|e| format!("DuckDB open: {}", e))?;
        let mut stmt = db
            .prepare(
                "SELECT id, \
                        CAST(created_at AS VARCHAR) AS created_at, \
                        category, summary, example_question, attempted_path, \
                        what_was_painful, workaround, proposed_solution, \
                        COALESCE(status, 'pending') AS status \
                 FROM feedback ORDER BY created_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let frames = stmt.query_arrow(duckdb::params![]).map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        for batch in frames {
            let col_names: Vec<String> = batch
                .schema()
                .fields()
                .iter()
                .map(|f| f.name().clone())
                .collect();
            for row_idx in 0..batch.num_rows() {
                let mut obj = serde_json::Map::new();
                for (col_idx, name) in col_names.iter().enumerate() {
                    obj.insert(
                        name.clone(),
                        crate::query::arrow_to_json(batch.column(col_idx), row_idx),
                    );
                }
                rows.push(Value::Object(obj));
            }
        }
        Ok(rows)
    })
    .await
    .map_err(|e| err(500, &format!("Task: {}", e)))?
    .map_err(|e| err(500, &e))?;

    let count = rows.len();
    Ok(Json(json!({ "feedback": rows, "count": count })))
}

/// `PATCH /api/feedback/:id` — update the lifecycle status. Body: `{ status }`
/// where status is one of `pending` / `partial` / `addressed`. 400 on any
/// other value; 404 if the id doesn't exist.
pub async fn update_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let status = body["status"].as_str().unwrap_or("").trim().to_string();
    if !VALID_STATUSES.contains(&status.as_str()) {
        return Err(err(
            400,
            &format!(
                "status must be one of {} — got {:?}",
                VALID_STATUSES.join(", "),
                status
            ),
        ));
    }

    let duckdb_path = state.duckdb_path.clone();
    let id_clone = id.clone();
    let status_clone = status.clone();
    let updated = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let db = duckdb::Connection::open(&duckdb_path)
            .map_err(|e| format!("DuckDB open: {}", e))?;
        db.execute(
            "UPDATE feedback SET status = ? WHERE id = ?",
            duckdb::params![status_clone, id_clone],
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| err(500, &format!("Task: {}", e)))?
    .map_err(|e| err(500, &e))?;

    if updated == 0 {
        return Err(err(404, &format!("feedback {} not found", id)));
    }

    Ok(Json(json!({ "id": id, "status": status })))
}
