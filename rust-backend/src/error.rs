use std::fmt;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

/// Machine-readable codes for HTTP 401 responses. Callers should branch on `code`, not `message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AuthErrorCode {
    #[serde(rename = "AUTH_INVALID_CREDENTIALS")]
    InvalidCredentials,
    #[serde(rename = "AUTH_SESSION_REQUIRED")]
    SessionRequired,
    #[serde(rename = "AUTH_CRON_SECRET_MISMATCH")]
    CronSecretMismatch,
}

impl AuthErrorCode {
    pub const fn default_message(self) -> &'static str {
        match self {
            Self::InvalidCredentials => "Invalid credentials",
            Self::SessionRequired => "Valid session required",
            Self::CronSecretMismatch => "Invalid or missing cron authorization",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UnauthorizedError {
    pub code: AuthErrorCode,
    pub message: String,
}

impl UnauthorizedError {
    pub fn new(code: AuthErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn from_code(code: AuthErrorCode) -> Self {
        Self::new(code, code.default_message())
    }
}

impl fmt::Display for UnauthorizedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[derive(Debug, Serialize)]
struct UnauthorizedBody {
    error: UnauthorizedError,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(UnauthorizedError),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    NotImplemented(String),
    #[error("Internal server error")]
    Internal,
}

#[derive(Serialize)]
struct LegacyErrorBody {
    error: String,
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    /// Prefer [`Self::unauthorized_code`] unless you need a custom `message` for the same `code`.
    #[allow(dead_code)]
    pub fn unauthorized(err: UnauthorizedError) -> Self {
        Self::Unauthorized(err)
    }

    pub fn unauthorized_code(code: AuthErrorCode) -> Self {
        Self::Unauthorized(UnauthorizedError::from_code(code))
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::NotImplemented(message.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::Unauthorized(err) => (
                StatusCode::UNAUTHORIZED,
                Json(UnauthorizedBody { error: err }),
            )
                .into_response(),
            Self::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                Json(LegacyErrorBody { error: message }),
            )
                .into_response(),
            Self::NotFound(message) => (
                StatusCode::NOT_FOUND,
                Json(LegacyErrorBody { error: message }),
            )
                .into_response(),
            Self::Conflict(message) => (
                StatusCode::CONFLICT,
                Json(LegacyErrorBody { error: message }),
            )
                .into_response(),
            Self::NotImplemented(message) => (
                StatusCode::NOT_IMPLEMENTED,
                Json(LegacyErrorBody { error: message }),
            )
                .into_response(),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(LegacyErrorBody {
                    error: "Unexpected error".to_string(),
                }),
            )
                .into_response(),
        }
    }
}

impl From<tokio_postgres::Error> for AppError {
    fn from(_: tokio_postgres::Error) -> Self {
        Self::Internal
    }
}

impl From<deadpool_postgres::PoolError> for AppError {
    fn from(_: deadpool_postgres::PoolError) -> Self {
        Self::Internal
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(_: jsonwebtoken::errors::Error) -> Self {
        Self::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthErrorCode, UnauthorizedError};

    #[test]
    fn unauthorized_body_serializes_nested_error_with_code() {
        let body = serde_json::json!({
            "error": UnauthorizedError::from_code(AuthErrorCode::InvalidCredentials)
        });
        assert_eq!(body["error"]["code"], "AUTH_INVALID_CREDENTIALS");
        assert_eq!(body["error"]["message"], "Invalid credentials");
    }

    #[test]
    fn session_required_code_is_stable() {
        let err = UnauthorizedError::from_code(AuthErrorCode::SessionRequired);
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["code"], "AUTH_SESSION_REQUIRED");
        assert_eq!(v["message"], "Valid session required");
    }

    #[test]
    fn cron_secret_mismatch_code_is_stable() {
        let err = UnauthorizedError::from_code(AuthErrorCode::CronSecretMismatch);
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["code"], "AUTH_CRON_SECRET_MISMATCH");
    }
}
