use axum::{extract::{Path, State, Query}, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;
use super::err;

/// POST /api/generate/dataview/:dv_id/preview
/// Returns the generated files as JSON without writing to disk.
pub async fn preview_dataview(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let dv = state.db.query_one(
        "SELECT * FROM dataviews WHERE id = ?1",
        &[&dv_id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "DataView not found"))?;

    let files = generate_grpc_service(&state, &dv)
        .map_err(|e| err(500, &format!("Generation failed: {}", e)))?;

    Ok(Json(json!({
        "dataview_id": dv_id,
        "files": files,
    })))
}

/// POST /api/generate/dataview/:dv_id/write
/// Generates files and writes them to disk under generated-services/.
pub async fn write_dataview(
    State(state): State<Arc<AppState>>,
    Path(dv_id): Path<String>,
    Query(params): Query<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let dv = state.db.query_one(
        "SELECT * FROM dataviews WHERE id = ?1",
        &[&dv_id as &dyn rusqlite::types::ToSql],
    ).map_err(|_| err(404, "DataView not found"))?;

    let files = generate_grpc_service(&state, &dv)
        .map_err(|e| err(500, &format!("Generation failed: {}", e)))?;

    // Determine output directory
    let output_base = params.get("output_dir")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let crate_name = to_crate_name(&dv_id);
            // Write relative to the smartstudio project root
            format!("generated-services/{}", crate_name)
        });

    // Resolve relative to exe base dir
    let exe_dir = std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let base_dir = exe_dir.join("../..");

    let output_dir = if std::path::Path::new(&output_base).is_absolute() {
        std::path::PathBuf::from(&output_base)
    } else {
        base_dir.join(&output_base)
    };

    // Write files
    let files_obj = files.as_object().ok_or_else(|| err(500, "Invalid files output"))?;
    let mut written = Vec::new();
    for (path, content) in files_obj {
        let full_path = output_dir.join(path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| err(500, &format!("mkdir failed: {}", e)))?;
        }
        std::fs::write(&full_path, content.as_str().unwrap_or(""))
            .map_err(|e| err(500, &format!("write failed: {}", e)))?;
        written.push(full_path.to_string_lossy().to_string());
    }

    Ok(Json(json!({
        "dataview_id": dv_id,
        "output_dir": output_dir.to_string_lossy(),
        "files_written": written,
    })))
}

// ── Template rendering ──

fn generate_grpc_service(_state: &AppState, dv: &Value) -> Result<Value, String> {
    let mut tera = tera::Tera::default();

    // Load templates from the templates directory
    let templates_dir = find_templates_dir()?;
    for entry in std::fs::read_dir(&templates_dir).map_err(|e| format!("read templates dir: {}", e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("tera") {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let content = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {}", name, e))?;
            tera.add_raw_template(&name, &content).map_err(|e| format!("parse {}: {}", name, e))?;
        }
    }

    let dv_id = dv["id"].as_str().unwrap_or("unknown");
    let display_name = dv["display_name"].as_str().unwrap_or(dv_id);

    // Parse columns from DataView
    let columns_json = dv.get("columns").cloned().unwrap_or(Value::Array(vec![]));
    let columns_arr = columns_json.as_array().ok_or("columns is not an array")?;

    // Build column metadata for templates
    let mut columns = Vec::new();
    for (i, col) in columns_arr.iter().enumerate() {
        let name = col["name"].as_str().unwrap_or("unknown").to_string();
        let col_type = col["type"].as_str().unwrap_or("string").to_lowercase();
        let (proto_type, rust_type, default_value) = match col_type.as_str() {
            "double" | "float" | "f64" | "number" => ("double".to_string(), "f64".to_string(), "0.0".to_string()),
            "int" | "integer" | "i64" | "int64" => ("int64".to_string(), "i64".to_string(), "0".to_string()),
            "bool" | "boolean" => ("bool".to_string(), "bool".to_string(), "false".to_string()),
            _ => ("string".to_string(), "String".to_string(), "String::new()".to_string()),
        };

        columns.push(json!({
            "name": name,
            "proto_type": proto_type,
            "rust_type": rust_type,
            "default_value": default_value,
            "field_number": i + 1,
            "index": i,
            "sortable": col.get("sortable").and_then(|v| v.as_bool()).unwrap_or(false),
            "searchable": col.get("searchable").and_then(|v| v.as_bool()).unwrap_or(false),
            "filterable": col.get("filterable").and_then(|v| v.as_bool()).unwrap_or(false),
        }));
    }

    // Extract filter/sort/search columns
    let filter_cols: Vec<String> = columns.iter()
        .filter(|c| c["filterable"].as_bool().unwrap_or(false))
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    let sort_cols: Vec<String> = columns.iter()
        .filter(|c| c["sortable"].as_bool().unwrap_or(false))
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    let search_cols: Vec<String> = columns.iter()
        .filter(|c| c["searchable"].as_bool().unwrap_or(false))
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();

    // Derive naming
    let package_name = to_snake_case(dv_id);
    let crate_name = to_crate_name(dv_id);
    let crate_name_underscored = crate_name.replace('-', "_");
    let service_name = to_pascal_case(dv_id) + "Service";
    let service_impl_name = service_name.clone() + "Impl";
    let server_module = to_snake_case(dv_id) + "_service_server";

    // Parquet path from backend_workflow
    let backend_workflow = dv.get("backend_workflow").cloned().unwrap_or(json!({}));
    let default_parquet_path = format!("config/{}", package_name);
    let parquet_path = backend_workflow.get("parquet")
        .and_then(|p| p.get("path"))
        .and_then(|p| p.as_str())
        .unwrap_or(&default_parquet_path);
    let partition_cols = backend_workflow.get("parquet")
        .and_then(|p| p.get("partition_by"))
        .and_then(|p| p.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    // Default sort
    let sort_json = dv.get("sort").cloned().unwrap_or(json!({}));
    let default_sort_col = sort_json.get("column")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| columns.first().and_then(|c| c["name"].as_str()).unwrap_or("id"));
    let default_sort_dir = sort_json.get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("ASC");

    // Build tera context
    let mut ctx = tera::Context::new();
    ctx.insert("dataview_id", dv_id);
    ctx.insert("display_name", display_name);
    ctx.insert("package_name", &package_name);
    ctx.insert("crate_name", &crate_name);
    ctx.insert("crate_name_underscored", &crate_name_underscored);
    ctx.insert("service_name", &service_name);
    ctx.insert("service_impl_name", &service_impl_name);
    ctx.insert("server_module", &server_module);
    ctx.insert("columns", &columns);
    ctx.insert("filter_cols", &filter_cols);
    ctx.insert("sort_cols", &sort_cols);
    ctx.insert("search_cols", &search_cols);
    ctx.insert("parquet_relative_path", parquet_path);
    ctx.insert("hive_partitioned", &(partition_cols > 0));
    ctx.insert("default_sort_col", default_sort_col);
    ctx.insert("default_sort_dir", default_sort_dir);

    // Render all templates
    let mut files = serde_json::Map::new();

    files.insert(
        format!("proto/{}.proto", package_name),
        Value::String(tera.render("proto.tera", &ctx).map_err(|e| format!("proto.tera: {}", e))?),
    );
    files.insert(
        "Cargo.toml".to_string(),
        Value::String(tera.render("cargo_toml.tera", &ctx).map_err(|e| format!("cargo_toml.tera: {}", e))?),
    );
    files.insert(
        "build.rs".to_string(),
        Value::String(tera.render("build_rs.tera", &ctx).map_err(|e| format!("build_rs.tera: {}", e))?),
    );
    files.insert(
        "src/main.rs".to_string(),
        Value::String(tera.render("main_rs.tera", &ctx).map_err(|e| format!("main_rs.tera: {}", e))?),
    );
    files.insert(
        "src/service.rs".to_string(),
        Value::String(tera.render("service_rs.tera", &ctx).map_err(|e| format!("service_rs.tera: {}", e))?),
    );
    files.insert(
        "src/rest.rs".to_string(),
        Value::String(tera.render("rest_rs.tera", &ctx).map_err(|e| format!("rest_rs.tera: {}", e))?),
    );

    Ok(Value::Object(files))
}

fn find_templates_dir() -> Result<String, String> {
    // Try multiple locations
    let candidates = [
        "server/templates/grpc-service",
        "templates/grpc-service",
        "../templates/grpc-service",
        "../../templates/grpc-service",
        "../../server/templates/grpc-service",
    ];

    let exe_dir = std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let base_dir = exe_dir.join("../..");

    for candidate in &candidates {
        let path = base_dir.join(candidate);
        if path.is_dir() {
            return Ok(path.to_string_lossy().to_string());
        }
    }

    // Also try from CWD
    for candidate in &candidates {
        let path = std::path::PathBuf::from(candidate);
        if path.is_dir() {
            return Ok(path.to_string_lossy().to_string());
        }
    }

    Err("Templates directory not found. Expected server/templates/grpc-service/".to_string())
}

// ── Cargo command execution ──

/// POST /api/generate/cargo
/// Runs a cargo command (check, build, run) in the specified working directory.
/// Body: { "action": "check"|"build"|"run", "working_dir": "/abs/path" }
pub async fn run_cargo(
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let action = body["action"].as_str()
        .ok_or_else(|| err(400, "Missing 'action' field (check|build|run)"))?;
    let working_dir = body["working_dir"].as_str()
        .ok_or_else(|| err(400, "Missing 'working_dir' field"))?;

    // Validate action
    let cargo_args: Vec<&str> = match action {
        "check" => vec!["check", "--message-format=short"],
        "build" => vec!["build"],
        "run" => vec!["run"],
        _ => return Err(err(400, "Invalid action. Must be check, build, or run")),
    };

    // Validate directory exists and has Cargo.toml
    let dir = std::path::Path::new(working_dir);
    if !dir.is_dir() {
        return Err(err(400, &format!("Directory not found: {}", working_dir)));
    }
    if !dir.join("Cargo.toml").exists() {
        return Err(err(400, &format!("No Cargo.toml in {}", working_dir)));
    }

    let start = std::time::Instant::now();

    // For "run", we start the process and return quickly with the PID.
    // For check/build, we wait for completion.
    if action == "run" {
        // Kill any previously running service on the same dir (best-effort)
        let _ = std::process::Command::new("pkill")
            .args(["-f", &format!("target/debug/.*{}", dir.file_name().unwrap_or_default().to_string_lossy().replace('-', "."))])
            .output();

        let child = std::process::Command::new("cargo")
            .args(&cargo_args)
            .current_dir(working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| err(500, &format!("Failed to spawn cargo run: {}", e)))?;

        let pid = child.id();

        // Give it a moment to start (or fail immediately)
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        // Check if process is still running
        let status = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output();
        let running = status.map(|s| s.status.success()).unwrap_or(false);

        return Ok(Json(json!({
            "action": "run",
            "pid": pid,
            "running": running,
            "working_dir": working_dir,
            "elapsed_ms": start.elapsed().as_millis(),
        })));
    }

    // check / build — run synchronously with timeout
    let output = tokio::task::spawn_blocking({
        let dir = working_dir.to_string();
        let args = cargo_args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        move || {
            std::process::Command::new("cargo")
                .args(&args)
                .current_dir(&dir)
                .output()
        }
    })
    .await
    .map_err(|e| err(500, &format!("Task join error: {}", e)))?
    .map_err(|e| err(500, &format!("Failed to run cargo: {}", e)))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();
    let exit_code = output.status.code().unwrap_or(-1);

    Ok(Json(json!({
        "action": action,
        "success": success,
        "exit_code": exit_code,
        "stdout": stdout,
        "stderr": stderr,
        "working_dir": working_dir,
        "elapsed_ms": start.elapsed().as_millis(),
    })))
}

/// POST /api/generate/cargo/stop
/// Kills a running cargo process by PID.
/// Body: { "pid": 12345 }
pub async fn stop_cargo(
    Json(body): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    let pid = body["pid"].as_u64()
        .ok_or_else(|| err(400, "Missing 'pid' field"))?;

    let output = std::process::Command::new("kill")
        .args([&pid.to_string()])
        .output()
        .map_err(|e| err(500, &format!("Failed to kill process: {}", e)))?;

    let success = output.status.success();

    Ok(Json(json!({
        "killed": success,
        "pid": pid,
    })))
}

// ── Naming helpers ──

fn to_snake_case(s: &str) -> String {
    s.replace('-', "_").to_lowercase()
}

fn to_crate_name(s: &str) -> String {
    s.replace('_', "-").to_lowercase() + "-service"
}

fn to_pascal_case(s: &str) -> String {
    s.split(|c: char| c == '_' || c == '-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect()
}
