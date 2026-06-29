mod common;

async fn create_minimal_dataview(server: &axum_test::TestServer) -> String {
    let dv_id = "dv-gen-test-1";
    let resp = server.post("/dataviews").json(&serde_json::json!({
        "id": dv_id,
        "display_name": "Gen Test View",
        "columns": [{"name": "id", "type": "VARCHAR", "visible": true}],
        "contract": {
            "grpc_service": "gen_test",
            "grpc_method": "list_gen_test"
        }
    })).await;
    // Accept both 200 and 201 — the test DB may already have this row from a prior run
    let status = resp.status_code();
    assert!(
        status == axum::http::StatusCode::OK || status == axum::http::StatusCode::CREATED,
        "create dataview failed with status {}", status
    );
    dv_id.to_string()
}

#[tokio::test]
async fn generate_preview_returns_six_expected_file_keys() {
    let (server, _tmp) = common::setup_server().await;
    let dv_id = create_minimal_dataview(&server).await;

    let resp = server
        .post(&format!("/generate/dataview/{dv_id}/preview"))
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    let files = body.get("files")
        .expect("response missing 'files' key")
        .as_object()
        .expect("'files' is not an object");

    // The six required keys — the .proto key includes a package-name prefix
    // dv_id "dv-gen-test-1" → snake_case "dv_gen_test_1" → proto/dv_gen_test_1.proto
    assert!(
        files.keys().any(|k| k.ends_with(".proto")),
        "no .proto key in files: {:?}", files.keys().collect::<Vec<_>>()
    );
    assert!(files.contains_key("Cargo.toml"),     "missing Cargo.toml");
    assert!(files.contains_key("build.rs"),        "missing build.rs");
    assert!(files.contains_key("src/main.rs"),     "missing src/main.rs");
    assert!(files.contains_key("src/service.rs"),  "missing src/service.rs");
    assert!(files.contains_key("src/rest.rs"),     "missing src/rest.rs");
    assert_eq!(files.len(), 6, "expected exactly 6 file keys, got {}: {:?}", files.len(), files.keys().collect::<Vec<_>>());
}

#[tokio::test]
async fn generate_write_creates_files_on_disk() {
    let (server, _tmp) = common::setup_server().await;
    let dv_id = create_minimal_dataview(&server).await;

    // Use a dedicated temp directory as output so we control its location
    let out_tmp = tempfile::tempdir().expect("tempdir for output");
    let out_dir = out_tmp.path().to_string_lossy().to_string();

    let resp = server
        .post(&format!("/generate/dataview/{dv_id}/write?output_dir={out_dir}"))
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    let files_written = body.get("files_written")
        .expect("missing 'files_written'")
        .as_array()
        .expect("'files_written' is not an array");

    assert!(!files_written.is_empty(), "files_written is empty");
    for path_val in files_written {
        let path = path_val.as_str().expect("path is not a string");
        assert!(
            std::path::Path::new(path).exists(),
            "generated file does not exist on disk: {path}"
        );
    }
}
