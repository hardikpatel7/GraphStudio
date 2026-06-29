use axum::{extract::{Path, State}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::{err, stringify};
use std::time::Instant;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query("SELECT * FROM pipelines ORDER BY display_name", &[])
        .map(|rows| Json(Value::Array(rows)))
        .map_err(|e| err(500, &e.to_string()))
}

pub async fn get_one(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    state.db.query_one("SELECT * FROM pipelines WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map(Json)
        .map_err(|_| err(404, "Shared pipeline not found"))
}

pub async fn create(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let id = body["id"].as_str().unwrap_or("");
    let name = body["display_name"].as_str().unwrap_or("");
    let pipeline = body.get("pipeline").map(stringify).unwrap_or_else(|| "[]".into());
    let description = body.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();

    if id.is_empty() || name.is_empty() { return Err(err(400, "id and display_name required")); }

    state.db.execute(
        "INSERT INTO pipelines (id, display_name, pipeline, description) VALUES (?1, ?2, ?3, ?4)",
        &[&id as &dyn rusqlite::types::ToSql, &name as _, &pipeline as _, &description as _],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one("SELECT * FROM pipelines WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "shared_pipeline", "create", "success", &format!("Created shared pipeline '{}'", id), None, Some(elapsed));
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn update(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let mut sets = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

    if let Some(v) = body.get("display_name").and_then(|v| v.as_str()) { sets.push("display_name = ?"); vals.push(Box::new(v.to_string())); }
    if let Some(v) = body.get("description").and_then(|v| v.as_str()) { sets.push("description = ?"); vals.push(Box::new(v.to_string())); }
    if body.get("pipeline").is_some() { sets.push("pipeline = ?"); vals.push(Box::new(stringify(&body["pipeline"]))); }
    // Phase 2 of misty-hinton: trigger column. Validated below by parsing
    // through PipelineTrigger so a malformed payload returns 400 instead of
    // landing as garbage in SQLite.
    if let Some(v) = body.get("trigger") {
        match serde_json::from_value::<pipeline::PipelineTrigger>(v.clone()) {
            Ok(_) => {
                sets.push("trigger = ?");
                vals.push(Box::new(stringify(v)));
            }
            Err(e) => return Err(err(400, &format!("invalid trigger JSON: {}", e))),
        }
    }
    // Phase 3 of misty-hinton: placement column. Accepts a string
    // (`"duck_db_only"` / `"duck_db_and_in_memory"`).
    if let Some(v) = body.get("placement").and_then(|v| v.as_str()) {
        match v {
            "duck_db_only" | "duck_db_and_in_memory" => {
                sets.push("placement = ?");
                vals.push(Box::new(v.to_string()));
            }
            other => return Err(err(400, &format!("invalid placement '{}'", other))),
        }
    }

    if sets.is_empty() { return Err(err(400, "nothing to update")); }
    sets.push("updated_at = datetime('now')");

    let sql = format!("UPDATE pipelines SET {} WHERE id = ?", sets.join(", "));
    vals.push(Box::new(id.clone()));

    let params: Vec<&dyn rusqlite::types::ToSql> = vals.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();
    let n = state.db.execute(&sql, &params).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Shared pipeline not found")); }

    let row = state.db.query_one("SELECT * FROM pipelines WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql])
        .map_err(|e| err(500, &e.to_string()))?;
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "shared_pipeline", "update", "success", &format!("Updated shared pipeline '{}'", id), None, Some(elapsed));
    Ok(Json(row))
}

/// GET `/api/pipelines/{id}/export` — return the pipeline as a downloadable
/// JSON document. Strips transient fields (created_at, updated_at) so the
/// payload round-trips cleanly through `import` on another tenant or the
/// same tenant later.
pub async fn export(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, (axum::http::StatusCode, Json<Value>)> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let row = state
        .db
        .query_one(
            "SELECT * FROM pipelines WHERE id = ?1",
            &[&id as &dyn rusqlite::types::ToSql],
        )
        .map_err(|_| err(404, "Shared pipeline not found"))?;

    // Drop transient fields. Keep id so the user can see what they exported;
    // import lets the caller override it for "new" mode.
    let mut payload = row;
    if let Some(obj) = payload.as_object_mut() {
        obj.remove("created_at");
        obj.remove("updated_at");
    }

    let body = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    let filename = format!("{}.pipeline.json", id);
    let resp = (
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", filename),
            ),
        ],
        body,
    )
        .into_response();
    Ok(resp)
}

/// POST `/api/pipelines/import` — import a previously-exported pipeline.
///
/// Body: `{ "data": <export JSON>, "mode": "new" | "replace",
///          "target_id": "<optional>" }`
///
/// - `mode = "new"` (default): create. `target_id` (or a freshly minted ID
///   from `data.id` + a `_imported_<unix>` suffix) is required to be unused.
/// - `mode = "replace"`: replace. Uses `target_id` if provided, otherwise
///   `data.id`. Falls back to insert when the row doesn't exist (so the
///   same payload can both first-time-create and re-import-replace).
///
/// Honored fields from `data`: id, display_name, pipeline, trigger,
/// placement, execution. Anything else is ignored.
pub async fn import(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<(axum::http::StatusCode, Json<Value>), (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let data = body
        .get("data")
        .ok_or_else(|| err(400, "import body must include `data`"))?;
    let mode = body
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("new");
    let target_override = body
        .get("target_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    let source_id = data
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let display_name = data
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if display_name.is_empty() {
        return Err(err(400, "import data missing display_name"));
    }

    let target_id = match (mode, target_override.as_deref()) {
        ("new", Some(id)) => id.to_string(),
        ("new", None) => {
            // Suffix the original id with a timestamp so it doesn't clash.
            // If even that exists (rapid re-imports), bail and let the
            // caller pick a unique target_id.
            let base = if source_id.is_empty() { "imported".to_string() } else { source_id.clone() };
            format!(
                "{}_imported_{}",
                base,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            )
        }
        ("replace", Some(id)) => id.to_string(),
        ("replace", None) => {
            if source_id.is_empty() {
                return Err(err(
                    400,
                    "mode=replace requires either target_id or data.id",
                ));
            }
            source_id.clone()
        }
        (other, _) => return Err(err(400, &format!("invalid mode '{}'", other))),
    };

    let exists = state
        .db
        .query_one(
            "SELECT id FROM pipelines WHERE id = ?1",
            &[&target_id as &dyn rusqlite::types::ToSql],
        )
        .is_ok();

    if mode == "new" && exists {
        return Err(err(
            409,
            &format!("pipeline '{}' already exists; use mode=replace or pick a different target_id", target_id),
        ));
    }

    let pipeline_str = data.get("pipeline").map(stringify).unwrap_or_else(|| "[]".into());
    let trigger_str = data.get("trigger").map(stringify).unwrap_or_else(|| r#"{"kind":"manual"}"#.to_string());
    let placement = data.get("placement").and_then(|v| v.as_str()).unwrap_or("duck_db_only");
    let placement = match placement {
        "duck_db_only" | "duck_db_and_in_memory" => placement.to_string(),
        other => return Err(err(400, &format!("invalid placement '{}'", other))),
    };
    let execution = data.get("execution").and_then(|v| v.as_str()).unwrap_or("sequence");
    let execution = match execution {
        "sequence" | "parallel" => execution.to_string(),
        other => return Err(err(400, &format!("invalid execution '{}'", other))),
    };
    let description = data.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();

    // Validate trigger by parsing it through the pipeline crate. Same
    // guard as the update handler — malformed payloads land as 400.
    if let Err(e) = serde_json::from_str::<pipeline::PipelineTrigger>(&trigger_str) {
        return Err(err(400, &format!("invalid trigger JSON: {}", e)));
    }

    let action = if exists { "replace" } else { "insert" };
    let n = state.db.execute(
        "INSERT INTO pipelines (id, display_name, pipeline, trigger, placement, execution, description, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now')) \
         ON CONFLICT(id) DO UPDATE SET \
             display_name = excluded.display_name, \
             pipeline     = excluded.pipeline, \
             trigger      = excluded.trigger, \
             placement    = excluded.placement, \
             execution    = excluded.execution, \
             description  = excluded.description, \
             updated_at   = datetime('now')",
        &[
            &target_id as &dyn rusqlite::types::ToSql,
            &display_name as _,
            &pipeline_str as _,
            &trigger_str as _,
            &placement as _,
            &execution as _,
            &description as _,
        ],
    ).map_err(|e| err(500, &e.to_string()))?;

    let row = state.db.query_one(
        "SELECT * FROM pipelines WHERE id = ?1",
        &[&target_id as &dyn rusqlite::types::ToSql],
    ).map_err(|e| err(500, &e.to_string()))?;

    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(
        &state,
        &state.tenant_id,
        "shared_pipeline",
        "import",
        "success",
        &format!("Imported pipeline '{}' (mode={}, action={}, source_id={})", target_id, mode, action, source_id),
        None,
        Some(elapsed),
    );

    let _ = n;
    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn delete(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let t = Instant::now();
    let n = state.db.execute("DELETE FROM pipelines WHERE id = ?1", &[&id as &dyn rusqlite::types::ToSql]).map_err(|e| err(500, &e.to_string()))?;
    if n == 0 { return Err(err(404, "Shared pipeline not found")); }
    let elapsed = t.elapsed().as_millis() as i64;
    super::log_activity(&state, &state.tenant_id, "shared_pipeline", "delete", "success", &format!("Deleted shared pipeline '{}'", id), None, Some(elapsed));
    Ok(Json(json!({"deleted": true})))
}
