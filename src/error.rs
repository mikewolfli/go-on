//! Error types
//!
//! This module defines the error types used throughout the application.

use thiserror::Error;

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
    UnknownPhase(String),

    /// Agent not found error
    #[error("agent not found: {0}")]
    UnknownAgent(String),

    /// Internal error
    #[error("internal error: {0}")]
    Internal(String),
}
