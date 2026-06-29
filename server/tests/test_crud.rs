mod common;

// ── connections ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn connections_create_and_read() {
    let (server, _tmp) = common::setup_server().await;
    let body = serde_json::json!({
        "id": "conn-test-1",
        "display_name": "Test Connection",
        "type": "pg",
        "config": { "host": "localhost", "port": 5432, "user": "u", "password": "p", "database": "d" }
    });
    let create = server.post("/connections").json(&body).await;
    create.assert_status(axum::http::StatusCode::CREATED);

    let get = server.get("/connections/conn-test-1").await;
    get.assert_status_ok();
    let resp: serde_json::Value = get.json();
    assert_eq!(resp["id"], "conn-test-1");
    assert_eq!(resp["display_name"], "Test Connection");
}

#[tokio::test]
async fn connections_get_missing_returns_404() {
    let (server, _tmp) = common::setup_server().await;
    server.get("/connections/no-such-id").await.assert_status_not_found();
}

// ── sources ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sources_create_and_read() {
    let (server, _tmp) = common::setup_server().await;
    let body = serde_json::json!({
        "id": "src-test-1",
        "display_name": "Test Source",
        "kind": "duckdb_table",
        "config": { "table_name": "test_table" }
    });
    let create = server.post("/sources").json(&body).await;
    create.assert_status(axum::http::StatusCode::CREATED);

    let get = server.get("/sources/src-test-1").await;
    get.assert_status_ok();
    let resp: serde_json::Value = get.json();
    assert_eq!(resp["id"], "src-test-1");
    assert_eq!(resp["kind"], "duckdb_table");
}

#[tokio::test]
async fn sources_get_missing_returns_404() {
    let (server, _tmp) = common::setup_server().await;
    server.get("/sources/no-such-id").await.assert_status_not_found();
}

// ── dataviews ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dataviews_create_and_read() {
    let (server, _tmp) = common::setup_server().await;
    let body = serde_json::json!({
        "id": "dv-test-1",
        "display_name": "Test DataView"
    });
    let create = server.post("/dataviews").json(&body).await;
    create.assert_status(axum::http::StatusCode::CREATED);

    let get = server.get("/dataviews/dv-test-1").await;
    get.assert_status_ok();
    let resp: serde_json::Value = get.json();
    assert_eq!(resp["id"], "dv-test-1");
    assert_eq!(resp["display_name"], "Test DataView");
}

#[tokio::test]
async fn dataviews_get_missing_returns_404() {
    let (server, _tmp) = common::setup_server().await;
    server.get("/dataviews/no-such-id").await.assert_status_not_found();
}

// ── pipelines ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pipelines_create_and_read() {
    let (server, _tmp) = common::setup_server().await;
    let body = serde_json::json!({
        "id": "pl-test-1",
        "display_name": "Test Pipeline"
    });
    let create = server.post("/pipelines").json(&body).await;
    create.assert_status(axum::http::StatusCode::CREATED);

    let get = server.get("/pipelines/pl-test-1").await;
    get.assert_status_ok();
    let resp: serde_json::Value = get.json();
    assert_eq!(resp["id"], "pl-test-1");
    assert_eq!(resp["display_name"], "Test Pipeline");
}

#[tokio::test]
async fn pipelines_get_missing_returns_404() {
    let (server, _tmp) = common::setup_server().await;
    server.get("/pipelines/no-such-id").await.assert_status_not_found();
}

// ── modules ──────────────────────────────────────────────────────────────────
// Note: modules only expose GET /modules (list) — no GET /modules/{id}.
// The 404 test uses DELETE which does return 404 for missing IDs.

#[tokio::test]
async fn modules_create_and_read() {
    let (server, _tmp) = common::setup_server().await;
    let body = serde_json::json!({
        "id": "mod-test-1",
        "display_name": "Test Module",
        "route": "/test",
        "icon": "table",
        "permission_key": "test"
    });
    let create = server.post("/modules").json(&body).await;
    create.assert_status(axum::http::StatusCode::CREATED);

    let get = server.get("/modules").await;
    get.assert_status_ok();
    let list: Vec<serde_json::Value> = get.json();
    assert!(list.iter().any(|m| m["id"] == "mod-test-1"),
            "mod-test-1 not found in list");
}

#[tokio::test]
async fn modules_delete_missing_returns_404() {
    let (server, _tmp) = common::setup_server().await;
    server.delete("/modules/no-such-id").await.assert_status_not_found();
}

// ── dimensions ───────────────────────────────────────────────────────────────
// Note: dimensions only expose GET /dimensions (list) — no GET /dimensions/{id}.
// The 404 test uses DELETE which does return 404 for missing IDs.

#[tokio::test]
async fn dimensions_create_and_read() {
    let (server, _tmp) = common::setup_server().await;
    // dimensions need a connection reference; create one first
    server.post("/connections").json(&serde_json::json!({
        "id": "conn-for-dim",
        "display_name": "Dim Conn",
        "type": "pg",
        "config": { "host": "localhost", "port": 5432, "user": "u", "password": "p", "database": "d" }
    })).await;
    let body = serde_json::json!({
        "id": "dim-test-1",
        "display_name": "Test Dimension",
        "master_table": "product_attributes",
        "datasource_ref": "conn-for-dim"
    });
    let create = server.post("/dimensions").json(&body).await;
    create.assert_status(axum::http::StatusCode::CREATED);

    let get = server.get("/dimensions").await;
    get.assert_status_ok();
    let list: Vec<serde_json::Value> = get.json();
    assert!(list.iter().any(|d| d["id"] == "dim-test-1"),
            "dim-test-1 not found in list");
}

#[tokio::test]
async fn dimensions_delete_missing_returns_404() {
    let (server, _tmp) = common::setup_server().await;
    server.delete("/dimensions/no-such-id").await.assert_status_not_found();
}

// ── filter-configs ───────────────────────────────────────────────────────────

#[tokio::test]
async fn filter_configs_create_and_read() {
    let (server, _tmp) = common::setup_server().await;
    let body = serde_json::json!({
        "id": "fc-test-1",
        "display_name": "Test Filter Config",
        "dimension_ref": "dim-product"
    });
    let create = server.post("/filter-configs").json(&body).await;
    create.assert_status(axum::http::StatusCode::CREATED);

    let get = server.get("/filter-configs/fc-test-1").await;
    get.assert_status_ok();
    let resp: serde_json::Value = get.json();
    assert_eq!(resp["id"], "fc-test-1");
    assert_eq!(resp["dimension_ref"], "dim-product");
}

#[tokio::test]
async fn filter_configs_get_missing_returns_404() {
    let (server, _tmp) = common::setup_server().await;
    server.get("/filter-configs/no-such-id").await.assert_status_not_found();
}

// ── templates ────────────────────────────────────────────────────────────────
// Note: templates only expose GET /templates (list) — no GET /templates/{id}.

#[tokio::test]
async fn templates_create_and_list() {
    let (server, _tmp) = common::setup_server().await;
    let body = serde_json::json!({
        "id": "tmpl-test-1",
        "display_name": "Test Template",
        "description": "A test template"
    });
    let create = server.post("/templates").json(&body).await;
    create.assert_status(axum::http::StatusCode::CREATED);

    let get = server.get("/templates").await;
    get.assert_status_ok();
    let list: Vec<serde_json::Value> = get.json();
    assert!(list.iter().any(|t| t["id"] == "tmpl-test-1"),
            "tmpl-test-1 not found in list");
}
