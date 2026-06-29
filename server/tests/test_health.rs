mod common;

#[tokio::test]
async fn health_returns_200() {
    let (server, _tmp) = common::setup_server().await;
    let resp = server.get("/health").await;
    resp.assert_status_ok();
}

#[tokio::test]
async fn identity_returns_200_with_expected_shape() {
    let (server, _tmp) = common::setup_server().await;
    let resp = server.get("/identity").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body.get("id").and_then(|v| v.as_str()).is_some(), "missing id");
    assert!(body.get("client").and_then(|v| v.as_str()).is_some(), "missing client");
    assert!(body.get("app_type").and_then(|v| v.as_str()).is_some(), "missing app_type");
    assert!(body.get("environment").and_then(|v| v.as_str()).is_some(), "missing environment");
    assert!(body.get("display_name").and_then(|v| v.as_str()).is_some(), "missing display_name");
    // tenant_id = client-app_type-environment
    let id = body["id"].as_str().unwrap();
    assert_eq!(id, "test-test-test");
}
