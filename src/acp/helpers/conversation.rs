//! Conversation helper functions for ACP server
//!
//! This module provides utility functions for managing streaming limits,
//! latency monitoring, pipeline gates, and storage validation.

/// Maximum stream chunks
pub const MAX_STREAM_CHUNKS: usize = 4_096;

/// Maximum stream characters
pub const MAX_STREAM_CHARS: usize = 256_000;

/// Check if streaming would exceed limits
pub fn stream_would_exceed_limits(
    current_chunks: usize,
    current_chars: usize,
    next_token_chars: usize,
) -> bool {
    current_chunks.saturating_add(1) > MAX_STREAM_CHUNKS
        || current_chars.saturating_add(next_token_chars) > MAX_STREAM_CHARS
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── stream limits ─────────────────────────────────────────────────

    #[test]
    fn stream_would_exceed_limits_chunk_boundary() {
        assert!(stream_would_exceed_limits(MAX_STREAM_CHUNKS, 0, 0));
        assert!(!stream_would_exceed_limits(MAX_STREAM_CHUNKS - 1, 0, 10));
    }

    #[test]
    fn stream_would_exceed_limits_char_boundary() {
        assert!(stream_would_exceed_limits(0, MAX_STREAM_CHARS, 1));
        assert!(!stream_would_exceed_limits(0, MAX_STREAM_CHARS - 100, 50));
    }

    #[test]
    fn stream_would_exceed_limits_zero_chars_ok() {
        assert!(!stream_would_exceed_limits(0, 0, 0));
    }
}
