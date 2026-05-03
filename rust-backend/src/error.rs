use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::fmt;

#[derive(Debug)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub status: StatusCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    // Auth errors
    SessionRequired,
    InvalidCredentials,
    SessionExpired,
    RateLimited,
    CronSecretMismatch,

    // Validation errors
    InvalidPayload,
    InvalidUuid,

    // Business logic errors
    InvoiceNotFound,
    InvoiceExpired,
    InvoiceAlreadyPaid,
    DuplicateEmail,
    DuplicateKeys,

    // System errors
    DatabaseError,
    NetworkError,
    Internal,
    NotImplemented,
    HorizonUnavailable,
}

#[derive(Debug, Clone, Copy)]
pub enum AuthErrorCode {
    SessionRequired,
    InvalidCredentials,
    SessionExpired,
    RateLimited,
    CronSecretMismatch,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self {
            ErrorCode::SessionRequired => "SESSION_REQUIRED",
            ErrorCode::InvalidCredentials => "INVALID_CREDENTIALS",
            ErrorCode::SessionExpired => "SESSION_EXPIRED",
            ErrorCode::RateLimited => "RATE_LIMITED",
            ErrorCode::CronSecretMismatch => "CRON_SECRET_MISMATCH",
            ErrorCode::InvalidPayload => "INVALID_PAYLOAD",
            ErrorCode::InvalidUuid => "INVALID_UUID",
            ErrorCode::InvoiceNotFound => "INVOICE_NOT_FOUND",
            ErrorCode::InvoiceExpired => "INVOICE_EXPIRED",
            ErrorCode::InvoiceAlreadyPaid => "INVOICE_ALREADY_PAID",
            ErrorCode::DuplicateEmail => "DUPLICATE_EMAIL",
            ErrorCode::DuplicateKeys => "DUPLICATE_KEYS",
            ErrorCode::DatabaseError => "DATABASE_ERROR",
            ErrorCode::NetworkError => "NETWORK_ERROR",
            ErrorCode::Internal => "INTERNAL_ERROR",
            ErrorCode::NotImplemented => "NOT_IMPLEMENTED",
            ErrorCode::HorizonUnavailable => "HORIZON_UNAVAILABLE",
        };
        write!(f, "{}", code)
    }
}

/// Broad classification used to decide whether to forward an error to Sentry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Client / auth errors — not forwarded to Sentry.
    User,
    /// Internal / unexpected errors — forwarded to Sentry.
    System,
    /// Upstream dependency failures (e.g. Horizon) — forwarded to Sentry.
    Upstream,
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidPayload,
            message: message.into(),
            status: StatusCode::BAD_REQUEST,
        }
    }

    pub fn unauthorized(message: &str) -> Self {
        Self {
            code: ErrorCode::SessionRequired,
            message: message.to_string(),
            status: StatusCode::UNAUTHORIZED,
        }
    }

    pub fn unauthorized_code(auth_code: AuthErrorCode) -> Self {
        let (code, message) = match auth_code {
            AuthErrorCode::SessionRequired => {
                (ErrorCode::SessionRequired, "Authentication required")
            }
            AuthErrorCode::InvalidCredentials => {
                (ErrorCode::InvalidCredentials, "Invalid email or password")
            }
            AuthErrorCode::SessionExpired => (ErrorCode::SessionExpired, "Session has expired"),
            AuthErrorCode::RateLimited => (
                ErrorCode::RateLimited,
                "Too many attempts, please try again later",
            ),
            AuthErrorCode::CronSecretMismatch => (ErrorCode::CronSecretMismatch, "Unauthorized"),
        };
        Self {
            code,
            message: message.to_string(),
            status: StatusCode::UNAUTHORIZED,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvoiceNotFound,
            message: message.into(),
            status: StatusCode::NOT_FOUND,
        }
    }

    pub fn conflict(message: &str) -> Self {
        Self {
            code: ErrorCode::DuplicateEmail,
            message: message.to_string(),
            status: StatusCode::CONFLICT,
        }
    }

    pub fn not_implemented(message: &str) -> Self {
        Self {
            code: ErrorCode::NotImplemented,
            message: message.to_string(),
            status: StatusCode::NOT_IMPLEMENTED,
        }
    }

    pub fn rate_limited(retry_after_seconds: u64) -> Self {
        Self {
            code: ErrorCode::RateLimited,
            message: format!(
                "Too many attempts. Please wait {} seconds before trying again.",
                retry_after_seconds
            ),
            status: StatusCode::TOO_MANY_REQUESTS,
        }
    }

    /// Horizon is temporarily unavailable — callers should skip and retry later.
    pub const HORIZON_UNAVAILABLE: Self = Self {
        code: ErrorCode::HorizonUnavailable,
        message: String::new(),
        status: StatusCode::BAD_GATEWAY,
    };

    pub const INTERNAL: Self = Self {
        code: ErrorCode::Internal,
        message: String::new(),
        status: StatusCode::INTERNAL_SERVER_ERROR,
    };

    /// Classify the error for Sentry gating.
    pub fn classify(&self) -> ErrorClass {
        match self.code {
            ErrorCode::Internal
            | ErrorCode::DatabaseError
            | ErrorCode::NetworkError
            | ErrorCode::NotImplemented => ErrorClass::System,
            ErrorCode::HorizonUnavailable => ErrorClass::Upstream,
            _ => ErrorClass::User,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Forward system and upstream errors to Sentry. No-ops when no DSN is configured.
        if matches!(self.classify(), ErrorClass::System | ErrorClass::Upstream) {
            sentry::capture_error(&self);
        }

        let body = Json(json!({
            "error": {
                "code": self.code.to_string(),
                "message": if self.message.is_empty() { "Internal server error" } else { &self.message }
            }
        }));
        (self.status, body).into_response()
    }
}

impl From<deadpool_postgres::PoolError> for AppError {
    fn from(_: deadpool_postgres::PoolError) -> Self {
        Self {
            code: ErrorCode::DatabaseError,
            message: "Database connection failed".to_string(),
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<tokio_postgres::Error> for AppError {
    fn from(err: tokio_postgres::Error) -> Self {
        tracing::error!("Database error: {}", err);
        Self {
            code: ErrorCode::DatabaseError,
            message: "Database operation failed".to_string(),
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<uuid::Error> for AppError {
    fn from(_: uuid::Error) -> Self {
        Self {
            code: ErrorCode::InvalidUuid,
            message: "Invalid UUID format".to_string(),
            status: StatusCode::BAD_REQUEST,
        }
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(_: jsonwebtoken::errors::Error) -> Self {
        Self {
            code: ErrorCode::SessionExpired,
            message: "Session token is invalid or expired".to_string(),
            status: StatusCode::UNAUTHORIZED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppError, AuthErrorCode, ErrorClass};

    // ── Sentry capture gate ──────────────────────────────────────────────────

    #[test]
    fn sentry_captures_system_errors() {
        let errors = [AppError::INTERNAL, AppError::not_implemented("x")];
        for e in &errors {
            assert_eq!(
                e.classify(),
                ErrorClass::System,
                "{e:?} should be System (captured by Sentry)"
            );
        }
    }

    #[test]
    fn sentry_captures_upstream_errors() {
        assert_eq!(
            AppError::HORIZON_UNAVAILABLE.classify(),
            ErrorClass::Upstream
        );
    }

    #[test]
    fn sentry_does_not_capture_user_errors() {
        let errors = [
            AppError::bad_request("bad"),
            AppError::unauthorized_code(AuthErrorCode::SessionRequired),
            AppError::rate_limited(60),
            AppError::not_found("x"),
            AppError::conflict("x"),
        ];
        for e in &errors {
            assert_eq!(
                e.classify(),
                ErrorClass::User,
                "{e:?} should be User (not captured by Sentry)"
            );
        }
    }

    #[test]
    fn rate_limited_has_correct_status() {
        let e = AppError::rate_limited(30);
        assert_eq!(e.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn internal_error_has_500_status() {
        assert_eq!(
            AppError::INTERNAL.status,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
