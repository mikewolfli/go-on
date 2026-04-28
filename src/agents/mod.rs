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
pub mod factory;
pub mod fireworks;
pub mod gemini;
pub mod glm;
pub mod groq;
pub mod hunyuan;
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
pub mod qianfan;
pub mod qwen;
pub mod replicate;
pub mod skywork;
pub mod stepfun;
pub mod titan;
pub mod together;
pub mod vendors;
pub mod wenxin;
pub mod xihu;
pub mod yi;

use std::collections::HashMap;

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::Value;

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
pub use qwen::QwenAgent;
pub use replicate::ReplicateAgent;
pub use skywork::SkyworkAgent;
pub use stepfun::StepFunAgent;
pub use titan::TitanAgent;
pub use together::TogetherAgent;
pub use wenxin::WenxinAgent;
pub use xihu::XihuAgent;
pub use yi::YiAgent;

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
    ];

    for key in KEYS {
        if let Some(value) = map.get(*key) {
            payload[*key] = value.clone();
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

#[derive(Default)]
struct SseEventParser {
    buffer: String,
    event_data_lines: Vec<String>,
}

impl SseEventParser {
    fn push_chunk(&mut self, chunk: &str) -> Vec<String> {
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

            if line.starts_with(':') {
                continue;
            }

            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_str(), ""),
            };

            if field == "data" {
                self.event_data_lines.push(value.to_string());
            }
        }

        events
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
        let chunk_text = String::from_utf8_lossy(&chunk);
        for event in parser.push_chunk(&chunk_text) {
            if matches!(on_event(&event)?, SseEventAction::Stop) {
                return Ok(());
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
        if data.trim() == "[DONE]" {
            return Ok(SseEventAction::Stop);
        }

        if let Ok(json) = serde_json::from_str::<Value>(data) {
            if let Some(token) = extract_token(&json) {
                if sender.send(token).is_err() {
                    return Ok(SseEventAction::Stop);
                }
            }
        }

        Ok(SseEventAction::Continue)
    })
    .await
}

fn extract_token(value: &Value) -> Option<String> {
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

        let events = parser.push_chunk(": ping\r\ndata: first\r\ndata: second\r\n\r\n");

        assert_eq!(events, vec!["first\nsecond".to_string()]);
    }

    #[test]
    fn sse_parser_handles_partial_chunks_and_eof_flush() {
        let mut parser = SseEventParser::default();

        assert!(parser
            .push_chunk("data: {\"choices\":[{\"delta\":{\"content\":\"he")
            .is_empty());
        assert!(parser.push_chunk("llo\"}}]}\n").is_empty());
        let events = parser.push_chunk("\n");

        assert_eq!(events.len(), 1);
        assert!(events[0].contains("hello"));

        assert!(parser.push_chunk("data: tail without delimiter").is_empty());
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
}
