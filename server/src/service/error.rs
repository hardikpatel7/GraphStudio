//! Service-layer error type. Distinguishes the small set of HTTP-mappable
//! states (not-found, bad-request) from internal errors. Callers in
//! `handlers/` translate each variant into a `(StatusCode, Json)`; the
//! agent's tool layer collapses everything into a JSON `{ "error": ... }`
//! payload.
//!
//! Hand-rolled (no thiserror dep) — tiny enough that the manual `Display` /
//! `Error` impls are easier to read than the macro expansion.

use std::fmt;

#[derive(Debug)]
pub enum ServiceError {
    NotFound(String),
    BadRequest(String),
    Internal(anyhow::Error),
}

impl ServiceError {
    pub fn not_found(msg: impl Into<String>) -> Self { Self::NotFound(msg.into()) }
    pub fn bad_request(msg: impl Into<String>) -> Self { Self::BadRequest(msg.into()) }
    pub fn internal(err: impl Into<anyhow::Error>) -> Self { Self::Internal(err.into()) }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(m)   => write!(f, "{m}"),
            Self::BadRequest(m) => write!(f, "{m}"),
            Self::Internal(e)   => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for ServiceError {
    fn from(e: anyhow::Error) -> Self { Self::Internal(e) }
}

impl From<rusqlite::Error> for ServiceError {
    fn from(e: rusqlite::Error) -> Self { Self::Internal(anyhow::Error::new(e)) }
}

pub type Result<T> = std::result::Result<T, ServiceError>;

/// Convenience for handlers: map a `ServiceError` to the existing
/// `(StatusCode, Json)` shape the rest of `handlers::*` uses.
pub fn into_http(e: ServiceError) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    use axum::http::StatusCode;
    let (code, msg) = match &e {
        ServiceError::NotFound(m)   => (StatusCode::NOT_FOUND, m.clone()),
        ServiceError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
        ServiceError::Internal(e)   => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    (code, axum::Json(serde_json::json!({ "error": msg })))
}
