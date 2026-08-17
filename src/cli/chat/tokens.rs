//! Token accounting for the terminal chat loop: cumulative usage/cost
//! tracking and the shared streaming-token classifier.

use serde::{Deserialize, Serialize};

use crate::i18n::runtime::tf;
use crate::orchestration::autonomy_runtime::{
    parse_tool_call_token, REASONING_END, REASONING_START, TOKEN_FINISH_REASON_PREFIX,
    TOKEN_THINKING_PREFIX, TOKEN_USAGE_PREFIX,
};

use super::ansi;

/// Default pricing fallback: GPT-4o input cost per token ($0.15 per 1M tokens).
/// Used when provider cost info is unavailable.
const GPT4O_INPUT_COST_PER_TOKEN: f64 = 0.15 / 1_000_000.0;

/// Default pricing fallback: GPT-4o output cost per token ($0.60 per 1M tokens).
/// Used when provider cost info is unavailable.
const GPT4O_OUTPUT_COST_PER_TOKEN: f64 = 0.60 / 1_000_000.0;

/// Record a completed turn's token usage and print the turn-complete summary.
/// Shared by the main request path and the `/retry` path (previously the
/// same 8-line block in both).
pub(super) fn record_turn_usage(
    token_tracker: &mut TokenTracker,
    prompt_tokens: usize,
    completion_tokens: usize,
    resp: &str,
) {
    token_tracker.record_usage(prompt_tokens, completion_tokens);
    if !resp.trim().is_empty() {
        eprintln!(
            "{}{}{}",
            ansi!("90"),
            tf(
                "cli.chat.turn_complete",
                &[("tokens", &(prompt_tokens + completion_tokens).to_string())]
            ),
            ansi!("0")
        );
    }
}

/// Semantic classification of a streaming token.
///
/// Shared by the three streaming loops (primary agent phase, follow-up phase,
/// `chat_simple`) so the marker/telemetry filter rules cannot drift between
/// them. The predicates are mutually exclusive, so the check order does not
/// matter; each caller keeps its own display behavior per kind.
pub(super) enum TokenKind<'a> {
    /// `__tool_call__:name:args` protocol token (tool name; args are not
    /// inspected by the chat display loops).
    ToolCall(&'a str),
    /// Reasoning-content start marker.
    ReasoningStart,
    /// Reasoning-content end marker.
    ReasoningEnd,
    /// `__thinking__`-prefixed reasoning token (payload after the prefix).
    Thinking(&'a str),
    /// Finish-reason or usage telemetry — never displayed.
    Telemetry,
    /// Regular content token.
    Content,
}

/// Classify a streaming token using the canonical marker/telemetry rules.
///
/// See [`TokenKind`] for the semantics of each kind. `__model_used__:` tokens
/// match none of the markers and are classified as `Content`, preserving the
/// historical behavior of the chat loops (they are only intercepted by the
/// ACP-side `classify_agent_token` used in the server path).
pub(super) fn classify_token(token: &str) -> TokenKind<'_> {
    if let Some((tool_name, _)) = parse_tool_call_token(token) {
        return TokenKind::ToolCall(tool_name);
    }
    if token == REASONING_START {
        return TokenKind::ReasoningStart;
    }
    if token == REASONING_END {
        return TokenKind::ReasoningEnd;
    }
    if let Some(think) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
        return TokenKind::Thinking(think);
    }
    if token.starts_with(TOKEN_FINISH_REASON_PREFIX) || token.starts_with(TOKEN_USAGE_PREFIX) {
        return TokenKind::Telemetry;
    }
    TokenKind::Content
}

/// Track cumulative token usage and cost across the session.
#[derive(Default, Clone, Serialize, Deserialize)]
pub(super) struct TokenTracker {
    total_prompt_tokens: usize,
    total_completion_tokens: usize,
    total_cost_usd: f64,
}

impl TokenTracker {
    fn record_usage(&mut self, prompt_tokens: usize, completion_tokens: usize) {
        self.total_prompt_tokens += prompt_tokens;
        self.total_completion_tokens += completion_tokens;
        // GPT-4o reference pricing: the CLI has no per-provider cost table
        // (token counts arrive without model info), so a fixed reference rate
        // is used for the displayed estimate.
        self.total_cost_usd += (prompt_tokens as f64 * GPT4O_INPUT_COST_PER_TOKEN)
            + (completion_tokens as f64 * GPT4O_OUTPUT_COST_PER_TOKEN);
    }

    pub(super) fn display(&self) -> String {
        format!(
            "{}Tokens:{}{} prompt + {} completion = {} total  |  Cost: ${:.6}\n",
            ansi!("1"),
            ansi!("0"),
            self.total_prompt_tokens,
            self.total_completion_tokens,
            self.total_prompt_tokens + self.total_completion_tokens,
            self.total_cost_usd,
        )
    }
}
