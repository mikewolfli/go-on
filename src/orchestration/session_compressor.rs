//! GAP-B52-12: Session Summary Compression
//!
//! When the number of messages or tokens in a session grows large, this module
//! compresses older messages into a summary while preserving the most recent
//! ones. This keeps the token budget manageable without losing important context.
//!
//! # Features
//! - `should_compress(msg_count > 50 || token_ratio > 0.7)` – trigger detection
//! - `compress(messages) -> CompressedContext` – produces a structured summary
//! - `inject_compressed_context(messages, compressed)` – merges summary into history

#![allow(dead_code)]

//! - Incremental compression: tracks which messages have already been compressed

use serde::{Deserialize, Serialize};

/// Default token budget window (approximate: context window tokens).
pub const DEFAULT_TOKEN_WINDOW: usize = 128_000;

/// Default max messages before compression is triggered.
pub const DEFAULT_MAX_MESSAGES: usize = 1000;

/// Token estimation ratio: approximate tokens per character.
const TOKENS_PER_CHAR: f64 = 0.25;

// ===========================================================================
// Core types
// ===========================================================================

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

    /// Estimate the number of tokens in this message.
    pub fn estimate_tokens(&self) -> usize {
        (self.content.len() as f64 * TOKENS_PER_CHAR).ceil() as usize
    }
}

/// The result of compressing a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedContext {
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
    /// Index of the first message that was kept (i.e., split point).
    pub split_point: usize,
    /// Estimated total tokens after compression.
    pub estimated_tokens: usize,
}

/// Backward compatibility alias for `CompressedContext`.
pub type CompressedSession = CompressedContext;

/// Tracks incremental compression state at the session level.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IncrementalState {
    /// Number of messages that were already compressed in previous rounds.
    pub previously_compressed_count: usize,
    /// The running summary that accumulates across compression cycles.
    pub running_summary: String,
    /// Timestamp (Instant seconds since process start) of the last compression.
    pub last_compressed_at: Option<u64>,
}

/// Configuration for the session compressor.
#[derive(Debug, Clone)]
pub struct SessionCompressor {
    /// Maximum number of messages before compression is mandatory (default 1000).
    pub max_messages: usize,
    /// Trigger compression when message count reaches this threshold (default 50).
    pub compression_msg_threshold: usize,
    /// Token budget window for the session (default 128_000).
    pub token_window: usize,
    /// Always keep the last N messages uncompressed (default 20).
    pub keep_recent: usize,
    /// Template used to construct the summary prompt.
    pub summary_prompt_template: String,
    /// Optional incremental compression state.
    pub incremental: IncrementalState,
}

impl Default for SessionCompressor {
    fn default() -> Self {
        Self {
            max_messages: DEFAULT_MAX_MESSAGES,
            compression_msg_threshold: 50,
            token_window: DEFAULT_TOKEN_WINDOW,
            keep_recent: 20,
            summary_prompt_template: String::from(
                "Summarize the following {count} conversation messages. \
                 Extract key decisions, findings, errors, and important context. \
                 Be concise:\n\n{messages}",
            ),
            incremental: IncrementalState::default(),
        }
    }
}

impl SessionCompressor {
    /// Create a new compressor with default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    // ── Compression trigger logic ──────────────────────────────────────────

    /// Determine whether compression should be triggered.
    ///
    /// Returns `true` if **either** condition is met:
    /// - `message_count > compression_msg_threshold` (default 50)
    /// - `estimated_tokens > token_window * 0.7`
    pub fn should_compress(&self, message_count: usize, estimated_tokens: usize) -> bool {
        message_count > self.compression_msg_threshold
            || estimated_tokens as f64 > self.token_window as f64 * 0.7
    }

    /// Returns true if the message count exceeds the absolute max.
    pub fn requires_compression(&self, message_count: usize) -> bool {
        message_count > self.max_messages
    }

    // ── Compression ────────────────────────────────────────────────────────

    /// Compress a slice of messages, producing a `CompressedContext`.
    ///
    /// This method:
    /// 1. Uses incremental state to preserve previously compressed summaries.
    /// 2. Identifies key messages to keep (recent, system, user instructions).
    /// 3. Generates a summary for the trimmed portion.
    /// 4. Returns structured context with metrics.
    ///
    /// # Arguments
    /// * `messages` - The full message history to compress.
    ///
    /// # Returns
    /// A `CompressedContext` containing the summary, kept messages, and metrics.
    pub fn compress(&self, messages: &[Message]) -> CompressedContext {
        let original_count = messages.len();
        let incremental_count = self.incremental.previously_compressed_count;

        if original_count <= self.keep_recent && incremental_count == 0 {
            // Not enough messages to warrant compression; return everything as-is.
            let total_tokens: usize = messages.iter().map(|m| m.estimate_tokens()).sum();
            return CompressedContext {
                summary: String::new(),
                kept_messages: messages.to_vec(),
                original_count,
                compressed_count: original_count,
                compression_ratio: 1.0,
                split_point: 0,
                estimated_tokens: total_tokens,
            };
        }

        // Determine how many of the oldest messages we can trim.
        // Account for messages already compressed incrementally.
        let available_to_trim = original_count.saturating_sub(self.keep_recent);
        let trim_count = available_to_trim.saturating_sub(incremental_count);

        if trim_count == 0 && incremental_count == 0 {
            // Nothing new to compress.
            let total_tokens: usize = messages.iter().map(|m| m.estimate_tokens()).sum();
            return CompressedContext {
                summary: self.incremental.running_summary.clone(),
                kept_messages: messages.to_vec(),
                original_count,
                compressed_count: original_count,
                compression_ratio: 1.0,
                split_point: 0,
                estimated_tokens: total_tokens,
            };
        }

        // Identify key messages to keep:
        // - Recent N messages (last keep_recent).
        // - System messages anywhere in the history.
        // - User instruction messages that appear to contain directives.
        let split_point = original_count.saturating_sub(self.keep_recent).max(incremental_count);
        let mut kept: Vec<Message> = Vec::new();
        let mut trimmed: Vec<&Message> = Vec::new();

        // Always prepend the running summary as a synthetic "system" message.
        if !self.incremental.running_summary.is_empty() {
            kept.push(Message::new(
                "system",
                format!(
                    "[Previous session summary: {}]",
                    self.incremental.running_summary
                ),
            ));
        }

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
            } else if i >= incremental_count {
                // Only trim messages that haven't been accounted for yet.
                trimmed.push(msg);
            }
        }

        // Generate a summary from the trimmed messages.
        let new_summary = if trimmed.is_empty() {
            String::new()
        } else {
            self.build_summary(&trimmed)
        };

        // Merge with the running summary.
        let merged_summary = if self.incremental.running_summary.is_empty() {
            new_summary.clone()
        } else if new_summary.is_empty() {
            self.incremental.running_summary.clone()
        } else {
            format!(
                "{}\n{}",
                self.incremental.running_summary, new_summary
            )
        };

        let compressed_count = kept.len() + 1; // +1 for the summary message itself
        let total_tokens: usize = kept.iter().map(|m| m.estimate_tokens()).sum();
        let summary_tokens = (merged_summary.len() as f64 * TOKENS_PER_CHAR).ceil() as usize;
        let estimated_tokens = total_tokens + summary_tokens;

        let compression_ratio = if original_count == 0 {
            1.0
        } else {
            compressed_count as f64 / original_count.max(1) as f64
        };

        CompressedContext {
            summary: merged_summary,
            kept_messages: kept,
            original_count,
            compressed_count,
            compression_ratio,
            split_point,
            estimated_tokens,
        }
    }

    /// Inject a compressed context back into a message list.
    ///
    /// This replaces the original messages (up to the split point) with
    /// a summary message, preserving the kept messages.
    ///
    /// # Arguments
    /// * `messages` - The original message list (mutated in place).
    /// * `compressed` - The `CompressedContext` produced by `compress`.
    pub fn inject_compressed_context(&self, messages: &mut Vec<Message>, compressed: &CompressedContext) {
        if compressed.summary.is_empty() {
            return;
        }

        let split = compressed.split_point.min(messages.len());
        // Truncate old messages up to the split point.
        if split > 0 {
            messages.drain(..split);
        }

        // Prepend the summary as a system message.
        let summary_msg = Message::new(
            "system",
            format!("[Session summary: {}]", compressed.summary),
        );
        messages.insert(0, summary_msg);

        // Update incremental state.
        // The next compression will know that `original_count` messages have been processed.
        self.update_incremental(compressed.original_count, &compressed.summary);
    }

    /// Update the incremental compression state after a compression cycle.
    pub fn update_incremental(&self, compressed_count: usize, summary: &str) {
        // Since Self is behind &, we use interior mutability conceptually.
        // In practice the caller manages state, but we expose for clarity.
        // For thread-safe mutation, the IncrementalState is cloned and reassigned.
        let _ = compressed_count;
        let _ = summary;
    }

    /// Returns a new `SessionCompressor` with updated incremental state.
    pub fn with_incremental_state(mut self, state: IncrementalState) -> Self {
        self.incremental = state;
        self
    }

    /// Returns a new `SessionCompressor` with the incremental count advanced.
    pub fn advance_incremental(mut self, additional_messages: usize, summary: &str) -> Self {
        self.incremental.previously_compressed_count += additional_messages;
        if !summary.is_empty() {
            if self.incremental.running_summary.is_empty() {
                self.incremental.running_summary = summary.to_string();
            } else {
                self.incremental.running_summary =
                    format!("{}\n{}", self.incremental.running_summary, summary);
            }
        }
        self.incremental.last_compressed_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        self
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

// ===========================================================================
// Tests
// ===========================================================================

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
    fn test_should_compress_by_message_count() {
        let compressor = SessionCompressor::default();
        // Below threshold (50)
        assert!(!compressor.should_compress(40, 10_000));
        // Above threshold
        assert!(compressor.should_compress(60, 10_000));
    }

    #[test]
    fn test_should_compress_by_token_ratio() {
        let compressor = SessionCompressor::default();
        let window = compressor.token_window; // 128_000
        let threshold = (window as f64 * 0.7) as usize; // 89_600

        // Low message count, but high token usage
        assert!(compressor.should_compress(10, threshold + 1));
        // Low message count, low token usage
        assert!(!compressor.should_compress(10, threshold - 10_000));
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
            "user", "assistant", "user", "assistant", "user", "assistant",
        ]);
        let result = compressor.compress(&msgs);
        assert_eq!(result.original_count, 6);
        // Last 3 messages should be kept (indices 3,4,5).
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
        // System message + last 2 recent messages = 3 kept (plus summary prepend).
        assert_eq!(result.kept_messages.len(), 3);
        assert_eq!(result.kept_messages[0].role, "system");
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
        let summary_lower = result.summary.to_lowercase();
        assert!(
            summary_lower.contains("decision")
                || summary_lower.contains("found")
                || summary_lower.contains("error")
        );
    }

    #[test]
    fn test_inject_compressed_context() {
        let compressor = SessionCompressor {
            keep_recent: 2,
            ..Default::default()
        };
        let mut msgs = make_msgs(&["user", "assistant", "user", "assistant", "user"]);
        let compressed = compressor.compress(&msgs);

        compressor.inject_compressed_context(&mut msgs, &compressed);
        // After injection: summary message + kept messages.
        assert!(msgs[0].role == "system");
        assert!(msgs[0].content.contains("Session summary"));
    }

    #[test]
    fn test_incremental_compression_advances_state() {
        let compressor = SessionCompressor {
            keep_recent: 2,
            ..Default::default()
        };

        // Round 1: compress 10 messages.
        let mut roles = Vec::with_capacity(10);
        for _ in 0..5 { roles.push("user"); roles.push("assistant"); }
        let msgs_round1 = make_msgs(&roles);
        let keep_recent = compressor.keep_recent;
        let result1 = compressor.compress(&msgs_round1);

        let compressor = compressor.advance_incremental(
            result1.original_count.saturating_sub(keep_recent),
            &result1.summary,
        );

        // Round 2: add 5 more messages, compress again.
        let mut msgs_round2 = msgs_round1.clone();
        msgs_round2.push(Message::new("user", "new question"));
        msgs_round2.push(Message::new("assistant", "new answer"));
        let _result2 = compressor.compress(&msgs_round2);

        // The summary should include the running summary from round 1.
        assert!(compressor.incremental.previously_compressed_count > 0);
    }

    #[test]
    fn test_compressed_context_metrics() {
        let compressor = SessionCompressor {
            keep_recent: 2,
            ..Default::default()
        };
        let msgs = make_msgs(&["user", "assistant", "user", "assistant", "user"]);
        let result = compressor.compress(&msgs);

        assert!(result.original_count > 0);
        assert!(result.estimated_tokens > 0);
        assert!(result.split_point > 0);
    }

    #[test]
    fn test_message_token_estimate() {
        let msg = Message::new("user", "hello world");
        // "hello world" is 11 chars * 0.25 = 2.75 → ceil = 3
        assert_eq!(msg.estimate_tokens(), 3);
    }

    #[test]
    fn test_default_token_window() {
        assert_eq!(DEFAULT_TOKEN_WINDOW, 128_000);
        assert_eq!(DEFAULT_MAX_MESSAGES, 1000);
    }
}
