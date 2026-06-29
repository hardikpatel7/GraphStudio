mod common;

/// Canary: bundle export `Content-Disposition` header contains "smartstudio-bundle-".
/// Stays GREEN — bundle filename prefix is NOT renamed.
/// Seeds a DataView so the export endpoint returns 200 (it requires at least
/// one selected object).
#[tokio::test]
async fn bundle_export_content_disposition_contains_smartstudio_bundle() {
    let (server, _tmp) = common::setup_server().await;

    // Seed a DataView so the bundle export has something to export.
    let create = server
        .post("/dataviews")
        .json(&serde_json::json!({
            "id": "bundle-canary-dv",
            "display_name": "Bundle Canary"
        }))
        .await;
    create.assert_status(axum::http::StatusCode::CREATED);

    // POST to the live bundle export endpoint selecting the seeded DataView.
    let resp = server
        .post("/bundle/export")
        .json(&serde_json::json!({
            "kinds": {
                "dataviews": ["bundle-canary-dv"]
            }
        }))
        .await;

    resp.assert_status_ok();

    let cd = resp
        .headers()
        .get("content-disposition")
        .expect("missing Content-Disposition header")
        .to_str()
        .expect("non-ASCII Content-Disposition");
    assert!(
        cd.contains("smartstudio-bundle-"),
        "Content-Disposition does not contain 'smartstudio-bundle-': {cd}"
    );
}

/// Canary: resolved db_path ends with "smartstudio.db".
/// Turns RED when the SQLite filename is renamed.
#[tokio::test]
async fn sqlite_filename_is_smartstudio_db() {
    let tmp2 = tempfile::tempdir().unwrap();
    let home = tmp2.path().to_str().unwrap();
    let toml = format!(
        r#"home_path = "{home}"
client = "test"
app_type = "test"
environment = "test"
is_new = false
[server]
port = 13002
grpc_port = 50053
[rcl]
enabled = false
"#
    );
    let toml_path = tmp2.path().join("environment.toml");
    std::fs::write(&toml_path, toml).unwrap();
    let cfg = smartstudio_server::instance_config::load(&toml_path).unwrap();
    let resolved = smartstudio_server::instance_config::resolve(cfg).unwrap();
    assert!(
        resolved.db_path.ends_with("smartstudio.db"),
        "db_path '{}' does not end with 'smartstudio.db'",
        resolved.db_path
    );
}
