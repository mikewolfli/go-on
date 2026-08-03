//! # Unified Execution Loop — AutonomyLoop
//!
//! This module is the **single entry point** for all tool-execution loops
//! in go-on.  It packages the real agent-driven execution loop into one API:
//!
//! | Layer | Module | Role |
//! |-------|--------|------|
//! | **Execution** | `acp/helpers/autonomy/autonomy_loop.rs` | Thin agent-driven tool executor: call LLM → parse tool tokens → run tools → loop |
//!
//! ## Architecture
//!
//! ```text
//! chat.rs / chat_phases.rs
//!         │
//!         ▼
//!   run_acp_autonomy_loop()    ← YOU ARE HERE — the unified entry point
//!         │
//!         └─► run_autonomy_loop()    [Execution layer]
//!                  (agent → tools → loop)
//! ```
//!
//! The former `use_brain_loop` branch (BrainLoop orchestration) was
//! bookkeeping-only — it never invoked the agent or tools — so it has been
//! removed; every request now drives the same real execution path.

use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::acp::r#impl::chat::StreamFrame;
use crate::agent::{Agent, Message};

use super::autonomy_loop::{
    run_autonomy_loop, AutonomyLoopConfig, AutonomyLoopParams, AutonomyLoopResult,
};
use crate::orchestration::tool::ToolRegistry;

/// Parameters for `run_acp_autonomy_loop`, bundled to avoid clippy `too_many_arguments`.
#[allow(missing_docs)]
pub(crate) struct AcpAutonomyLoopParams {
    pub agent: Arc<dyn Agent>,
    pub tool_registry: Option<Arc<ToolRegistry>>,
    pub messages: Vec<Message>,
    pub acp_session_id: Option<String>,
    pub principles: Option<Vec<String>>,
    pub options: Option<std::collections::HashMap<String, Value>>,
    pub timeout_duration: Option<std::time::Duration>,
    pub stream_tx: Option<mpsc::UnboundedSender<String>>,
    pub progress_sse_tx: Option<mpsc::UnboundedSender<StreamFrame>>,
    /// Operation mode for tool approval events (edit, safeguard, full_auto, etc.)
    pub operation_mode: String,
}

/// Run the multi-round autonomy loop in an ACP-compatible way.
///
/// This function wraps `run_autonomy_loop` while respecting the ACP
/// streaming contract:
///   - Each round's assistant output is streamed to `stream_tx`.
///   - Tool-observation follow-up is handled transparently inside the loop.
///   - The final response, reasoning, and model are returned.
pub(crate) async fn run_acp_autonomy_loop(
    params: AcpAutonomyLoopParams,
) -> Result<AutonomyLoopResult> {
    let objective = extract_objective(&params.messages);
    let config = AutonomyLoopConfig {
        max_iterations: 5,
        // Enable persistent loop so the autonomy loop doesn't stop after
        // one text-only round — it continues with a planning prompt to
        // encourage tool-based autonomous execution (like Zed's agent).
        persistent_loop: true,
        operation_mode: params.operation_mode.clone(),
        acp_session_id: params.acp_session_id.clone(),
        progress_tx: params.progress_sse_tx.clone(),
        // The `use_brain_loop` option is accepted for backward compatibility
        // but both branches drive the same real execution loop: the former
        // BrainLoop orchestration path was bookkeeping-only (it never called
        // the agent or tools), so it has been removed in favour of the single
        // real execution path below.
        use_brain_loop: false,
    };

    let loop_params = AutonomyLoopParams {
        agent: params.agent,
        tool_registry: params.tool_registry,
        objective,
        messages: params.messages,
        principles: params.principles,
        options: params.options,
    };
    let result = run_autonomy_loop(loop_params, config, params.timeout_duration).await?;

    // Stream the final response if a channel was provided
    if let Some(tx) = params.stream_tx {
        for chunk in split_for_streaming(&result.response, 256) {
            if tx.send(chunk).is_err() {
                tracing::warn!("autonomy_loop_adapter: streaming receiver disconnected");
                break;
            }
        }
    }

    Ok(result)
}

/// Extract a concise objective from the message list.
fn extract_objective(messages: &[Message]) -> String {
    messages
        .iter()
        .rfind(|m| m.role == "user")
        .map(|m| {
            let text = m.content.trim();
            // Take first 200 chars as the objective
            // Use chars() to avoid panic on multi-byte UTF-8 boundary
            let char_count = text.chars().count();
            if char_count > 200 {
                format!("{}...", text.chars().take(200).collect::<String>())
            } else {
                text.to_string()
            }
        })
        .unwrap_or_else(|| "complete the user request".to_string())
}

/// Split text into chunks for streaming (preserving word boundaries).
/// Uses char_indices to avoid panic on multi-byte UTF-8 boundaries.
fn split_for_streaming(text: &str, chunk_size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start_byte = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return chunks;
    }
    let mut start_idx = 0; // char index
    while start_idx < chars.len() {
        let end_idx = (start_idx + chunk_size).min(chars.len());
        let end_byte = if end_idx < chars.len() {
            // end_byte is the byte offset of the char at end_idx — safe boundary
            chars[end_idx].0
        } else {
            text.len()
        };
        // Try to break at a word boundary (space)
        let break_at = if end_idx < chars.len() {
            // Search backwards in the visible bytes for a space
            let search_bytes = &text.as_bytes()[start_byte..end_byte];
            let last_space = search_bytes
                .iter()
                .rposition(|&b| b == b' ')
                .map(|pos| start_byte + pos + 1);
            match last_space {
                Some(boundary) => boundary,
                None => end_byte,
            }
        } else {
            text.len()
        };
        chunks.push(text[start_byte..break_at].to_string());
        // Advance start_byte to break_at, and start_idx to the corresponding char index
        start_byte = break_at;
        // Find the char index for the new start byte
        match chars.binary_search_by_key(&start_byte, |&(b, _)| b) {
            Ok(idx) => start_idx = idx,
            Err(idx) => start_idx = idx,
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_objective_uses_last_user_message() {
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: "I'll help".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "refactor the auth module".to_string(),
            },
        ];
        let obj = extract_objective(&messages);
        assert_eq!(obj, "refactor the auth module");
    }

    #[test]
    fn extract_objective_truncates_long_text() {
        let long = "a".repeat(300);
        let messages = vec![Message {
            role: "user".to_string(),
            content: long.clone(),
        }];
        let obj = extract_objective(&messages);
        assert_eq!(obj.len(), 203); // 200 chars + "..."
        assert!(obj.ends_with("..."));
    }

    #[test]
    fn split_for_streaming_respects_boundaries() {
        let text = "hello world foo bar baz";
        let chunks = split_for_streaming(text, 10);
        assert!(chunks.len() >= 2);
        // Each chunk should be ≤ 10 chars
        for chunk in &chunks {
            assert!(
                chunk.len() <= 10,
                "chunk '{}' is {} chars",
                chunk,
                chunk.len()
            );
        }
        // Concatenating should recover the original text
        let joined: String = chunks.join("");
        assert_eq!(joined, text);
    }
}
