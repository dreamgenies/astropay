use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::fmt;

#[derive(Debug)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub status: StatusCode,
}

#[derive(Debug, Clone, Copy)]
pub enum ErrorCode {
    // Auth errors
    SessionRequired,
    InvalidCredentials,
    SessionExpired,
    RateLimited,
    
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
}

#[derive(Debug, Clone, Copy)]
pub enum AuthErrorCode {
    SessionRequired,
    InvalidCredentials,
    SessionExpired,
    RateLimited,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self {
            ErrorCode::SessionRequired => "SESSION_REQUIRED",
            ErrorCode::InvalidCredentials => "INVALID_CREDENTIALS", 
            ErrorCode::SessionExpired => "SESSION_EXPIRED",
            ErrorCode::RateLimited => "RATE_LIMITED",
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
        };
        write!(f, "{}", code)
    }
}

impl AppError {
    pub fn bad_request(message: &str) -> Self {
        Self {
            code: ErrorCode::InvalidPayload,
            message: message.to_string(),
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
            AuthErrorCode::SessionRequired => (ErrorCode::SessionRequired, "Authentication required"),
            AuthErrorCode::InvalidCredentials => (ErrorCode::InvalidCredentials, "Invalid email or password"),
            AuthErrorCode::SessionExpired => (ErrorCode::SessionExpired, "Session has expired"),
            AuthErrorCode::RateLimited => (ErrorCode::RateLimited, "Too many attempts, please try again later"),
        };
        Self {
            code,
            message: message.to_string(),
            status: StatusCode::UNAUTHORIZED,
        }
    }

    pub fn not_found(message: &str) -> Self {
        Self {
            code: ErrorCode::InvoiceNotFound,
            message: message.to_string(),
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

    pub fn rate_limited(message: &str) -> Self {
        Self {
            code: ErrorCode::RateLimited,
            message: message.to_string(),
            status: StatusCode::TOO_MANY_REQUESTS,
        }
    }

    pub const Internal: Self = Self {
        code: ErrorCode::Internal,
        message: String::new(),
        status: StatusCode::INTERNAL_SERVER_ERROR,
    };
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Capture non-user errors to Sentry. No-ops when Sentry is not configured.
        if matches!(self.classify(), ErrorClass::System | ErrorClass::Upstream) {
            sentry::capture_error(&self);
        }

        match self {
            Self::RateLimited {
                retry_after_seconds,
            } => {
                let body = RateLimitedBody {
                    error: RateLimitedInner {
                        code: "AUTH_RATE_LIMITED",
                        message: "Too many login attempts. Please wait before trying again."
                            .to_string(),
                        retry_after_seconds,
                    },
                };
                let mut res = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
                if let Ok(h) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
                    res.headers_mut().insert(header::RETRY_AFTER, h);
                }
                res
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

    // --- Sentry capture gate ---
    // Verifies which error classes should be forwarded to Sentry.
    // The actual sentry::capture_error call is a no-op when no DSN is configured,
    // so these tests assert the classification logic that gates the call.

    #[test]
    fn sentry_captures_system_errors() {
        use super::{AppError, ErrorClass};
        let errors = [AppError::Internal, AppError::not_implemented("x")];
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
        use super::{AppError, ErrorClass};
        assert_eq!(AppError::HorizonUnavailable.classify(), ErrorClass::Upstream);
    }

    #[test]
    fn sentry_does_not_capture_user_errors() {
        use super::{AppError, AuthErrorCode, ErrorClass};
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
}
}
