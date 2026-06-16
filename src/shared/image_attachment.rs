//! Image attachment module — provides `ImageAttachment` for embedding inline
//! images into multimodal inputs (e.g. GUI / VSCode clients).
//!
//! Attachments are base64-encoded for JSON serialization.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// An inline image attachment with base64-encoded data, MIME type, and optional
/// alt text. Used by GUI clients to send images to the backend.
#[allow(dead_code)] // Public API for test/tool consumers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    /// Base64-encoded image bytes.
    pub data: String,
    /// MIME type (e.g. "image/png", "image/webp").
    pub mime_type: String,
    /// Optional alt text description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
}

impl ImageAttachment {
    /// Create a new `ImageAttachment` from raw image bytes.
    #[allow(dead_code)] // Public API for test/tool consumers
    pub fn from_bytes(
        bytes: &[u8],
        mime_type: impl Into<String>,
        alt_text: Option<String>,
    ) -> Self {
        let engine = base64::engine::general_purpose::STANDARD;
        Self {
            data: engine.encode(bytes),
            mime_type: mime_type.into(),
            alt_text,
        }
    }

    /// Decode the base64 data back into raw bytes.
    #[allow(dead_code)] // Public API for test/tool consumers
    pub fn decode(&self) -> Result<Vec<u8>, base64::DecodeError> {
        let engine = base64::engine::general_purpose::STANDARD;
        engine.decode(&self.data)
    }
}
