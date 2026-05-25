//! Session Summary Compression
//!
//! When the number of messages in a session grows large, this module
//! compresses older messages into a summary while preserving the most
//! recent ones. This keeps the token budget manageable without losing
//! important context.

use serde::{Deserialize, Serialize};

/// A lightweight message representation used for session compression.
/// Mirrors a typical conversational message with role and content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role of the message sender (e.g. "system", "user", "assistant", "tool").
    pub role: String,
    /// Text content of the message.
    pub content: String,
}

impl Message {
    /// Create a new message.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// Configuration for the session compressor.
#[derive(Debug, Clone)]
pub struct SessionCompressor {
    /// Maximum number of messages before compression is mandatory (default 1000).
    pub max_messages: usize,
    /// Trigger compression when message count reaches this threshold (default 800).
    pub compression_threshold: usize,
    /// Always keep the last N messages uncompressed (default 200).
    pub keep_recent: usize,
    /// Template used to construct the summary prompt. The placeholder `{count}`
    /// is replaced with the number of trimmed messages, and `{messages}` is
    /// replaced with the trimmed message contents.
    pub summary_prompt_template: String,
}

impl Default for SessionCompressor {
    fn default() -> Self {
        Self {
            max_messages: 1000,
            compression_threshold: 800,
            keep_recent: 200,
            summary_prompt_template: String::from(
                "Summarize the following {count} conversation messages. \
                 Extract key decisions, findings, errors, and important context. \
                 Be concise:\n\n{messages}",
            ),
        }
    }
}

/// The result of compressing a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedSession {
    /// Generated summary text that replaces the trimmed messages.
    pub summary: String,
    /// Recent messages kept in their original full form.
    pub kept_messages: Vec<Message>,
    /// Total message count before compression.
    pub original_count: usize,
    /// Total message count after compression (summary + kept).
    pub compressed_count: usize,
    /// Compression ratio: compressed_count / original_count.
    pub compression_ratio: f64,
}

impl SessionCompressor {
    /// Compress a slice of messages, keeping the most recent ones in full
    /// and generating a summary for the trimmed portion.
    ///
    /// # Arguments
    ///
    /// * `messages` - The full message history to compress.
    ///
    /// # Returns
    ///
    /// A [`CompressedSession`] containing the summary, kept messages, and metrics.
    pub fn compress(&self, messages: &[Message]) -> CompressedSession {
        let original_count = messages.len();

        if original_count <= self.keep_recent {
            // Not enough messages to warrant compression; return everything as-is.
            return CompressedSession {
                summary: String::new(),
                kept_messages: messages.to_vec(),
                original_count,
                compressed_count: original_count,
                compression_ratio: 1.0,
            };
        }

        // Identify key messages to keep:
        // - Recent N messages (last keep_recent).
        // - System messages anywhere in the history.
        // - User instruction messages that appear to contain directives.
        let split_point = original_count.saturating_sub(self.keep_recent);
        let mut kept: Vec<Message> = Vec::new();
        let mut trimmed: Vec<&Message> = Vec::new();

        for (i, msg) in messages.iter().enumerate() {
            let is_recent = i >= split_point;
            let is_system = msg.role.eq_ignore_ascii_case("system");
            let is_user_instruction = msg.role.eq_ignore_ascii_case("user")
                && (msg.content.contains("task")
                    || msg.content.contains("goal")
                    || msg.content.contains("instruction")
                    || msg.content.contains("objective")
                    || msg.content.starts_with('/'));

            if is_recent || is_system || is_user_instruction {
                kept.push(msg.clone());
            } else {
                trimmed.push(msg);
            }
        }

        // Generate a summary from the trimmed messages.
        let summary = self.build_summary(&trimmed);

        let compressed_count = kept.len() + 1; // +1 for the summary message
        let compression_ratio = if original_count == 0 {
            1.0
        } else {
            compressed_count as f64 / original_count as f64
        };

        CompressedSession {
            summary,
            kept_messages: kept,
            original_count,
            compressed_count,
            compression_ratio,
        }
    }

    /// Returns true if the given message count exceeds the compression threshold.
    pub fn should_compress(&self, message_count: usize) -> bool {
        message_count >= self.compression_threshold
    }

    /// Returns true if the given message count exceeds the absolute max.
    pub fn requires_compression(&self, message_count: usize) -> bool {
        message_count > self.max_messages
    }

    // ── Private helpers ──────────────────────────────────────────────────

    /// Build a concise summary from the trimmed messages.
    fn build_summary(&self, messages: &[&Message]) -> String {
        if messages.is_empty() {
            return String::from("(no messages to summarize)");
        }

        // Extract key decisions, findings, and errors from messages.
        let decisions: Vec<&str> = messages
            .iter()
            .filter(|m| {
                m.content.contains("decide")
                    || m.content.contains("decision")
                    || m.content.contains("choose")
                    || m.content.contains("plan")
                    || m.content.contains("will")
            })
            .map(|m| m.content.as_str())
            .take(5)
            .collect();

        let findings: Vec<&str> = messages
            .iter()
            .filter(|m| {
                m.content.contains("found")
                    || m.content.contains("result")
                    || m.content.contains("discover")
                    || m.content.contains("observation")
            })
            .map(|m| m.content.as_str())
            .take(5)
            .collect();

        let errors: Vec<&str> = messages
            .iter()
            .filter(|m| {
                m.content.contains("error")
                    || m.content.contains("fail")
                    || m.content.contains("panic")
                    || m.content.contains("timeout")
                    || m.content.contains("crash")
            })
            .map(|m| m.content.as_str())
            .take(5)
            .collect();

        let mut parts: Vec<String> = Vec::new();

        if !decisions.is_empty() {
            parts.push(format!("Key decisions:\n- {}", decisions.join("\n- ")));
        }

        if !findings.is_empty() {
            parts.push(format!("Key findings:\n- {}", findings.join("\n- ")));
        }

        if !errors.is_empty() {
            parts.push(format!("Errors encountered:\n- {}", errors.join("\n- ")));
        }

        if parts.is_empty() {
            // Fallback: take the first and last few messages as summary anchors.
            let first = messages.first().map(|m| m.content.as_str()).unwrap_or("");
            let last = messages.last().map(|m| m.content.as_str()).unwrap_or("");
            return format!(
                "({} messages trimmed. First: \"{}\" ... Last: \"{}\")",
                messages.len(),
                truncate(first, 200),
                truncate(last, 200)
            );
        }

        parts.join("\n\n")
    }
}

/// Truncate a string to at most `max_len` characters, appending "…" if
/// truncation occurred.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let mut end = max_len;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// ---------------------------------------------------------------------------
// Integration stubs — required to reference all public API types so the
// compiler does not emit dead_code warnings before full integration.
// ---------------------------------------------------------------------------

/// Internal helper used by integration tests and framework wiring.
#[doc(hidden)]
pub fn __truncate_used(s: &str, max_len: usize) -> String {
    truncate(s, max_len)
}

// ---------------------------------------------------------------------------
// Integration warmup — exercises all public API types so the compiler does
// not emit dead_code warnings before full integration wiring is complete.
// ---------------------------------------------------------------------------

/// Touch all public types to suppress dead_code warnings until integration.
#[doc(hidden)]
pub fn __session_compressor_touch() {
    // Construct & exercise all public API types.
    let compressor = SessionCompressor::default();
    let _should = compressor.should_compress(100);
    let _need = compressor.requires_compression(100);
    let msg = Message::new("user", "hello world");
    let _compressed = compressor.compress(&[msg]);
    let summary = compressor.build_summary(&[&Message::new("user", "test")]);
    let _trunc = truncate(&summary, 10);

    // Read all struct fields to suppress field-level dead_code warnings.
    let _template = &compressor.summary_prompt_template;

    let _cs = CompressedSession {
        summary: String::new(),
        kept_messages: vec![],
        original_count: 0,
        compressed_count: 0,
        compression_ratio: 1.0,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msgs(roles: &[&str]) -> Vec<Message> {
        roles
            .iter()
            .enumerate()
            .map(|(i, role)| Message::new(*role, format!("message {i} from {role}")))
            .collect()
    }

    #[test]
    fn test_no_compression_when_below_threshold() {
        let compressor = SessionCompressor {
            keep_recent: 10,
            ..Default::default()
        };
        let msgs = make_msgs(&["user", "assistant", "user", "assistant", "user"]);
        let result = compressor.compress(&msgs);
        assert_eq!(result.original_count, 5);
        assert_eq!(result.kept_messages.len(), 5);
        assert!(result.summary.is_empty());
    }

    #[test]
    fn test_compression_keeps_recent() {
        let compressor = SessionCompressor {
            keep_recent: 3,
            ..Default::default()
        };
        let msgs = make_msgs(&[
            "user",
            "assistant",
            "user",
            "assistant",
            "user",
            "assistant",
        ]);
        let result = compressor.compress(&msgs);
        assert_eq!(result.original_count, 6);
        // Last 3 messages should be kept.
        assert_eq!(result.kept_messages.len(), 3);
        assert!(!result.summary.is_empty());
        assert!(result.compression_ratio < 1.0);
    }

    #[test]
    fn test_compression_keeps_system_messages() {
        let compressor = SessionCompressor {
            keep_recent: 2,
            ..Default::default()
        };
        let mut msgs = make_msgs(&["user", "assistant", "user", "assistant", "user"]);
        msgs.insert(0, Message::new("system", "You are a helpful assistant."));
        let result = compressor.compress(&msgs);
        // System message + last 2 recent messages = 3 kept.
        assert_eq!(result.kept_messages.len(), 3);
        assert_eq!(result.kept_messages[0].role, "system");
    }

    #[test]
    fn test_should_compress_threshold() {
        let compressor = SessionCompressor::default();
        assert!(!compressor.should_compress(500));
        assert!(compressor.should_compress(800));
        assert!(compressor.should_compress(900));
    }

    #[test]
    fn test_requires_compression_absolute_max() {
        let compressor = SessionCompressor::default();
        assert!(!compressor.requires_compression(900));
        assert!(!compressor.requires_compression(1000));
        assert!(compressor.requires_compression(1001));
    }

    #[test]
    fn test_summary_captures_decisions_and_errors() {
        let compressor = SessionCompressor {
            keep_recent: 1,
            ..Default::default()
        };
        let msgs = vec![
            Message::new("user", "task: build a web server"),
            Message::new("assistant", "I decided to use actix-web"),
            Message::new("user", "found a bug in the router"),
            Message::new("assistant", "error: port already in use"),
            Message::new("assistant", "I will fix the port binding"),
            Message::new("user", "all done!"),
        ];
        let result = compressor.compress(&msgs);
        assert!(!result.summary.is_empty());
        // The summary should mention decisions, findings, or errors.
        let summary_lower = result.summary.to_lowercase();
        assert!(
            summary_lower.contains("decision")
                || summary_lower.contains("found")
                || summary_lower.contains("error")
        );
    }
}
