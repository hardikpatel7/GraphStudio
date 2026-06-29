use axum::Json;
use serde_json::{json, Value};

pub async fn list() -> Json<Value> {
    Json(json!([
        {
            "id": "rust-backend",
            "display_name": "Rust Backend",
            "description": "Rust gRPC service with metric framework, dimensions, and BQ export",
            "template_count": 8
        },
        {
            "id": "react-frontend",
            "display_name": "React Frontend",
            "description": "React app with TanStack Table/Router, Zustand stores, and Tailwind CSS",
            "template_count": 6
        }
    ]))
}
