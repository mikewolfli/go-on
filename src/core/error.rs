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

use thiserror::Error;

/// Main application error type
#[derive(Debug, Error)]
#[allow(dead_code)]
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
    #[allow(dead_code)]
    RateLimitExceeded(String),

    /// Circuit breaker open error
    #[error("circuit breaker open: {0}")]
    #[allow(dead_code)]
    CircuitBreakerOpen(String),

    /// Timeout error
    #[error("timeout: {0}")]
    #[allow(dead_code)]
    Timeout(String),
}

/// Configuration and input validation errors
/// Configuration validation errors
#[derive(Debug, Error)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/// Convenience type alias for Result<T, AppError>
#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, AppError>;

/// Extension trait for error context
#[allow(dead_code)]
pub trait ErrorContext {
    /// Add context to an error
    fn context(self, context: &str) -> Self;
}

impl ErrorContext for AppError {
    fn context(self, context: &str) -> Self {
        match self {
            AppError::Proxy(err) => AppError::Proxy(match err {
                ProxyError::InvalidRequest(msg) => {
                    ProxyError::InvalidRequest(format!("{}: {}", context, msg))
                }
                ProxyError::UnknownMethod(msg) => {
                    ProxyError::UnknownMethod(format!("{}: {}", context, msg))
                }
                ProxyError::PhaseNotFound(msg) => {
                    ProxyError::PhaseNotFound(format!("{}: {}", context, msg))
                }
                ProxyError::AgentNotFound(msg) => {
                    ProxyError::AgentNotFound(format!("{}: {}", context, msg))
                }
                ProxyError::Internal(msg) => ProxyError::Internal(format!("{}: {}", context, msg)),
                ProxyError::RateLimitExceeded(msg) => {
                    ProxyError::RateLimitExceeded(format!("{}: {}", context, msg))
                }
                ProxyError::CircuitBreakerOpen(msg) => {
                    ProxyError::CircuitBreakerOpen(format!("{}: {}", context, msg))
                }
                ProxyError::Timeout(msg) => ProxyError::Timeout(format!("{}: {}", context, msg)),
            }),
            // Similar implementations for other error variants...
            _ => self,
        }
    }
}

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
    fn test_error_context() {
        let error = AppError::Proxy(ProxyError::InvalidRequest("bad input".to_string()));
        let error_with_context = error.context("request processing");

        match error_with_context {
            AppError::Proxy(ProxyError::InvalidRequest(msg)) => {
                assert_eq!(msg, "request processing: bad input");
            }
            _ => panic!("expected ProxyError::InvalidRequest with context"),
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

        assert_eq!(returns_result().unwrap(), "success");
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
}
