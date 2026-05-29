//! Error types for the go-on Rust SDK.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("request timed out after {elapsed_secs}s")]
    Timeout { elapsed_secs: u64 },

    #[error("rate limited: {retry_after_secs}s until retry")]
    RateLimited { retry_after_secs: u64 },

    #[error("JSON-RPC error: code={code}, message={message}")]
    JsonRpc { code: i64, message: String },

    #[error("unexpected response shape: {0}")]
    UnexpectedShape(String),

    #[error("stream error: {0}")]
    Stream(String),
}
