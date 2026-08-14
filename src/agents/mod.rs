//! Agent implementations
//!
//! This module provides implementations for various AI agents, including Anthropic, Cohere,
//! Copilot, DeepSeek, Gemini (Google), OpenAI, OpenAI-compatible, and Baidu ERNIE
//! (Wenxin + Qianfan unified).

pub mod agent;
pub mod anthropic;
pub mod baidu_auth;
pub mod cohere;
pub mod communication;
pub mod copilot;
pub mod deepseek;
pub mod ernie;
pub mod gemini;
// OpenAI agent removed — fully superseded by `OpenAiCompatibleAgent`. File deleted.
pub mod openai_compatible;
pub mod self_evolution_agent; // GAP-B52-03: Self-Evolution Agent

pub mod sse_compressor;
pub mod sse_optimizer;

use std::collections::HashMap;

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::{debug, warn};

use crate::orchestration::autonomy_runtime::{
    build_thinking_token, build_tool_call_token, TOKEN_FINISH_REASON_PREFIX, TOKEN_USAGE_PREFIX,
};

/// Accumulated tool call state across SSE chunks (index-keyed).
/// Zed uses the same approach: `tool_calls_by_index: HashMap<usize, RawToolCall>`
/// that accumulates `id`, `name`, `arguments` incrementally.
#[derive(Default)]
pub(crate) struct ToolCallAcc {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Per-stream accumulator for multi-chunk tool calls (index-keyed).
///
/// Owned by the SSE streaming loop — created fresh per `chat_once` stream —
/// NOT thread-local: at each `stream.next().await` the loop can migrate
/// between tokio worker threads, and a thread-local would split one stream's
/// deltas across threads (the finish_reason drain on one thread would only
/// see that thread's partial entries). It is also dropped on every stream end
/// path (finish_reason drain, `[DONE]`, client disconnect, network error), so
/// a retried or subsequent stream can never append to another stream's
/// leftovers.
#[derive(Default)]
pub(crate) struct ToolCallAccumulator {
    pub entries: HashMap<usize, ToolCallAcc>,
}

/// Strip trailing incomplete escape sequences from partial JSON tool arguments.
/// Zed's `strip_trailing_incomplete_escape` does the same transformation.
pub(crate) fn strip_trailing_incomplete_escape(s: &str) -> String {
    if s.is_empty() {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut end = s.len();
    // Walk backwards from the end, skipping complete escape sequences
    while end > 0 {
        if bytes[end - 1] == b'"' || (bytes[end - 1] > 0x1f && bytes[end - 1] != b'\\') {
            break;
        }
        if bytes[end - 1] == b'\\' && end > 1 {
            // Check if this is a complete escape: \", \\, \n, \t, \r, \/, \b, \f
            // or an incomplete one (trailing backslash)
            if matches!(
                bytes[end - 2],
                b'\\' | b'"' | b'n' | b't' | b'r' | b'/' | b'b' | b'f'
            ) {
                break;
            }
            // Trailing incomplete escape: remove the backslash
            end -= 1;
            break;
        }
        end -= 1;
    }
    s[..end].to_string()
}

pub use anthropic::AnthropicAgent;
pub use cohere::CohereAgent;
pub use copilot::CopilotAgent;
pub use deepseek::DeepSeekAgent;
pub use ernie::{BaiduErnieAgent, ErnieApi};
pub use gemini::GeminiAgent;
pub use openai_compatible::OpenAiCompatibleAgent;

pub use sse_compressor::{SseDecompressor, StreamingConfig};

/// Convert principles to text format
///
/// # Arguments
/// * `principles` - List of principles
///
/// # Returns
/// * `String` - Formatted principles text
pub fn principles_to_text(principles: &[String]) -> String {
    let mut text = String::from("Please follow these programming principles:\n");
    for line in principles {
        text.push_str("- ");
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// Normalize an optional principles list into a system prompt text.
/// Returns `None` when there are no principles (or the list is empty), so
/// providers can skip system injection entirely in that case.
pub fn principles_to_system_text(principles: &Option<Vec<String>>) -> Option<String> {
    let items = principles.as_ref()?;
    if items.is_empty() {
        return None;
    }
    Some(principles_to_text(items))
}

/// Build a system prompt from principles plus an extra fragment (e.g. a
/// provider-specific stage note). Providers that always push a system message
/// (even when both inputs are empty) use this variant.
pub fn system_text_with_extra(principles: &Option<Vec<String>>, extra: &str) -> String {
    let mut text = String::new();
    if let Some(p) = principles_to_system_text(principles) {
        text.push_str(&p);
        text.push('\n');
    }
    text.push_str(extra);
    text
}

/// Get string option from HashMap
///
/// # Arguments
/// * `options` - Optional HashMap of options
/// * `key` - Option key
///
/// # Returns
/// * `Option<String>` - Optional string value
pub fn option_string(options: &Option<HashMap<String, Value>>, key: &str) -> Option<String> {
    options
        .as_ref()
        .and_then(|map| map.get(key))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

/// Get f64 option from HashMap
///
/// # Arguments
/// * `options` - Optional HashMap of options
/// * `key` - Option key
///
/// # Returns
/// * `Option<f64>` - Optional f64 value
pub fn option_f64(options: &Option<HashMap<String, Value>>, key: &str) -> Option<f64> {
    options
        .as_ref()
        .and_then(|map| map.get(key))
        .and_then(|v| v.as_f64())
}

/// Get u64 option from HashMap
///
/// # Arguments
/// * `options` - Optional HashMap of options
/// * `key` - Option key
///
/// # Returns
/// * `Option<u64>` - Optional u64 value
pub fn option_u64(options: &Option<HashMap<String, Value>>, key: &str) -> Option<u64> {
    options
        .as_ref()
        .and_then(|map| map.get(key))
        .and_then(|v| v.as_u64())
}

/// Shared cache for short-lived auth tokens with expiry-based auto-refresh.
///
/// Unified from `BaiduAuthClient` and `CopilotAgent`, which each maintained a
/// private `CachedToken { token, expires_at }` with the same fast-path
/// freshness check + slow-path refresh pattern (differing only in expiry
/// representation, lock type, and where the safety margin is applied). Expiry
/// is stored as Unix seconds; callers convert their provider-specific TTL.
pub(crate) struct TokenCache {
    inner: std::sync::Mutex<Option<CachedToken>>,
}

struct CachedToken {
    token: String,
    expires_at: u64,
}

impl TokenCache {
    pub(crate) fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(None),
        }
    }

    /// Return the cached token when it is still fresh — i.e. at least
    /// `safety_margin_secs` before expiry — otherwise `None`.
    pub(crate) fn fresh(&self, safety_margin_secs: u64) -> Option<String> {
        let now = unix_now_secs();
        let guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("token cache lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        guard
            .as_ref()
            .filter(|c| now + safety_margin_secs < c.expires_at)
            .map(|c| c.token.clone())
    }

    /// Store a freshly fetched token with its absolute expiry (Unix seconds).
    pub(crate) fn store(&self, token: String, expires_at_secs: u64) {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("token cache lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        *guard = Some(CachedToken {
            token,
            expires_at: expires_at_secs,
        });
    }
}

/// Current Unix time in seconds.
///
/// Delegates to the shared timestamp helper (`shared::timestamps::now_ts`) —
/// the single source of truth for epoch-second clocks, per the timestamps
/// module's documented intent.
pub(crate) fn unix_now_secs() -> u64 {
    crate::shared::timestamps::now_ts() as u64
}

/// Build a streaming config from request options, honoring the
/// `sse_compress` flag with a provider-specific default.
///
/// Previously each provider's `chat_once` inlined the same
/// `options["sse_compress"]` unwrap (4 copies) — this is the single source.
pub fn streaming_config(
    options: &Option<HashMap<String, Value>>,
    default_enable: bool,
) -> StreamingConfig {
    StreamingConfig {
        enable_compression: options
            .as_ref()
            .and_then(|o| o.get("sse_compress"))
            .and_then(|v| v.as_bool())
            .unwrap_or(default_enable),
        ..Default::default()
    }
}

/// Merge phase principles into the message list for providers that do not
/// carry a separate system-role concept (or honor it inconsistently).
///
/// When `supports_system` is true the principles are prepended as a `system`
/// message; otherwise they are prepended to the first user message so the
/// constraints survive providers that ignore `system`.
///
/// Unified from `OpenAiCompatibleAgent` and `CopilotAgent` (which duplicated
/// the user-prepend branch verbatim).
pub fn merge_principles_into_messages(
    messages: &[crate::agent::Message],
    principles: &Option<Vec<String>>,
    supports_system: bool,
) -> Vec<crate::agent::Message> {
    let Some(items) = principles else {
        return messages.to_vec();
    };

    if items.is_empty() {
        return messages.to_vec();
    }

    let instruction = principles_to_text(items);

    if supports_system {
        let mut merged = Vec::with_capacity(messages.len() + 1);
        merged.push(crate::agent::Message {
            role: "system".to_string(),
            content: instruction,
        });
        merged.extend(messages.iter().cloned());
        return merged;
    }

    // Providers that ignore `system`: prepend phase principles to the first
    // user message to preserve constraints.
    let mut owned = messages.to_vec();
    if let Some(first_user) = owned.iter_mut().find(|m| m.role == "user") {
        first_user.content = format!("{}\n{}", instruction, first_user.content);
    } else {
        owned.insert(
            0,
            crate::agent::Message {
                role: "user".to_string(),
                content: instruction,
            },
        );
    }

    owned
}

#[cfg(test)]
mod token_cache_tests {
    use super::TokenCache;

    #[test]
    fn fresh_returns_stored_token_within_margin() {
        let cache = TokenCache::new();
        assert_eq!(cache.fresh(60), None, "empty cache must not be fresh");

        let now = super::unix_now_secs();
        cache.store("tok-1".to_string(), now + 600);
        // 600s to expiry with a 60s margin → still fresh.
        assert_eq!(cache.fresh(60).as_deref(), Some("tok-1"));

        // Within the safety margin (only 30s left) → treated as stale.
        cache.store("tok-2".to_string(), now + 30);
        assert_eq!(cache.fresh(60), None, "token inside safety margin is stale");

        // Already expired → stale.
        cache.store("tok-3".to_string(), now.saturating_sub(10));
        assert_eq!(cache.fresh(60), None, "expired token is stale");
    }
}

/// Check an LLM API response for success and valid content-type.
/// Returns the response on success for further processing (e.g., streaming),
/// or bails with a descriptive error including the response body on failure.
pub async fn check_api_response(
    response: reqwest::Response,
    provider_name: &str,
) -> anyhow::Result<reqwest::Response> {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "{}",
            agent::chat_request_failed_msg(provider_name, &status.to_string(), &body)
        );
    }
    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
        warn!("{provider_name}: unexpected content-type: {ct}");
        anyhow::bail!("unexpected content-type: {ct}");
    }
    Ok(response)
}

/// Resolve the effective model name, substituting "auto" or empty with
/// the provider's first available (default) model. This prevents passing
/// "auto" as a literal model name to APIs that only accept real model IDs.
///
/// Resolution order:
/// 1. `options["model"]` — explicit user/request override
/// 2. `configured_model` — the agent's configured model (from config.toml)
/// 3. First model from `available_models()` — last-resort default
pub fn resolve_effective_model(
    configured_model: &str,
    options: &Option<HashMap<String, Value>>,
    available_models: &[crate::agent::ModelInfo],
) -> String {
    // 1. Check for explicit model override in request options
    if let Some(model) = option_string(options, "model") {
        if model != "auto" && !model.is_empty() {
            return model;
        }
    }

    // 2. Use configured model if it's a real model name (not "auto" / empty)
    if configured_model != "auto" && !configured_model.is_empty() {
        return configured_model.to_string();
    }

    // 3. Fallback: first available model (the provider's canonical default)
    if let Some(first) = available_models.first() {
        return first.id.clone();
    }

    // Ultimate fallback — should never reach here for any real provider
    "unknown".to_string()
}

/// Apply common OpenAI chat completion options into payload.
///
/// This keeps compatibility with OpenAI-compatible fields while allowing
/// providers to ignore unknown keys safely.
pub fn apply_openai_common_options(payload: &mut Value, options: &Option<HashMap<String, Value>>) {
    let Some(map) = options.as_ref() else {
        return;
    };

    const KEYS: &[&str] = &[
        "temperature",
        "top_p",
        "max_tokens",
        "n",
        "stop",
        "presence_penalty",
        "frequency_penalty",
        "logit_bias",
        "user",
        "seed",
        "response_format",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "function_call",
        "functions",
        "reasoning_effort",
        "max_completion_tokens",
        "metadata",
        "modalities",
        "store",
        "stream_options",
    ];

    for key in KEYS {
        if let Some(value) = map.get(*key) {
            payload[*key] = value.clone();
        }
    }

    // ── Universal tools schema patch ─────────────────────────────────
    // Some APIs (e.g. GitHub Copilot) strictly validate that every
    // function's `parameters` object has a `properties` field.
    // Agent-generated tool definitions may omit it, so we ensure it exists.
    if let Some(tools) = payload.get_mut("tools").and_then(|t| t.as_array_mut()) {
        let strict_from_options = map.get("strict").and_then(|v| v.as_bool());
        for tool in tools.iter_mut() {
            if let Some(function) = tool.get_mut("function").and_then(|f| f.as_object_mut()) {
                if let Some(params) = function
                    .get_mut("parameters")
                    .and_then(|p| p.as_object_mut())
                {
                    if !params.contains_key("properties") {
                        params.insert("properties".to_string(), Value::Object(Default::default()));
                    }
                }

                // `strict` is a tool-level flag in provider tool schemas,
                // not a top-level chat-completions field.
                if let Some(strict) = strict_from_options {
                    function
                        .entry("strict".to_string())
                        .or_insert_with(|| Value::Bool(strict));
                }
            }
        }
    }
}

/// SSE event action
pub(crate) enum SseEventAction {
    /// Continue processing events
    Continue,
    /// Stop processing events
    Stop,
}

/// Maximum SSE line length to prevent unbounded memory growth
/// from malicious or buggy servers. 1 MB per line is generous
/// for any legitimate use case.
const MAX_SSE_LINE_BYTES: usize = 1_048_576;

/// Maximum SSE event data size (aggregated `data:` lines).
/// 4 MB total per event is sufficient for any LLM response chunk.
const MAX_SSE_EVENT_DATA_BYTES: usize = 4 * 1_048_576;

#[derive(Default)]
struct SseEventParser {
    buffer: String,
    event_data_lines: Vec<String>,
    event_data_total_bytes: usize,
}

impl SseEventParser {
    fn push_chunk(&mut self, chunk: &str) -> Result<Vec<String>> {
        if self.buffer.len() + chunk.len() > MAX_SSE_LINE_BYTES * 2 {
            return Err(anyhow::anyhow!(
                "SSE buffer exceeded maximum size ({} bytes)",
                MAX_SSE_LINE_BYTES * 2
            ));
        }
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(pos) = self.buffer.find('\n') {
            let mut line: String = self.buffer.drain(..=pos).collect();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                if let Some(event) = self.finish_event() {
                    events.push(event);
                }
                continue;
            }

            if line.len() > MAX_SSE_LINE_BYTES {
                // Truncate and discard — this is a DoS prevention measure.
                warn!(
                    "SSE line exceeds maximum length ({} bytes), discarding",
                    MAX_SSE_LINE_BYTES
                );
                self.buffer.clear();
                self.event_data_lines.clear();
                self.event_data_total_bytes = 0;
                return Err(anyhow::anyhow!("SSE line exceeded maximum allowed length"));
            }

            if line.starts_with(':') {
                continue;
            }

            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_str(), ""),
            };

            if field == "data" {
                let data = value.to_string();
                self.event_data_total_bytes += data.len();
                if self.event_data_total_bytes > MAX_SSE_EVENT_DATA_BYTES {
                    warn!(
                        "SSE event data exceeds maximum size ({} bytes), discarding",
                        MAX_SSE_EVENT_DATA_BYTES
                    );
                    self.event_data_lines.clear();
                    self.event_data_total_bytes = 0;
                    return Err(anyhow::anyhow!(
                        "SSE event data exceeded maximum allowed size"
                    ));
                }
                self.event_data_lines.push(data);
            }
        }

        Ok(events)
    }

    /// Push raw bytes (e.g. from a decompression layer) into the parser.
    ///
    /// Internally validates UTF-8 without allocating — the bytes are borrowed
    /// directly as `&str` in the common (valid UTF-8) case. Falls back to a
    /// lossy conversion only when invalid sequences are encountered.
    fn push_chunk_bytes(&mut self, chunk: &[u8]) -> Result<Vec<String>> {
        match std::str::from_utf8(chunk) {
            Ok(valid) => self.push_chunk(valid),
            Err(_) => {
                let owned = String::from_utf8_lossy(chunk).into_owned();
                self.push_chunk(&owned)
            }
        }
    }

    fn finish(&mut self) -> Vec<String> {
        if !self.buffer.is_empty() {
            let mut line = std::mem::take(&mut self.buffer);
            if line.ends_with('\r') {
                line.pop();
            }
            self.consume_line(&line);
        }

        self.finish_event().into_iter().collect()
    }

    fn finish_event(&mut self) -> Option<String> {
        if self.event_data_lines.is_empty() {
            return None;
        }

        let event = self.event_data_lines.join("\n");
        self.event_data_lines.clear();
        Some(event)
    }

    fn consume_line(&mut self, line: &str) {
        if line.is_empty() || line.starts_with(':') {
            return;
        }

        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };

        if field == "data" {
            self.event_data_lines.push(value.to_string());
        }
    }
}

pub(crate) async fn stream_sse_events<F>(response: reqwest::Response, mut on_event: F) -> Result<()>
where
    F: FnMut(&str) -> Result<SseEventAction>,
{
    let mut stream = response.bytes_stream();
    let mut parser = SseEventParser::default();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        // Delegate to push_chunk_bytes which borrows the bytes as &str
        // (zero-copy) in the common valid-UTF-8 case.
        match parser.push_chunk_bytes(&chunk) {
            Ok(events) => {
                for event in events {
                    if matches!(on_event(&event)?, SseEventAction::Stop) {
                        return Ok(());
                    }
                }
            }
            Err(e) => {
                warn!("SSE parse error (chunk), continuing stream: {e}");
                // Reset parser state to recover from malformed data
                parser = SseEventParser::default();
            }
        }
    }

    for event in parser.finish() {
        if matches!(on_event(&event)?, SseEventAction::Stop) {
            return Ok(());
        }
    }

    Ok(())
}

/// Process a single SSE event and send token(s) through the sender.
/// Returns `true` when the stream should stop (`[DONE]` or sender dropped).
///
/// Matches Zed's approach: for a single SSE chunk, extracts ALL fields from
/// choices[0].delta (or .message) and sends them in Zed's order:
///   1. reasoning (Anthropic-style)     → __thinking__ prefix
///   2. reasoning_content (DeepSeek)    → __thinking__ prefix
///   3. content (regular text)          → as-is
///   4. tool_calls (function calling)   → __tool_call__ prefix
///
/// `tool_acc` carries the per-stream tool-call accumulation state (see
/// [`ToolCallAccumulator`]) across events of the same stream.
pub(crate) fn sse_event_to_sender(
    event: &str,
    sender: &crate::agent::StreamingSender,
    tool_acc: &mut ToolCallAccumulator,
) -> bool {
    if event.trim() == "[DONE]" {
        return true;
    }
    let Ok(json) = serde_json::from_str::<Value>(event) else {
        return false;
    };
    for token in extract_all_tokens(&json, tool_acc) {
        if sender.send(token).is_err() {
            return true;
        }
    }
    false
}

pub async fn stream_sse_to_sender(
    response: reqwest::Response,
    sender: crate::agent::StreamingSender,
    config: &StreamingConfig,
) -> anyhow::Result<()> {
    // Per-stream tool-call accumulator: owned here so it is dropped on every
    // stream end path (Stop/[DONE]/error/EOF), discarding partial tool calls
    // instead of leaking them into the next stream.
    let mut tool_acc = ToolCallAccumulator::default();
    let on_event = |data: &str| -> anyhow::Result<SseEventAction> {
        if sse_event_to_sender(data, &sender, &mut tool_acc) {
            Ok(SseEventAction::Stop)
        } else {
            Ok(SseEventAction::Continue)
        }
    };
    stream_sse_with_handler(response, config, on_event).await
}

/// Unified chat_once tail for OpenAI-shaped providers: send → validate →
/// stream to sender. Providers whose payloads/headers differ only in the
/// `reqwest::RequestBuilder` construction reuse this instead of re-implementing
/// the send/check/stream boilerplate (deepseek, openai_compatible, ernie).
pub(crate) async fn execute_chat_stream_openai(
    request: reqwest::RequestBuilder,
    provider_name: &str,
    config: &StreamingConfig,
    sender: crate::agent::StreamingSender,
) -> anyhow::Result<()> {
    let response = request.send().await?;
    let response = check_api_response(response, provider_name).await?;
    stream_sse_to_sender(response, sender, config).await
}

/// Shared SSE streaming loop with a configurable per-event handler.
///
/// When `config.enable_compression` is set, the raw bytes are gzip-decompressed
/// before parsing; otherwise events are parsed directly. The handler returns
/// [`SseEventAction::Stop`] to end the stream early. This is the single
/// implementation of the decompressing loop — it replaces the former inline
/// copy inside `stream_sse_to_sender`.
pub(crate) async fn stream_sse_with_handler<F>(
    response: reqwest::Response,
    config: &StreamingConfig,
    mut on_event: F,
) -> anyhow::Result<()>
where
    F: FnMut(&str) -> anyhow::Result<SseEventAction>,
{
    if !config.enable_compression {
        return stream_sse_events(response, on_event).await;
    }

    let mut decompressor = SseDecompressor::new(config);

    // Verify compression is active and track buffer state
    if !decompressor.is_enabled() {
        return stream_sse_events(response, on_event).await;
    }
    debug!(
        "SSE compression active, buffer threshold={}, initial_size={}",
        config.compression_threshold,
        decompressor.buffered_bytes()
    );

    let mut stream = response.bytes_stream();
    let mut parser = SseEventParser::default();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        let decompressed = decompressor.decompress_chunk(&chunk);
        if decompressed.is_empty() {
            continue;
        }
        match parser.push_chunk_bytes(&decompressed) {
            Ok(events) => {
                for event in events {
                    if matches!(on_event(&event)?, SseEventAction::Stop) {
                        return Ok(());
                    }
                }
            }
            Err(e) => {
                warn!("SSE parse error (compressed chunk), continuing stream: {e}");
                parser = SseEventParser::default();
            }
        }
    }

    // Flush any remaining decompressed data and parse it
    let tail = decompressor.flush();
    if !tail.is_empty() {
        // Parse decompressed tail data through a fresh parser
        let mut tail_parser = SseEventParser::default();
        if let Ok(events) = tail_parser.push_chunk_bytes(&tail) {
            for event in events {
                if matches!(on_event(&event)?, SseEventAction::Stop) {
                    break;
                }
            }
        }
        // Finish any remaining partial data in the tail parser
        for event in tail_parser.finish() {
            if matches!(on_event(&event)?, SseEventAction::Stop) {
                break;
            }
        }
    }

    // Process any remaining events accumulated by the main parser
    // that weren't terminated by a newline before stream end.
    for event in parser.finish() {
        if matches!(on_event(&event)?, SseEventAction::Stop) {
            break;
        }
    }

    Ok(())
}

/// Extract ALL tokens from a single SSE event, preserving the field order:
///   1. reasoning (Anthropic-style) → __thinking__ prefix
///   2. reasoning_content (OpenAI-style, DeepSeek) → __thinking__ prefix
///   3. content (regular text) → as-is
///   4. tool_calls (function calling) → __tool_call__ prefix
///
/// Returns them as a Vec so the caller can send each one through the stream.
/// `tool_acc` accumulates streaming tool-call deltas across events of the same
/// stream and is drained when `finish_reason` arrives.
pub(crate) fn extract_all_tokens(value: &Value, tool_acc: &mut ToolCallAccumulator) -> Vec<String> {
    let mut tokens = Vec::with_capacity(4);

    let Some(choices) = value.get("choices").and_then(|v| v.as_array()) else {
        // Non-standard: top-level result field
        if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
            if !result.is_empty() {
                tokens.push(result.to_string());
            }
        }
        return tokens;
    };
    let Some(choice) = choices.first() else {
        return tokens;
    };

    // Prefer delta (streaming), fall back to message (non-streaming final)
    let container = choice.get("delta").or_else(|| choice.get("message"));
    let Some(container) = container else {
        // Non-standard: choices[0].text
        if let Some(text) = choice.get("text").and_then(|v| v.as_str()) {
            if !text.is_empty() {
                tokens.push(text.to_string());
            }
        }
        return tokens;
    };

    // 1. reasoning (Anthropic-style)
    if let Some(t) = container.get("reasoning").and_then(|v| v.as_str()) {
        if !t.is_empty() {
            tokens.push(build_thinking_token(t));
        }
    }

    // 2. reasoning_content (OpenAI-style, DeepSeek)
    if let Some(t) = container.get("reasoning_content").and_then(|v| v.as_str()) {
        if !t.is_empty() {
            tokens.push(build_thinking_token(t));
        }
    }

    // 3. content (regular text) — string or array
    if let Some(content) = container.get("content") {
        if let Some(text) = content.as_str() {
            if !text.is_empty() {
                tokens.push(text.to_string());
            }
        } else if let Some(arr) = content.as_array() {
            let mut merged = String::new();
            for part in arr {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    merged.push_str(text);
                }
            }
            if !merged.is_empty() {
                tokens.push(merged);
            }
        }
    }

    // 4. tool_calls (function calling) — accumulate across chunks by index
    if let Some(tool_calls) = container.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tool_calls {
            let index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let entry = tool_acc.entries.entry(index).or_default();
            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                if !id.is_empty() {
                    entry.id = id.to_string();
                }
            }
            if let Some(func) = tc.get("function") {
                if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                    if !name.is_empty() {
                        entry.name = name.to_string();
                    }
                }
                if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                    if !args.is_empty() {
                        entry.arguments.push_str(args);
                    }
                }
            }
            // NOTE: Tool call tokens are NOT emitted here during streaming.
            // Emitting partial tool call tokens during streaming causes duplicates
            // because each SSE chunk with tool_calls arrives with incremental
            // arguments (delta). Emitting after each chunk would produce multiple
            // partial/incomplete tokens, confusing downstream consumers.
            //
            // Instead, tool calls are only emitted once in step 5 (finish_reason)
            // when finish_reason == "tool_calls", at which point all arguments
            // have been fully accumulated. This matches the reliable behavior
            // of Zed's tool call handling.
        }
    }

    // 5. finish_reason — forward as __finish_reason__:<reason>
    //     If the reason signals stream end ("stop", "length", "tool_calls"), drain
    //     any remaining accumulated tool calls and clear the accumulator so a new
    //     stream starts fresh.
    if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
        if !reason.is_empty() && reason != "null" {
            tokens.push(format!("{}:{}", TOKEN_FINISH_REASON_PREFIX, reason));
            for (_, entry) in tool_acc.entries.drain() {
                if !entry.name.is_empty() && !entry.id.is_empty() {
                    let fixed = strip_trailing_incomplete_escape(&entry.arguments);
                    tokens.push(build_tool_call_token(&entry.name, &fixed));
                }
            }
        }
    }

    // 6. usage — forward token economy info
    if let Some(usage) = value.get("usage") {
        let prompt = usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let completion = usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if prompt > 0 || completion > 0 || total > 0 {
            tokens.push(format!(
                "{}:{},{},{}",
                TOKEN_USAGE_PREFIX, prompt, completion, total
            ));
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_trailing_incomplete_escape_handles_multibyte_and_control_bytes() {
        // Regression guard for the byte-wise backward walk: it must only ever
        // stop on a char boundary (ASCII `"`/`\` positions, or one byte past
        // a >0x1f byte that is the last byte of its UTF-8 char), so `s[..end]`
        // never panics even with CJK content and a trailing backslash/control
        // byte (a truncation point mid-escape in streamed tool arguments).
        let cases: &[(&str, &str)] = &[
            // Complete escapes are kept.
            ("{\"x\":\"a\\n\"", "{\"x\":\"a\\n\""),
            ("a\\\"", "a\\\""),
            // Trailing incomplete escape: the backslash is dropped.
            ("好\\", "好"),
            ("{\"patch\":\"你好\\", "{\"patch\":\"你好"),
            ("abc\\", "abc"),
            // Control byte tail: the control byte is dropped, then the walk
            // stops at the preceding char boundary (no panic on CJK).
            ("好\u{1e}", "好"),
            ("好\u{0}", "好"),
            ("你\u{1e}", "你"),
            // No trailing special byte: unchanged.
            ("{\"x\":\"你", "{\"x\":\"你"),
            ("\u{1e}好", "\u{1e}好"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                &strip_trailing_incomplete_escape(input),
                expected,
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn principles_to_system_text_normalizes_empty_and_none() {
        assert_eq!(principles_to_system_text(&None), None);
        assert_eq!(principles_to_system_text(&Some(vec![])), None);
        let text = principles_to_system_text(&Some(vec!["p1".to_string()]));
        assert!(text.is_some());
        let inner = text.unwrap();
        assert!(inner.contains("- p1"));
        assert!(inner.starts_with("Please follow"));
    }

    #[test]
    fn system_text_with_extra_keeps_principles_newline_and_extra() {
        let text = system_text_with_extra(&Some(vec!["p1".to_string()]), "NOTE");
        assert!(
            text.ends_with("\nNOTE"),
            "expected trailing extra after newline: {text:?}"
        );
        let only_extra = system_text_with_extra(&None, "NOTE");
        assert_eq!(only_extra, "NOTE");
        let empty = system_text_with_extra(&None, "");
        assert_eq!(empty, "");
    }

    #[test]
    fn sse_parser_joins_multiline_data_and_ignores_comments() {
        let mut parser = SseEventParser::default();

        let events = parser
            .push_chunk(": ping\r\ndata: first\r\ndata: second\r\n\r\n")
            .expect("should parse SSE chunk");

        assert_eq!(events, vec!["first\nsecond".to_string()]);
    }

    #[test]
    fn sse_parser_handles_partial_chunks_and_eof_flush() {
        let mut parser = SseEventParser::default();

        assert!(parser
            .push_chunk("data: {\"choices\":[{\"delta\":{\"content\":\"he")
            .expect("should parse")
            .is_empty());
        assert!(parser
            .push_chunk("llo\"}}]}\n")
            .expect("should parse")
            .is_empty());
        let events = parser.push_chunk("\n").expect("should parse");

        assert_eq!(events.len(), 1);
        assert!(events[0].contains("hello"));

        assert!(parser
            .push_chunk("data: tail without delimiter")
            .expect("should parse")
            .is_empty());
        assert_eq!(parser.finish(), vec!["tail without delimiter".to_string()]);
    }

    #[test]
    fn extract_all_tokens_supports_content_arrays_and_result_fields() {
        let delta_array = json!({
            "choices": [{
                "delta": {
                    "content": [
                        { "text": "alpha" },
                        { "text": "beta" }
                    ]
                }
            }]
        });
        let result_field = json!({ "result": "wenxin-token" });

        assert_eq!(
            extract_all_tokens(&delta_array, &mut ToolCallAccumulator::default()),
            vec!["alphabeta"]
        );
        assert_eq!(
            extract_all_tokens(&result_field, &mut ToolCallAccumulator::default()),
            vec!["wenxin-token"]
        );
    }

    #[test]
    fn extract_all_tokens_accumulates_tool_calls_until_finish_reason() {
        // When tool_calls arrive in a delta (streaming), they are ACCUMULATED
        // but NOT emitted until finish_reason == "tool_calls". This prevents
        // duplicate partial tool call emissions during streaming.
        let stream_chunk = json!({
            "choices": [{
                "delta": {
                    "reasoning": "anthropic-think",
                    "reasoning_content": "deepseek-think",
                    "content": "hello world",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "search",
                            "arguments": "{\"q\":\"test\"}"
                        }
                    }]
                }
            }]
        });
        // Without finish_reason, tool calls should NOT be emitted
        // (a single accumulator shared across the chunks of one stream)
        let mut tool_acc = ToolCallAccumulator::default();
        let tokens = extract_all_tokens(&stream_chunk, &mut tool_acc);
        assert_eq!(
            tokens.len(),
            3,
            "only thinking/thinking/content, no tool_call yet"
        );
        assert!(tokens[0].starts_with("__thinking__"), "reasoning first");
        assert!(
            tokens[1].starts_with("__thinking__"),
            "reasoning_content second"
        );
        assert_eq!(tokens[2], "hello world", "content third");

        // Second chunk: accumulated arguments should continue but not emit
        let stream_chunk2 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": {
                            "arguments": "_extended"
                        }
                    }]
                }
            }]
        });
        let tokens2 = extract_all_tokens(&stream_chunk2, &mut tool_acc);
        assert_eq!(
            tokens2.len(),
            0,
            "no tokens emitted from delta-only tool_calls"
        );

        // Finish chunk: finish_reason="tool_calls" should emit the accumulated call
        let finish_event = json!({
            "choices": [{
                "delta": {},
                "finish_reason": "tool_calls"
            }]
        });
        let tokens3 = extract_all_tokens(&finish_event, &mut tool_acc);
        assert!(tokens3.len() >= 2, "should have finish_reason + tool_call");
        assert!(
            tokens3[0].starts_with("__finish_reason__"),
            "finish_reason first"
        );
        assert!(
            tokens3[1].starts_with("__tool_call__"),
            "tool_call emitted after finish_reason"
        );
        assert!(tokens3[1].contains("search"), "tool name present");
        assert!(tokens3[1].contains("_extended"), "accumulated args present");
    }

    #[test]
    fn extract_all_tokens_emits_complete_tool_call_from_single_chunk() {
        // A single chunk with both tool_calls AND finish_reason="tool_calls"
        // should emit the tool call once (not duplicate).
        let event = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"a.txt\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let tokens = extract_all_tokens(&event, &mut ToolCallAccumulator::default());
        // finish_reason token + exactly one tool_call token
        let tool_tokens: Vec<&str> = tokens
            .iter()
            .filter(|t| t.starts_with("__tool_call__"))
            .map(|s| s.as_str())
            .collect();
        assert_eq!(tool_tokens.len(), 1, "should emit exactly ONE tool_call");
        assert!(tool_tokens[0].contains("read_file"));
        assert!(tool_tokens[0].contains("a.txt"));
    }

    #[test]
    fn apply_openai_common_options_injects_strict_into_tool_function_only() {
        let mut payload = json!({
            "model": "gpt-4o",
            "messages": [],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "search",
                        "parameters": {
                            "type": "object"
                        }
                    }
                }
            ]
        });

        let options = Some(HashMap::from([(String::from("strict"), Value::Bool(true))]));
        apply_openai_common_options(&mut payload, &options);

        assert!(payload.get("strict").is_none());
        assert_eq!(payload["tools"][0]["function"]["strict"], Value::Bool(true));
        assert!(payload["tools"][0]["function"]["parameters"]
            .get("properties")
            .is_some());
    }
}
