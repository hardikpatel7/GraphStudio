//! Service layer — extracted handler bodies, callable both from HTTP routes
//! (`server/src/handlers/*`) and the agent's tool registry
//! (`server/src/agent/tools.rs`). Service fns take `&AppState` + typed args
//! and return `anyhow::Result<T>`; the surrounding handler wraps the result
//! in `Json<Value>` and maps errors into `(StatusCode, Json<Value>)`.
//!
//! Naming intentionally singular `service::` to stay distinct from the
//! existing plural `services::` directory (which hosts gRPC services).

pub mod connections;
pub mod dataviews;
pub mod error;
pub mod filter_configs;
pub mod graphs;
pub mod query;
pub mod sources;

pub use error::{Result as ServiceResult, ServiceError};
