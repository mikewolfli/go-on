//! Error types
//!
//! This module defines the error types used throughout the application.
//!
//! # Error Hierarchy
//!
//! The error system follows a hierarchical structure:
//! 1. **ProxyError**: Application-level errors
//! 2. **ValidationError**: Configuration and input validation errors
//! 3. **NetworkError**: Network and communication errors
//! 4. **ResourceError**: Resource and system errors
//!
//! Each error type provides detailed context and supports error chaining.
//!
//! # Usage status
//!
//! `AppError::Proxy` (via `ProxyError`) is the actively constructed production
//! path. The `Validation` / `Network` / `Resource` variants are retained as a
//! public library API for out-of-tree consumers and are exercised by unit
//! tests and benches; they are not constructed by production code paths today.

use thiserror::Error;

/// Main application error type
#[derive(Debug, Error)]
pub enum AppError {
    /// Proxy-related errors
    #[error("proxy error: {0}")]
    Proxy(#[from] ProxyError),

    /// Configuration validation errors
    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),

    /// Network and communication errors
    #[error("network error: {0}")]
    Network(#[from] NetworkError),

    /// Resource and system errors
    #[error("resource error: {0}")]
    Resource(#[from] ResourceError),

    /// External library errors
    #[error("external error: {0}")]
    External(#[from] anyhow::Error),
}

/// Proxy error types
#[derive(Debug, Error)]
pub enum ProxyError {
    /// Invalid request error
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Unknown method error
    #[error("unknown method: {0}")]
    UnknownMethod(String),

    /// Phase not found error
    #[error("phase not found: {0}")]
    PhaseNotFound(String),

    /// Agent not found error
    #[error("agent not found: {0}")]
    AgentNotFound(String),

    /// Internal server error
    #[error("internal server error: {0}")]
    Internal(String),

    /// Rate limit exceeded error
    #[error("rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    /// Circuit breaker open error
    #[error("circuit breaker open: {0}")]
    CircuitBreakerOpen(String),

    /// Timeout error
    #[error("timeout: {0}")]
    Timeout(String),
}

/// Configuration and input validation errors
/// Configuration validation errors
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Invalid configuration
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// Missing required field
    #[error("missing required field: {0}")]
    MissingField(String),

    /// Invalid input format
    #[error("invalid input format: {0}")]
    InvalidFormat(String),

    /// Value out of range
    #[error("value out of range: {0}")]
    OutOfRange(String),
}

/// Network and communication errors
#[derive(Debug, Error)]
pub enum NetworkError {
    /// Connection failed
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    /// Request timeout
    #[error("request timeout: {0}")]
    RequestTimeout(String),

    /// HTTP error
    #[error("HTTP error {0}: {1}")]
    Http(u16, String),

    /// SSL/TLS error
    #[error("SSL/TLS error: {0}")]
    Ssl(String),
}

/// Resource and system errors
#[derive(Debug, Error)]
pub enum ResourceError {
    /// File system error
    #[error("file system error: {0}")]
    FileSystem(String),

    /// Memory allocation error
    #[error("memory allocation error: {0}")]
    Memory(String),

    /// Database error
    #[error("database error: {0}")]
    Database(String),

    /// System resource exhausted
    #[error("system resource exhausted: {0}")]
    ResourceExhausted(String),
}

/// Standardized error code for API responses.
/// Maps to both HTTP status codes and machine-readable error identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ErrorCode {
    // ── 4xx Client Errors ───────────────────────────────────────
    BadRequest,
    InvalidRequest,
    ValidationError,
    MissingField,
    InvalidFormat,
    AuthenticationRequired,
    Unauthorized,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    RateLimitExceeded,
    RequestTimeout,
    Conflict,
    // ── 5xx Server Errors ───────────────────────────────────────
    InternalError,
    ServiceUnavailable,
    BadGateway,
    CircuitBreakerOpen,
    DatabaseError,
    ExternalServiceError,
    ResourceExhausted,
}

impl ErrorCode {
    /// Returns the HTTP status code for this error code.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::InvalidRequest => 400,
            Self::ValidationError => 400,
            Self::MissingField => 400,
            Self::InvalidFormat => 400,
            Self::AuthenticationRequired => 401,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::MethodNotAllowed => 405,
            Self::RateLimitExceeded => 429,
            Self::RequestTimeout => 408,
            Self::Conflict => 409,
            Self::InternalError => 500,
            Self::ServiceUnavailable => 503,
            Self::BadGateway => 502,
            Self::CircuitBreakerOpen => 503,
            Self::DatabaseError => 500,
            Self::ExternalServiceError => 502,
            Self::ResourceExhausted => 503,
        }
    }
}

/// Maps an HTTP status code to the appropriate canonical ErrorCode.
/// Returns `None` for non-error status codes (< 400).
pub(crate) fn error_code_from_status(status: u16) -> Option<ErrorCode> {
    match status {
        400 => Some(ErrorCode::BadRequest),
        401 => Some(ErrorCode::Unauthorized),
        403 => Some(ErrorCode::Forbidden),
        404 => Some(ErrorCode::NotFound),
        405 => Some(ErrorCode::MethodNotAllowed),
        408 => Some(ErrorCode::RequestTimeout),
        409 => Some(ErrorCode::Conflict),
        429 => Some(ErrorCode::RateLimitExceeded),
        500 => Some(ErrorCode::InternalError),
        502 => Some(ErrorCode::BadGateway),
        503 => Some(ErrorCode::ServiceUnavailable),
        _ if status >= 500 => Some(ErrorCode::InternalError),
        _ if status >= 400 => Some(ErrorCode::BadRequest),
        _ => None,
    }
}

impl AppError {
    /// Returns the canonical ErrorCode for this error.
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::Proxy(e) => match e {
                ProxyError::InvalidRequest(_) => ErrorCode::InvalidRequest,
                ProxyError::UnknownMethod(_) => ErrorCode::NotFound,
                ProxyError::PhaseNotFound(_) => ErrorCode::NotFound,
                ProxyError::AgentNotFound(_) => ErrorCode::NotFound,
                ProxyError::Internal(_) => ErrorCode::InternalError,
                ProxyError::RateLimitExceeded(_) => ErrorCode::RateLimitExceeded,
                ProxyError::CircuitBreakerOpen(_) => ErrorCode::CircuitBreakerOpen,
                ProxyError::Timeout(_) => ErrorCode::RequestTimeout,
            },
            Self::Validation(e) => match e {
                ValidationError::InvalidConfig(_) => ErrorCode::ValidationError,
                ValidationError::MissingField(_) => ErrorCode::MissingField,
                ValidationError::InvalidFormat(_) => ErrorCode::InvalidFormat,
                ValidationError::OutOfRange(_) => ErrorCode::BadRequest,
            },
            Self::Network(e) => match e {
                NetworkError::ConnectionFailed(_) => ErrorCode::ExternalServiceError,
                NetworkError::RequestTimeout(_) => ErrorCode::RequestTimeout,
                NetworkError::Http(code, _) => {
                    if *code >= 500 {
                        ErrorCode::BadGateway
                    } else {
                        ErrorCode::BadRequest
                    }
                }
                NetworkError::Ssl(_) => ErrorCode::ExternalServiceError,
            },
            Self::Resource(e) => match e {
                ResourceError::FileSystem(_) => ErrorCode::InternalError,
                ResourceError::Memory(_) => ErrorCode::ResourceExhausted,
                ResourceError::Database(_) => ErrorCode::DatabaseError,
                ResourceError::ResourceExhausted(_) => ErrorCode::ResourceExhausted,
            },
            Self::External(_) => ErrorCode::InternalError,
        }
    }
}

/// Convenience type alias for Result<T, AppError>
pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_error_display() {
        let error = ProxyError::InvalidRequest("missing required field".to_string());
        assert_eq!(error.to_string(), "invalid request: missing required field");

        let error = ProxyError::RateLimitExceeded("api calls".to_string());
        assert_eq!(error.to_string(), "rate limit exceeded: api calls");

        let error = ProxyError::CircuitBreakerOpen("openai".to_string());
        assert_eq!(error.to_string(), "circuit breaker open: openai");
    }

    #[test]
    fn test_app_error_from_proxy_error() {
        let proxy_error = ProxyError::Internal("something went wrong".to_string());
        let app_error: AppError = proxy_error.into();

        match app_error {
            AppError::Proxy(ProxyError::Internal(msg)) => {
                assert_eq!(msg, "something went wrong");
            }
            _ => panic!("expected ProxyError::Internal"),
        }
    }

    #[test]
    fn test_result_type_alias() {
        fn returns_result() -> Result<String> {
            Ok("success".to_string())
        }

        fn returns_error() -> Result<String> {
            Err(AppError::Proxy(ProxyError::Internal("error".to_string())))
        }

        assert_eq!(
            returns_result().expect("returns_result should succeed"),
            "success"
        );
        assert!(returns_error().is_err());
    }

    #[test]
    fn test_validation_error() {
        let error = ValidationError::InvalidConfig("missing api key".to_string());
        assert_eq!(error.to_string(), "invalid configuration: missing api key");

        let error = ValidationError::MissingField("name".to_string());
        assert_eq!(error.to_string(), "missing required field: name");
    }

    #[test]
    fn test_network_error() {
        let error = NetworkError::Http(404, "not found".to_string());
        assert_eq!(error.to_string(), "HTTP error 404: not found");

        let error = NetworkError::ConnectionFailed("connection refused".to_string());
        assert_eq!(error.to_string(), "connection failed: connection refused");
    }

    #[test]
    fn test_resource_error() {
        let error = ResourceError::FileSystem("permission denied".to_string());
        assert_eq!(error.to_string(), "file system error: permission denied");

        let error = ResourceError::ResourceExhausted("memory".to_string());
        assert_eq!(error.to_string(), "system resource exhausted: memory");
    }

    #[test]
    fn test_error_code_http_status_mapping() {
        // Every ErrorCode variant must map to a valid HTTP status code
        assert_eq!(ErrorCode::BadRequest.http_status(), 400);
        assert_eq!(ErrorCode::InvalidRequest.http_status(), 400);
        assert_eq!(ErrorCode::ValidationError.http_status(), 400);
        assert_eq!(ErrorCode::MissingField.http_status(), 400);
        assert_eq!(ErrorCode::InvalidFormat.http_status(), 400);
        assert_eq!(ErrorCode::AuthenticationRequired.http_status(), 401);
        assert_eq!(ErrorCode::Unauthorized.http_status(), 401);
        assert_eq!(ErrorCode::Forbidden.http_status(), 403);
        assert_eq!(ErrorCode::NotFound.http_status(), 404);
        assert_eq!(ErrorCode::MethodNotAllowed.http_status(), 405);
        assert_eq!(ErrorCode::RateLimitExceeded.http_status(), 429);
        assert_eq!(ErrorCode::RequestTimeout.http_status(), 408);
        assert_eq!(ErrorCode::Conflict.http_status(), 409);
        assert_eq!(ErrorCode::InternalError.http_status(), 500);
        assert_eq!(ErrorCode::ServiceUnavailable.http_status(), 503);
        assert_eq!(ErrorCode::BadGateway.http_status(), 502);
        assert_eq!(ErrorCode::CircuitBreakerOpen.http_status(), 503);
        assert_eq!(ErrorCode::DatabaseError.http_status(), 500);
        assert_eq!(ErrorCode::ExternalServiceError.http_status(), 502);
        assert_eq!(ErrorCode::ResourceExhausted.http_status(), 503);
    }

    #[test]
    fn test_error_code_serde_roundtrip() {
        // Serialize each variant and verify deserialization round-trips
        let variants = [
            ErrorCode::BadRequest,
            ErrorCode::InvalidRequest,
            ErrorCode::ValidationError,
            ErrorCode::MissingField,
            ErrorCode::InvalidFormat,
            ErrorCode::AuthenticationRequired,
            ErrorCode::Unauthorized,
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::MethodNotAllowed,
            ErrorCode::RateLimitExceeded,
            ErrorCode::RequestTimeout,
            ErrorCode::Conflict,
            ErrorCode::InternalError,
            ErrorCode::ServiceUnavailable,
            ErrorCode::BadGateway,
            ErrorCode::CircuitBreakerOpen,
            ErrorCode::DatabaseError,
            ErrorCode::ExternalServiceError,
            ErrorCode::ResourceExhausted,
        ];
        for variant in &variants {
            let json = serde_json::to_value(variant).expect("serialization should succeed");
            let deserialized: ErrorCode =
                serde_json::from_value(json).expect("deserialization should succeed");
            assert_eq!(*variant, deserialized);
        }
    }

    #[test]
    fn test_error_code_serde_screaming_snake_case() {
        // Verify the SCREAMING_SNAKE_CASE rename works correctly
        assert_eq!(
            serde_json::to_value(ErrorCode::BadRequest).unwrap(),
            serde_json::json!("BAD_REQUEST")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::InternalError).unwrap(),
            serde_json::json!("INTERNAL_ERROR")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::ExternalServiceError).unwrap(),
            serde_json::json!("EXTERNAL_SERVICE_ERROR")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::AuthenticationRequired).unwrap(),
            serde_json::json!("AUTHENTICATION_REQUIRED")
        );
    }

    #[test]
    fn test_error_code_from_status() {
        // Known status codes map to the expected ErrorCode
        assert_eq!(error_code_from_status(400), Some(ErrorCode::BadRequest));
        assert_eq!(error_code_from_status(401), Some(ErrorCode::Unauthorized));
        assert_eq!(error_code_from_status(403), Some(ErrorCode::Forbidden));
        assert_eq!(error_code_from_status(404), Some(ErrorCode::NotFound));
        assert_eq!(
            error_code_from_status(405),
            Some(ErrorCode::MethodNotAllowed)
        );
        assert_eq!(error_code_from_status(408), Some(ErrorCode::RequestTimeout));
        assert_eq!(error_code_from_status(409), Some(ErrorCode::Conflict));
        assert_eq!(
            error_code_from_status(429),
            Some(ErrorCode::RateLimitExceeded)
        );
        assert_eq!(error_code_from_status(500), Some(ErrorCode::InternalError));
        assert_eq!(error_code_from_status(502), Some(ErrorCode::BadGateway));
        assert_eq!(
            error_code_from_status(503),
            Some(ErrorCode::ServiceUnavailable)
        );

        // Unknown 4xx → BadRequest, unknown 5xx → InternalError
        assert_eq!(error_code_from_status(402), Some(ErrorCode::BadRequest));
        assert_eq!(error_code_from_status(406), Some(ErrorCode::BadRequest));
        assert_eq!(error_code_from_status(501), Some(ErrorCode::InternalError));
        assert_eq!(error_code_from_status(504), Some(ErrorCode::InternalError));

        // Non-error status codes → None
        assert_eq!(error_code_from_status(200), None);
        assert_eq!(error_code_from_status(302), None);
        assert_eq!(error_code_from_status(100), None);
    }

    #[test]
    fn test_app_error_error_code_mapping() {
        // Proxy errors
        let e = AppError::Proxy(ProxyError::InvalidRequest("bad".into()));
        assert_eq!(e.error_code(), ErrorCode::InvalidRequest);

        let e = AppError::Proxy(ProxyError::UnknownMethod("unknown".into()));
        assert_eq!(e.error_code(), ErrorCode::NotFound);

        let e = AppError::Proxy(ProxyError::PhaseNotFound("p".into()));
        assert_eq!(e.error_code(), ErrorCode::NotFound);

        let e = AppError::Proxy(ProxyError::AgentNotFound("a".into()));
        assert_eq!(e.error_code(), ErrorCode::NotFound);

        let e = AppError::Proxy(ProxyError::Internal("err".into()));
        assert_eq!(e.error_code(), ErrorCode::InternalError);

        let e = AppError::Proxy(ProxyError::RateLimitExceeded("too fast".into()));
        assert_eq!(e.error_code(), ErrorCode::RateLimitExceeded);

        let e = AppError::Proxy(ProxyError::CircuitBreakerOpen("open".into()));
        assert_eq!(e.error_code(), ErrorCode::CircuitBreakerOpen);

        let e = AppError::Proxy(ProxyError::Timeout("t".into()));
        assert_eq!(e.error_code(), ErrorCode::RequestTimeout);

        // Validation errors
        let e = AppError::Validation(ValidationError::InvalidConfig("c".into()));
        assert_eq!(e.error_code(), ErrorCode::ValidationError);

        let e = AppError::Validation(ValidationError::MissingField("f".into()));
        assert_eq!(e.error_code(), ErrorCode::MissingField);

        let e = AppError::Validation(ValidationError::InvalidFormat("f".into()));
        assert_eq!(e.error_code(), ErrorCode::InvalidFormat);

        let e = AppError::Validation(ValidationError::OutOfRange("o".into()));
        assert_eq!(e.error_code(), ErrorCode::BadRequest);

        // Network errors
        let e = AppError::Network(NetworkError::ConnectionFailed("refused".into()));
        assert_eq!(e.error_code(), ErrorCode::ExternalServiceError);

        let e = AppError::Network(NetworkError::RequestTimeout("slow".into()));
        assert_eq!(e.error_code(), ErrorCode::RequestTimeout);

        let e = AppError::Network(NetworkError::Http(502, "bad".into()));
        assert_eq!(e.error_code(), ErrorCode::BadGateway);

        let e = AppError::Network(NetworkError::Http(400, "bad".into()));
        assert_eq!(e.error_code(), ErrorCode::BadRequest);

        let e = AppError::Network(NetworkError::Ssl("cert".into()));
        assert_eq!(e.error_code(), ErrorCode::ExternalServiceError);

        // Resource errors
        let e = AppError::Resource(ResourceError::FileSystem("perm".into()));
        assert_eq!(e.error_code(), ErrorCode::InternalError);

        let e = AppError::Resource(ResourceError::Memory("oom".into()));
        assert_eq!(e.error_code(), ErrorCode::ResourceExhausted);

        let e = AppError::Resource(ResourceError::Database("conn".into()));
        assert_eq!(e.error_code(), ErrorCode::DatabaseError);

        let e = AppError::Resource(ResourceError::ResourceExhausted("fd".into()));
        assert_eq!(e.error_code(), ErrorCode::ResourceExhausted);

        // External error
        let e = AppError::External(anyhow::anyhow!("something"));
        assert_eq!(e.error_code(), ErrorCode::InternalError);
    }
}
