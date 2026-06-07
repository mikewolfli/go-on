//! Minimal `ImageAttachment` type for sharing image data between the backend
//! and GUI/VSCode clients over the wire.
//!
//! # Motivation
//!
//! The multimodal processor (`crate::multimodal::MultimodalInput::Image`) uses
//! raw `Vec<u8>` internally. The protocol types here provide a serialisable
//! envelope so that GUI and VS Code clients can attach images to chat requests
//! in a structured, self-describing format (e.g. JSON with base64-encoded bytes
//! and a MIME hint).
//!
//! # Usage
//!
//! ```rust,ignore
//! use go_on::shared::image_attachment::ImageAttachment;
//!
//! let attachment = ImageAttachment {
//!     data: "iVBORw0KGgo...".to_string(),  // base64
//!     mime_type: "image/png".to_string(),
//!     alt_text: Some("Screenshot".to_string()),
//! };
//! let json = serde_json::to_string(&attachment)?;
//! ```

use serde::{Deserialize, Serialize};

/// A structured image attachment that can be sent over the wire between
/// the backend and GUI/VSCode clients.
///
/// ### Fields
/// - `data` — base64-encoded image bytes
/// - `mime_type` — MIME type hint (e.g. `"image/png"`, `"image/jpeg"`, `"image/webp"`)
/// - `alt_text` — optional human-readable description (for accessibility / logging)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ImageAttachment {
    /// Base64-encoded image bytes.
    pub data: String,
    /// MIME type hint (e.g. "image/png", "image/jpeg", "image/webp").
    pub mime_type: String,
    /// Optional human-readable description of the image content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
}

impl ImageAttachment {
    /// Create a new `ImageAttachment` from raw bytes, encoding them as base64.
    ///
    /// The `mime_type` parameter should be a valid image MIME type
    /// (e.g. `"image/png"`, `"image/jpeg"`, `"image/webp"`).
    #[allow(dead_code)]
    pub fn from_bytes(
        bytes: &[u8],
        mime_type: impl Into<String>,
        alt_text: Option<String>,
    ) -> Self {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        Self {
            data,
            mime_type: mime_type.into(),
            alt_text,
        }
    }

    /// Decode the base64 `data` field back into raw bytes.
    #[allow(dead_code)]
    pub fn decode(&self) -> Result<Vec<u8>, base64::DecodeError> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(&self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_attachment_round_trip() {
        let raw: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]; // PNG magic
        let attachment = ImageAttachment::from_bytes(&raw, "image/png", Some("test".into()));

        let json = serde_json::to_string(&attachment).expect("serialize");
        let decoded: ImageAttachment = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.mime_type, "image/png");
        assert_eq!(decoded.alt_text, Some("test".to_string()));
        let recovered = decoded.decode().expect("base64 decode");
        assert_eq!(recovered, raw);
    }

    #[test]
    fn test_image_attachment_no_alt() {
        let raw = b"hello world".to_vec();
        let attachment = ImageAttachment::from_bytes(&raw, "image/jpeg", None);

        let json = serde_json::to_string(&attachment).expect("serialize");
        let decoded: ImageAttachment = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.mime_type, "image/jpeg");
        assert!(decoded.alt_text.is_none());
        let recovered = decoded.decode().expect("base64 decode");
        assert_eq!(recovered, raw);
    }
}
