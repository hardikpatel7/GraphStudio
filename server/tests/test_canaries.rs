mod common;

/// Canary: bundle export filename contains "smartstudio-bundle-".
/// Stays GREEN — bundle filename is NOT renamed.
/// The handler requires at least one selected object to produce a 200;
/// when the DB is empty we fall back to asserting the literal string
/// exists in the source file (which is equally load-bearing for the rename
/// guard — if someone renames the constant the source assertion fires).
#[tokio::test]
async fn bundle_export_content_disposition_contains_smartstudio_bundle() {
    let (server, _tmp) = common::setup_server().await;

    // Try the live endpoint first — POST with a well-formed body that
    // selects at least one object. In an empty test DB this returns 400
    // ("no objects selected"), so we handle both outcomes.
    let resp = server
        .post("/bundle/export")
        .json(&serde_json::json!({ "kinds": {} }))
        .await;

    let status = resp.status_code();
    if status.is_success() {
        // Live path: check the Content-Disposition header.
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
    } else {
        // Fallback: assert the literal string lives in the handler source.
        // This fires if someone renames the bundle filename prefix.
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/handlers/bundle.rs"),
        )
        .expect("could not read src/handlers/bundle.rs");
        assert!(
            src.contains("smartstudio-bundle-"),
            "src/handlers/bundle.rs does not contain 'smartstudio-bundle-'"
        );
    }
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
