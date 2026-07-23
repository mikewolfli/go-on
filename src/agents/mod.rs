//! Agent implementations
//!
//! This module provides implementations for various AI agents, including AI21 Labs, Aleph Alpha, Anthropic, Cohere, Copilot, DeepQuest, DeepSeek, Doubao (ByteDance), FaceWall, Fireworks AI, Gemini (Google), GLM (Zhipu AI), Groq, Hunyuan (Tencent), Langboat, Llama (Meta), Loop AI, MiniMax, Mistral AI, Moonshot, Nim, OpenAI, OpenAI-compatible, Perplexity AI, Qianfan (Baidu), Qwen (Alibaba), Replicate, Skywork, StepFun, Together AI, Titan (Amazon), Wenxin (Baidu), Xihu, and Yi (01.AI).

pub mod agent;
pub mod ai21;
pub mod aleph;
pub mod anthropic;
pub mod cohere;
pub mod communication;
pub mod copilot;
pub mod deepquest;
pub mod deepseek;
pub mod facewall;
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "simple-server",
    feature = "multi-users-server"
))]
pub mod factory;
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "simple-server",
    feature = "multi-users-server"
))]
pub mod fireworks;
pub mod gemini;
pub mod glm;
pub mod groq;
pub mod hunyuan;
pub mod kimi;
pub mod langboat;
pub mod llama;
pub mod loopai;
pub mod minimax;
pub mod mistral;
pub mod moonshot;
pub mod nim;
pub mod openai;
pub mod openai_compatible;
pub mod perplexity;
pub mod progress_reporter;
pub mod qianfan;
pub mod replicate;
pub mod self_evolution_agent; // GAP-B52-03: Self-Evolution Agent
pub mod siliconflow;
pub mod skywork;
pub mod sse_compressor;
pub mod sse_optimizer;
pub mod stepfun;
pub mod titan;
pub mod together;
pub mod wenxin;
pub mod xai;
pub mod xihu;
pub mod yi;

use std::collections::HashMap;

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::{debug, warn};

use crate::orchestration::autonomy_runtime::{build_thinking_token, build_tool_call_token};

/// Accumulated tool call state across SSE chunks (index-keyed).
/// Zed uses the same approach: `tool_calls_by_index: HashMap<usize, RawToolCall>`
/// that accumulates `id`, `name`, `arguments` incrementally.
#[derive(Default)]
pub(crate) struct ToolCallAcc {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

thread_local! {
    // Thread-local accumulator for multi-chunk tool calls (index-keyed).
    // Each tokio worker thread gets its own instance, avoiding cross-stream pollution
    // when multiple chat streams run concurrently on different threads.
    static TOOL_CALL_ACC: std::cell::RefCell<HashMap<usize, ToolCallAcc>> =
        std::cell::RefCell::new(HashMap::new());
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

pub use ai21::Ai21Agent;
pub use aleph::AlephAgent;
pub use anthropic::AnthropicAgent;
pub use cohere::CohereAgent;
pub use copilot::CopilotAgent;
pub use deepquest::DeepQuestAgent;
pub use deepseek::DeepSeekAgent;

pub use facewall::FaceWallAgent;
pub use fireworks::FireworksAgent;
pub use gemini::GeminiAgent;
pub use glm::GlmAgent;
pub use groq::GroqAgent;
pub use hunyuan::HunyuanAgent;
pub use kimi::KimiAgent;
pub use langboat::LangboatAgent;
pub use llama::LlamaAgent;
pub use loopai::LoopAiAgent;
pub use minimax::MiniMaxAgent;
pub use mistral::MistralAgent;
pub use moonshot::MoonshotAgent;
pub use nim::NimAgent;
pub use openai::OpenAiAgent;
pub use openai_compatible::OpenAiCompatibleAgent;
pub use perplexity::PerplexityAgent;
pub use qianfan::QianfanAgent;
pub use replicate::ReplicateAgent;
pub use siliconflow::SiliconFlowAgent;
pub use skywork::SkyworkAgent;
pub use stepfun::StepFunAgent;
pub use titan::TitanAgent;
pub use together::TogetherAgent;
pub use wenxin::WenxinAgent;
pub use xai::XaiAgent;
pub use xihu::XihuAgent;
pub use yi::YiAgent;

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
        // Fast UTF-8 decode — SSE streams from LLM APIs are almost always
        // valid UTF-8, so we avoid the allocation overhead of from_utf8_lossy
        // by checking first. Falls back to lossy conversion for robustness.
        let chunk_text = std::str::from_utf8(&chunk)
            .map(|s| s.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(&chunk).to_string());
        match parser.push_chunk(&chunk_text) {
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

/// Process a single SSE event through the sender.
/// Returns `true` when the stream should stop (sender dropped or [DONE]).
/// Process one SSE event and send token(s) through the sender.
/// Returns `true` when the stream should stop (`[DONE]` or sender dropped).
///
/// Matches Zed's approach: for a single SSE chunk, extracts ALL fields from
/// choices[0].delta (or .message) and sends them in Zed's order:
///   1. reasoning (Anthropic-style)     → __thinking__ prefix
///   2. reasoning_content (DeepSeek)    → __thinking__ prefix
///   3. content (regular text)          → as-is
///   4. tool_calls (function calling)   → __tool_call__ prefix
fn sse_event_to_sender(event: &str, sender: &crate::agent::StreamingSender) -> bool {
    if event.trim() == "[DONE]" {
        return true;
    }
    let Ok(json) = serde_json::from_str::<Value>(event) else {
        return false;
    };
    for token in extract_all_tokens(&json) {
        if sender.send(token).is_err() {
            return true;
        }
    }
    false
}

pub async fn stream_sse_to_sender(
    response: reqwest::Response,
    sender: crate::agent::StreamingSender,
) -> anyhow::Result<()> {
    stream_sse_events(response, move |data| {
        if sse_event_to_sender(data, &sender) {
            Ok(SseEventAction::Stop)
        } else {
            Ok(SseEventAction::Continue)
        }
    })
    .await
}

/// Stream SSE events to sender with optional gzip compression.
///
/// When `config.enable_compression` is true, response chunks are buffered
/// and compressed with gzip before being processed, reducing bandwidth for
/// large streaming responses. This is particularly useful for models that
/// return verbose output or when operating over constrained networks.
pub async fn stream_sse_to_sender_compressed(
    response: reqwest::Response,
    sender: crate::agent::StreamingSender,
    config: &StreamingConfig,
) -> anyhow::Result<()> {
    if !config.enable_compression {
        return stream_sse_to_sender(response, sender).await;
    }

    let cfg = config.clone();
    let mut decompressor = SseDecompressor::new(&cfg);

    // Verify compression is active and track buffer state
    if !decompressor.is_enabled() {
        return stream_sse_to_sender(response, sender).await;
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
        let chunk_text = std::str::from_utf8(&decompressed)
            .map(|s| s.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(&decompressed).to_string());
        match parser.push_chunk(&chunk_text) {
            Ok(events) => {
                for event in events {
                    if sse_event_to_sender(&event, &sender) {
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
        let tail_text = std::str::from_utf8(&tail)
            .map(|s| s.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(&tail).to_string());
        // Parse decompressed tail data through a fresh parser
        let mut tail_parser = SseEventParser::default();
        if let Ok(events) = tail_parser.push_chunk(&tail_text) {
            for event in events {
                if sse_event_to_sender(&event, &sender) {
                    break;
                }
            }
        }
        // Finish any remaining partial data in the tail parser
        for event in tail_parser.finish() {
            if sse_event_to_sender(&event, &sender) {
                break;
            }
        }
    }

    // Process any remaining events accumulated by the main parser
    // that weren't terminated by a newline before stream end.
    for event in parser.finish() {
        if sse_event_to_sender(&event, &sender) {
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
/// Empty vec means no content was found in this event.
pub(crate) fn extract_all_tokens(value: &Value) -> Vec<String> {
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
        TOOL_CALL_ACC.with(|cell| {
            let mut acc = cell.borrow_mut();
            for tc in tool_calls {
                let index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let entry = acc.entry(index).or_default();
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
        });
    }

    // 5. finish_reason — forward as __finish_reason__:<reason>
    //     If the reason signals stream end ("stop", "length", "tool_calls"), drain
    //     any remaining accumulated tool calls and clear the thread-local accumulator
    //     so a new stream on the same thread starts fresh.
    if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
        if !reason.is_empty() && reason != "null" {
            tokens.push(format!("__finish_reason__:{}", reason));
            TOOL_CALL_ACC.with(|cell| {
                let mut acc = cell.borrow_mut();
                for (_, entry) in acc.drain() {
                    if !entry.name.is_empty() && !entry.id.is_empty() {
                        let fixed = strip_trailing_incomplete_escape(&entry.arguments);
                        tokens.push(build_tool_call_token(&entry.name, &fixed));
                    }
                }
            });
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
            tokens.push(format!("__usage__:{},{},{}", prompt, completion, total));
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

        assert_eq!(extract_all_tokens(&delta_array), vec!["alphabeta"]);
        assert_eq!(extract_all_tokens(&result_field), vec!["wenxin-token"]);
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
        let tokens = extract_all_tokens(&stream_chunk);
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
        let tokens2 = extract_all_tokens(&stream_chunk2);
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
        let tokens3 = extract_all_tokens(&finish_event);
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
        let tokens = extract_all_tokens(&event);
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
