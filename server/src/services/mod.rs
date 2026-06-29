//! In-process gRPC services exposed alongside the Axum HTTP server.
//!
//! Currently:
//! - [`rcl_grpc`] — wraps the `rcl` crate (`RuleStore` + pure resolvers)
//!   in a Tonic service.
//!
//! The Tonic server runs on `[server].grpc_port` (default 50051), separate
//! from the Axum HTTP port. The two share the same tokio runtime and
//! `Arc<AppState>` but otherwise don't interact.

pub mod graph_articles_grpc;
pub mod article_selection_grpc;
pub mod cross_filter_grpc;
pub mod pipeline_scheduler;
pub mod rcl_grpc;
