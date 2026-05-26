//! Error types for the go-on Rust SDK.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON-RPC error: code={code}, message={message}")]
    JsonRpc { code: i64, message: String },

    #[error("unexpected response shape: {0}")]
    UnexpectedShape(String),

    #[error("stream error: {0}")]
    Stream(String),
}
