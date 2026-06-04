//! Agent implementations
//!
//! This module provides implementations for various AI agents, including AI21 Labs, Aleph Alpha, Anthropic, Cohere, Copilot, DeepQuest, DeepSeek, Doubao (ByteDance), FaceWall, Fireworks AI, Gemini (Google), GLM (Zhipu AI), Groq, Hunyuan (Tencent), Langboat, Llama (Meta), Loop AI, MiniMax, Mistral AI, Moonshot, Nim, OpenAI, OpenAI-compatible, Perplexity AI, Qianfan (Baidu), Qwen (Alibaba), Replicate, Skywork, StepFun, Together AI, Titan (Amazon), Wenxin (Baidu), Xihu, and Yi (01.AI).

pub mod agent;
pub mod ai21;
pub mod aleph;
pub mod anthropic;
pub mod cohere;
pub mod copilot;
pub mod deepquest;
pub mod deepseek;
pub mod facewall;
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "profile-simple-server",
    feature = "profile-multi-users-server"
))]
pub mod factory;
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
pub mod vendors;
pub mod wenxin;
pub mod xai;
pub mod xihu;
pub mod yi;

// These modules are publicly exported and will be fully wired in upcoming integrations.
#[cfg(test)]
mod integration_gate {
    fn _gate_sse_optimizer() {
        let _ = super::sse_optimizer::SseBufferPool::new(4, 1024);
    }

    fn _gate_siliconflow() {
        let _ = super::siliconflow::SiliconFlowAgent::new(
            "KEY".to_string(),
            "https://api.siliconflow.cn".to_string(),
            "test-model".to_string(),
            reqwest::Client::new(),
        );
    }

    fn _gate_xai() {
        let _ = super::xai::XaiAgent::new(
            "KEY".to_string(),
            "https://api.x.ai".to_string(),
            "grok-3".to_string(),
            reqwest::Client::new(),
        );
    }
}

use std::collections::HashMap;

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::{debug, warn};

use crate::orchestration::autonomy_runtime::{build_thinking_token, build_tool_call_token};

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

pub async fn stream_sse_to_sender(
    response: reqwest::Response,
    sender: crate::agent::StreamingSender,
) -> anyhow::Result<()> {
    stream_sse_events(response, move |data| {
        // Fast path: extract content using string operations, avoiding
        // full JSON Value allocation on every SSE token.
        if let Some(token) = fast_extract_token(data) {
            if sender.send(token).is_err() {
                return Ok(SseEventAction::Stop);
            }
        } else if data.trim() == "[DONE]" {
            return Ok(SseEventAction::Stop);
        }

        Ok(SseEventAction::Continue)
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
        let chunk_text = std::str::from_utf8(&decompressed)
            .map(|s| s.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(&decompressed).to_string());
        match parser.push_chunk(&chunk_text) {
            Ok(events) => {
                for event in events {
                    if let Some(token) = fast_extract_token(&event) {
                        if sender.send(token).is_err() {
                            return Ok(());
                        }
                    } else if event.trim() == "[DONE]" {
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
                if let Some(token) = fast_extract_token(&event) {
                    let _ = sender.send(token);
                } else if event.trim() == "[DONE]" {
                    break;
                }
            }
        }
        // Finish any remaining partial data in the tail parser
        for event in tail_parser.finish() {
            if let Some(token) = fast_extract_token(&event) {
                let _ = sender.send(token);
            } else if event.trim() == "[DONE]" {
                break;
            }
        }
    }

    // Process any remaining events accumulated by the main parser
    // that weren't terminated by a newline before stream end.
    for event in parser.finish() {
        if let Some(token) = fast_extract_token(&event) {
            let _ = sender.send(token);
        } else if event.trim() == "[DONE]" {
            break;
        }
    }

    Ok(())
}

/// Fast SSE token extraction — avoids full JSON `Value` allocation for the
/// common case (content tokens from `choices[0].delta.content`).
///
/// Falls back to full `serde_json::from_str` + `extract_token` for complex
/// payloads (tool calls, thinking tokens, content arrays, non-standard fields).
#[inline]
fn fast_extract_token(data: &str) -> Option<String> {
    // ── Fast string-based content extraction (common case) ──────────
    // Bypasses full JSON Value allocation for the typical SSE token:
    //   {"choices":[{"delta":{"content":"Hello"}}]}
    // Uses simple string search — ~5-10x faster than serde_json Value parsing.
    // Scope the search within a "delta" object to avoid false matches.
    let delta_scope = data.find(r#""delta""#).map(|i| &data[i..]);
    if let Some(scope) = delta_scope {
        if let Some(content) = try_fast_extract_field(scope, r#""content":""#, true) {
            return Some(content);
        }
    }

    // ── Reasoning / thinking content (fast path) ─────────────────────
    if let Some(scope) = delta_scope {
        if let Some(thinking) = try_fast_extract_field(scope, r#""reasoning_content":""#, true) {
            // Check if content also exists in the same delta
            if let Some(text) = try_fast_extract_field(scope, r#""content":""#, true) {
                return Some(build_thinking_token(&thinking, Some(&text)));
            }
            return Some(build_thinking_token(&thinking, None));
        }
    }

    // ── Fallback: full JSON parsing for complex cases ───────────────
    // Tool calls, content arrays (OpenAI text parts), non-standard
    // fields like `result`, `text`, and final `message.content` all
    // require proper JSON parsing.
    if let Ok(json) = serde_json::from_str::<Value>(data) {
        extract_token(&json)
    } else {
        None
    }
}

/// Fast string-based extraction of a JSON string field by its key name.
///
/// Searches for `"key":"` in `data`, extracts the string value (handling
/// JSON escape sequences), and returns `Some(value)` on success.
/// If `unescape` is true, JSON escape sequences (\", \\, \n, \t, \r, \/)
/// are decoded; otherwise the raw substring is returned.
fn try_fast_extract_field(data: &str, key: &str, unescape: bool) -> Option<String> {
    let start = data.find(key)?;
    let value_start = start + key.len();
    let bytes = data.as_bytes();
    let mut end = value_start;
    while end < data.len() && bytes[end] != b'"' {
        if bytes[end] == b'\\' {
            end += 2; // skip escaped char
        } else {
            end += 1;
        }
    }
    if end <= value_start || end > data.len() {
        return None;
    }
    let raw = &data[value_start..end];
    if unescape && raw.contains('\\') {
        let mut result = String::with_capacity(raw.len());
        let rb = raw.as_bytes();
        let mut i = 0;
        while i < raw.len() {
            if rb[i] == b'\\' && i + 1 < raw.len() {
                match rb[i + 1] {
                    b'"' => result.push('"'),
                    b'n' => result.push('\n'),
                    b'\\' => result.push('\\'),
                    b't' => result.push('\t'),
                    b'r' => result.push('\r'),
                    b'/' => result.push('/'),
                    _ => {}
                }
                i += 2;
            } else {
                result.push(rb[i] as char);
                i += 1;
            }
        }
        return Some(result);
    }
    Some(raw.to_string())
}

pub(crate) fn extract_token(value: &Value) -> Option<String> {
    // ── Tool call detection ──────────────────────────────────────────
    // When the LLM responds with tool_calls (function calling), encode
    // them as structured text tokens so the chat handler can detect and
    // execute the corresponding skills.  The format is:
    //   __tool_call__:<tool_name>:<json_arguments>
    //
    // Check both delta (streaming) and message (non-streaming) locations.
    if let Some(tool_calls) = value
        .get("choices")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("delta"))
        .and_then(|v| v.get("tool_calls"))
        .and_then(|v| v.as_array())
    {
        for tc in tool_calls {
            if let (Some(name), Some(args)) = (
                tc.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str()),
                tc.get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str()),
            ) {
                let token = build_tool_call_token(name, args);
                return Some(token);
            }
        }
    }

    // Also check the final message (non-streaming) for tool_calls
    if let Some(tool_calls) = value
        .get("choices")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("message"))
        .and_then(|v| v.get("tool_calls"))
        .and_then(|v| v.as_array())
    {
        for tc in tool_calls {
            if let (Some(name), Some(args)) = (
                tc.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str()),
                tc.get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str()),
            ) {
                let token = build_tool_call_token(name, args);
                return Some(token);
            }
        }
    }

    // ── Reasoning / thinking content extraction ──────────────────────
    // DeepSeek and some other OpenAI-compatible APIs return thinking/
    // reasoning tokens in delta.reasoning_content (streaming) or
    // message.reasoning_content (non-streaming). We prefix these with
    // __thinking__ so the chat handler can separate them from the main
    // response and stream them as "reasoning" SSE events.
    //
    // If BOTH reasoning_content AND content are present in the same delta,
    // both are returned: first the thinking prefix with reasoning, then the
    // content (concatenated so neither is lost).
    if let Some(thinking) = value
        .get("choices")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("delta"))
        .and_then(|v| v.get("reasoning_content"))
        .and_then(|v| v.as_str())
    {
        if !thinking.is_empty() {
            // Check if content also exists in the same delta
            let content = value
                .get("choices")
                .and_then(|v| v.get(0))
                .and_then(|v| v.get("delta"))
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str());
            if let Some(text) = content {
                if !text.is_empty() {
                    return Some(build_thinking_token(thinking, Some(text)));
                }
            }
            return Some(build_thinking_token(thinking, None));
        }
    }
    if let Some(thinking) = value
        .get("choices")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("message"))
        .and_then(|v| v.get("reasoning_content"))
        .and_then(|v| v.as_str())
    {
        if !thinking.is_empty() {
            // Check if content also exists in the same message
            let content = value
                .get("choices")
                .and_then(|v| v.get(0))
                .and_then(|v| v.get("message"))
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str());
            if let Some(text) = content {
                if !text.is_empty() {
                    return Some(build_thinking_token(thinking, Some(text)));
                }
            }
            return Some(build_thinking_token(thinking, None));
        }
    }

    // ── Standard content extraction ───────────────────────────────────
    if let Some(token) = value
        .get("choices")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("delta"))
        .and_then(|v| v.get("content"))
        .and_then(|v| v.as_str())
    {
        return Some(token.to_string());
    }

    if let Some(parts) = value
        .get("choices")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("delta"))
        .and_then(|v| v.get("content"))
        .and_then(|v| v.as_array())
    {
        let mut merged = String::new();
        for part in parts {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                merged.push_str(text);
            }
        }
        if !merged.is_empty() {
            return Some(merged);
        }
    }

    if let Some(token) = value
        .get("choices")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("message"))
        .and_then(|v| v.get("content"))
        .and_then(|v| v.as_str())
    {
        return Some(token.to_string());
    }

    if let Some(token) = value
        .get("choices")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
    {
        return Some(token.to_string());
    }

    value
        .get("result")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
    fn extract_token_supports_openai_delta_arrays_and_result_fields() {
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

        assert_eq!(extract_token(&delta_array).as_deref(), Some("alphabeta"));
        assert_eq!(
            extract_token(&result_field).as_deref(),
            Some("wenxin-token")
        );
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
