//! Minimal error types for MLGSM external module

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("docker error: {0}")]
    Docker(#[from] bollard::errors::Error),

    #[error("game error: {0}")]
    Game(String),

    #[error("conflict: {0}")]
    Conflict(String),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            Error::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Error::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg.clone()),
            Error::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            Error::Io(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            Error::Serde(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            Error::Docker(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            Error::Game(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            Error::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
        };
        (status, body).into_response()
    }
}

pub type MlgsmResult<T> = std::result::Result<T, Error>;

impl Error {

    pub fn not_found<T>(what: impl Into<String>) -> MlgsmResult<T> {
        Err(Error::NotFound(what.into()))
    }

    pub fn internal<T>(msg: impl Into<String>) -> MlgsmResult<T> {
        Err(Error::Internal(msg.into()))
    }

    pub fn service_unavailable<T>(msg: impl Into<String>) -> MlgsmResult<T> {
        Err(Error::ServiceUnavailable(msg.into()))
    }
}

