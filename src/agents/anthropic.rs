//! Anthropic agent implementation
//!
//! This module provides an implementation for the Anthropic Claude API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::warn;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message};
use crate::agents::{
    check_api_response, option_f64, option_u64, principles_to_system_text, resolve_effective_model,
    stream_sse_events, SseEventAction,
};

/// Anthropic Claude agent
pub struct AnthropicAgent {
    /// Base URL for the Anthropic API
    base_url: String,
    /// Environment variable for API key
    api_key_env: String,
    /// Model name
    model: String,
    /// Anthropic API version
    anthropic_version: String,
    /// Maximum tokens for response
    max_tokens: u32,
    /// HTTP client
    client: reqwest::Client,
}

static ANTHROPIC_SSE_PARSE_ERROR_TOTAL: AtomicU64 = AtomicU64::new(0);

impl AnthropicAgent {
    fn normalize_thinking_option(options: &Option<HashMap<String, Value>>) -> Option<Value> {
        let thinking = options.as_ref().and_then(|map| map.get("thinking"))?;
        if thinking.is_object() {
            return Some(thinking.clone());
        }

        // Anthropic messages API expects object-style thinking.
        if let Some(mode) = thinking.as_str() {
            let normalized = match mode {
                "enabled" | "on" | "true" => json!({"type": "enabled"}),
                "disabled" | "off" | "false" => json!({"type": "disabled"}),
                // Reject unknown free-form values to avoid invalid API payloads.
                _ => return None,
            };
            return Some(normalized);
        }

        None
    }

    fn normalize_tool_choice_option(options: &Option<HashMap<String, Value>>) -> Option<Value> {
        let tool_choice = options.as_ref().and_then(|map| map.get("tool_choice"))?;
        if tool_choice.is_object() {
            return Some(tool_choice.clone());
        }

        // Anthropic tool_choice canonical shape is object-based.
        if let Some(mode) = tool_choice.as_str() {
            if let Some(tool_name) = mode.strip_prefix("tool:") {
                let name = tool_name.trim();
                if !name.is_empty() {
                    return Some(json!({"type": "tool", "name": name}));
                }
                return None;
            }

            return match mode {
                "auto" | "any" | "none" => Some(json!({"type": mode})),
                "tool" => None,
                _ => None,
            };
        }

        None
    }

    /// Create a new Anthropic agent
    ///
    /// # Arguments
    /// * `base_url` - Base URL for the Anthropic API
    /// * `api_key_env` - Environment variable for API key
    /// * `model` - Model name
    /// * `anthropic_version` - Anthropic API version
    /// * `max_tokens` - Maximum tokens for response
    /// * `client` - HTTP client
    ///
    /// # Returns
    /// * `Self` - New Anthropic agent instance
    pub fn new(
        base_url: String,
        api_key_env: String,
        model: String,
        anthropic_version: String,
        max_tokens: u32,
        client: reqwest::Client,
    ) -> Self {
        Self {
            base_url,
            api_key_env,
            model,
            anthropic_version,
            max_tokens,
            client,
        }
    }

    /// Build the content value for a message, supporting both plain text and content blocks.
    ///
    /// If `content` starts with `[`, it is treated as a JSON array of content blocks
    /// (e.g. image + text). Otherwise, it is used as a plain text string.
    ///
    /// # Arguments
    /// * `content` - The message content string
    ///
    /// # Returns
    /// * `Value` - The content value (either a string or an array of content blocks)
    fn build_message_content(content: &str) -> Value {
        let trimmed = content.trim();
        if trimmed.starts_with('[') {
            match serde_json::from_str::<Value>(trimmed) {
                Ok(Value::Array(arr)) => Value::Array(arr),
                _ => Value::String(content.to_string()),
            }
        } else {
            Value::String(content.to_string())
        }
    }

    /// Convert messages and options to Anthropic API payload
    ///
    /// # Arguments
    /// * `messages` - List of messages
    /// * `principles` - Optional list of principles
    /// * `options` - Optional HashMap of options
    ///
    /// # Returns
    /// * `Value` - Anthropic API payload
    fn to_anthropic_payload(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        tools: Option<Vec<Value>>,
    ) -> Value {
        let mut system_parts: Vec<String> = Vec::new();
        if let Some(text) = principles_to_system_text(principles) {
            system_parts.push(text);
        }

        let mut out_messages: Vec<Value> = Vec::new();
        for m in messages.iter() {
            if m.role.eq_ignore_ascii_case("system") {
                system_parts.push(m.content.clone());
                continue;
            }

            let role = if m.role.eq_ignore_ascii_case("assistant") {
                "assistant"
            } else {
                "user"
            };

            let content = Self::build_message_content(&m.content);

            out_messages.push(json!({
                "role": role,
                "content": content
            }));
        }

        let model = resolve_effective_model(&self.model, options, &self.available_models());
        let max_tokens = option_u64(options, "max_tokens")
            // Clamp to the u32 domain: a raw `as u32` silently truncates
            // (2^32+100 → 100) and sends a wrong provider request.
            .map(|v| v.min(u64::from(u32::MAX)) as u32)
            .unwrap_or(self.max_tokens);

        let temperature = option_f64(options, "temperature");
        let top_p = option_f64(options, "top_p");
        let top_k = option_u64(options, "top_k");

        let mut payload = json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": out_messages,
            "stream": true
        });

        if !system_parts.is_empty() {
            payload["system"] = Value::String(system_parts.join("\n\n"));
        }

        if let Some(temp) = temperature {
            payload["temperature"] = Value::from(temp);
        }

        if let Some(value) = top_p {
            payload["top_p"] = Value::from(value);
        }

        if let Some(value) = top_k {
            payload["top_k"] = Value::from(value);
        }

        // Forward thinking parameter as object style for Anthropic API.
        if let Some(thinking) = Self::normalize_thinking_option(options) {
            payload["thinking"] = thinking;
        }

        // Forward metadata parameter if present (e.g. user_id)
        if let Some(metadata) = options
            .as_ref()
            .and_then(|map| map.get("metadata"))
            .and_then(|v| v.as_object())
        {
            payload["metadata"] = Value::Object(metadata.clone());
        }

        // Forward stop_sequences parameter if present
        if let Some(stop_sequences) = options
            .as_ref()
            .and_then(|map| map.get("stop_sequences"))
            .and_then(|v| v.as_array())
        {
            payload["stop_sequences"] = Value::Array(stop_sequences.clone());
        }

        // Forward tools parameter if present (from options, higher priority)
        if let Some(tool_opts) = options
            .as_ref()
            .and_then(|map| map.get("tools"))
            .and_then(|v| v.as_array())
        {
            payload["tools"] = Value::Array(tool_opts.clone());
        } else if let Some(tool_defs) = tools {
            // Use native tool definitions passed directly
            if !tool_defs.is_empty() {
                payload["tools"] = Value::Array(tool_defs);
            }
        }

        // Forward tool_choice parameter as object style.
        if let Some(tool_choice) = Self::normalize_tool_choice_option(options) {
            payload["tool_choice"] = tool_choice;
        } else if payload.get("tools").is_some() && payload.get("tool_choice").is_none() {
            // Default to auto tool_choice when tools are present
            payload["tool_choice"] = json!({"type": "auto"});
        }

        payload
    }

    /// Stream SSE events from Anthropic API response
    ///
    /// # Arguments
    /// * `response` - HTTP response from Anthropic API
    /// * `sender` - Unbounded sender for streaming responses
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) if streaming completes successfully, or an error if something goes wrong
    async fn stream_sse(
        &self,
        response: reqwest::Response,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        stream_sse_events(response, Self::sse_event_handler(sender)).await
    }

    /// Stream SSE events with gzip decompression, using the same native
    /// Anthropic event parsing as [`Self::stream_sse`]. The shared
    /// OpenAI-shaped token extractor cannot parse Anthropic's
    /// `content_block_delta` / `input_json_delta` events, so the compressed
    /// path must reuse this handler instead of `stream_sse_to_sender`.
    async fn stream_sse_compressed(
        &self,
        response: reqwest::Response,
        sender: crate::agent::StreamingSender,
        config: &crate::agents::StreamingConfig,
    ) -> anyhow::Result<()> {
        crate::agents::stream_sse_with_handler(response, config, Self::sse_event_handler(sender))
            .await
    }

    /// Build the Anthropic SSE event handler: accumulates native `tool_use`
    /// input deltas and forwards `text_delta` chunks to the sender. Shared by
    /// the plain and decompressed streaming paths.
    fn sse_event_handler(
        sender: crate::agent::StreamingSender,
    ) -> impl FnMut(&str) -> anyhow::Result<SseEventAction> {
        let mut current_tool_name: Option<String> = None;
        let mut accumulated_args: String = String::new();
        move |data: &str| {
            let value = match serde_json::from_str::<Value>(data) {
                Ok(v) => v,
                Err(e) => {
                    let total = ANTHROPIC_SSE_PARSE_ERROR_TOTAL.fetch_add(1, Ordering::Relaxed) + 1;
                    warn!(
                        error = %e,
                        parse_error_total = total,
                        data_preview = %crate::shared::truncate::truncate_chars(data, 160, "..."),
                        "anthropic SSE frame parse failed; continue streaming"
                    );
                    return Ok(SseEventAction::Continue);
                }
            };
            let event_type = value.get("type").and_then(|v| v.as_str());

            // Detect tool_use start (Anthropic native function calling)
            if event_type == Some("content_block_start") {
                if let Some(block) = value.get("content_block") {
                    if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        current_tool_name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        accumulated_args.clear();
                        // Accumulate initial input if present
                        if let Some(input) = block
                            .get("input")
                            .and_then(|v| serde_json::to_string(v).ok())
                        {
                            if input != "{}" {
                                accumulated_args.push_str(&input);
                            }
                        }
                        return Ok(SseEventAction::Continue);
                    }
                }
                // A new non-tool_use content block means any previous tool_use is done
                if let Some(ref name) = current_tool_name.take() {
                    let token = crate::orchestration::autonomy_runtime::build_tool_call_token(
                        name,
                        &accumulated_args,
                    );
                    if sender.send(token).is_err() {
                        return Ok(SseEventAction::Stop);
                    }
                }
                accumulated_args.clear();
            }

            // Accumulate input_json_delta chunks
            if event_type == Some("content_block_delta") {
                if let Some(delta) = value.get("delta") {
                    if delta.get("type").and_then(|v| v.as_str()) == Some("input_json_delta") {
                        if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            accumulated_args.push_str(partial);
                        }
                        return Ok(SseEventAction::Continue);
                    }
                }
            }

            // On content_block_stop or message_stop, finalize any pending tool_use
            if event_type == Some("content_block_stop") || event_type == Some("message_stop") {
                if let Some(ref name) = current_tool_name.take() {
                    let token = crate::orchestration::autonomy_runtime::build_tool_call_token(
                        name,
                        &accumulated_args,
                    );
                    if sender.send(token).is_err() {
                        return Ok(SseEventAction::Stop);
                    }
                    accumulated_args.clear();
                }
                if event_type == Some("message_stop") {
                    return Ok(SseEventAction::Stop);
                }
                return Ok(SseEventAction::Continue);
            }

            // Flush any pending tool_use before processing text content
            if let Some(ref name) = current_tool_name.take() {
                let token = crate::orchestration::autonomy_runtime::build_tool_call_token(
                    name,
                    &accumulated_args,
                );
                if sender.send(token).is_err() {
                    return Ok(SseEventAction::Stop);
                }
                accumulated_args.clear();
            }

            // Standard delta text extraction
            if let Some(token) = value
                .get("delta")
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
            {
                if sender.send(token.to_string()).is_err() {
                    return Ok(SseEventAction::Stop);
                }
            }

            Ok(SseEventAction::Continue)
        }
    }
}

/// Parse Anthropic SSE event
///
/// # Arguments
/// * `data` - SSE event data
///
/// # Returns
/// * `Result<(SseEventAction, Option<String>)>` - Returns `Ok((SseEventAction, Option<String>))` with the action and optional token, or an error if parsing fails
#[cfg(test)]
fn parse_anthropic_event(data: &str) -> anyhow::Result<(SseEventAction, Option<String>)> {
    if data.trim() == "[DONE]" {
        return Ok((SseEventAction::Stop, None));
    }

    let value = serde_json::from_str::<Value>(data)?;
    if value.get("type").and_then(|v| v.as_str()) == Some("message_stop") {
        return Ok((SseEventAction::Stop, None));
    }

    let token = value
        .get("delta")
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .map(|text| text.to_string());

    Ok((SseEventAction::Continue, token))
}

#[async_trait]
impl Agent for AnthropicAgent {
    /// Build a single chat attempt (no retry).
    ///
    /// SSE compression is applied when `options["sse_compress"]` is set.
    /// Both paths use the Anthropic-specific SSE parser (which accumulates
    /// native tool_use input deltas); the compressed path additionally
    /// gzip-decompresses the stream first.
    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "claude.api_key_env")?;

        let payload = self.to_anthropic_payload(messages, principles, options, None);
        let endpoint = crate::shared::url_join::join_url(&self.base_url, "v1/messages");

        let response = self
            .client
            .post(endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", &self.anthropic_version)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await?;

        let response = check_api_response(response, "claude").await?;

        // SSE compression is applied when `options["sse_compress"]` is set; the
        // non-compressed path uses the Anthropic-specific SSE parser (which
        // accumulates native tool_use input deltas). The compressed path
        // decompresses the stream and feeds it through the same native parser
        // (the shared OpenAI-shaped extractor cannot parse Anthropic events).
        let cfg = crate::agents::streaming_config(options, false);
        if cfg.enable_compression {
            self.stream_sse_compressed(response, sender, &cfg).await
        } else {
            self.stream_sse(response, sender).await
        }
    }

    fn available_models(&self) -> Vec<crate::agent::ModelInfo> {
        let mut models = vec![
            crate::agent::ModelInfo {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4 (20250514)".to_string(),
                description: "Anthropic's best combination of speed and intelligence".to_string(),
                is_default: self.model == "claude-sonnet-4-20250514",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "reasoning".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                    "extended_thinking".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            crate::agent::ModelInfo {
                id: "claude-opus-4-7".to_string(),
                name: "Claude Opus 4.7".to_string(),
                description:
                    "Anthropic's most capable model for complex reasoning and agentic coding"
                        .to_string(),
                is_default: self.model == "claude-opus-4-7",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "reasoning".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                    "extended_thinking".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            crate::agent::ModelInfo {
                id: "claude-haiku-4-5-20251001".to_string(),
                name: "Claude Haiku 4.5 (20251001)".to_string(),
                description: "Anthropic's fastest model with near-frontier intelligence"
                    .to_string(),
                is_default: self.model == "claude-haiku-4-5-20251001",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "reasoning".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                    "extended_thinking".to_string(),
                ],
                context_window: Some(200_000),
            },
            crate::agent::ModelInfo {
                id: "claude-3-5-sonnet".to_string(),
                name: "Claude 3.5 Sonnet".to_string(),
                description: "Anthropic Claude 3.5 Sonnet".to_string(),
                is_default: self.model == "claude-3-5-sonnet",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(200_000),
            },
            crate::agent::ModelInfo {
                id: "claude-3-opus".to_string(),
                name: "Claude 3 Opus".to_string(),
                description: "Anthropic Claude 3 Opus (DEPRECATED — use claude-opus-4-7)"
                    .to_string(),
                is_default: self.model == "claude-3-opus",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(200_000),
            },
            crate::agent::ModelInfo {
                id: "claude-3-haiku".to_string(),
                name: "Claude 3 Haiku".to_string(),
                description: "Anthropic Claude 3 Haiku (DEPRECATED — use claude-haiku-4-5)"
                    .to_string(),
                is_default: self.model == "claude-3-haiku",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(200_000),
            },
        ];

        // Keep runtime resilient to newly released model IDs configured by users.
        if !self.model.is_empty() && !models.iter().any(|m| m.id == self.model) {
            models.insert(
                0,
                crate::agent::ModelInfo {
                    id: self.model.clone(),
                    name: self.model.clone(),
                    description: "Configured Anthropic model".to_string(),
                    is_default: true,
                    capabilities: vec![
                        "chat".to_string(),
                        "vision".to_string(),
                        "function_calling".to_string(),
                        "streaming".to_string(),
                    ],
                    context_window: None,
                },
            );
        }

        // Single source of truth: the provider spec's `model_suggestions` feed
        // the GUI's model dropdown, so every suggestion must also be listable
        // here. Hand-curated entries above keep their capability/context
        // enrichment; suggestions with no curated entry get a default shape.
        if let Some(spec) = crate::core::providers::provider_spec_by_name("anthropic") {
            for id in &spec.model_suggestions {
                if !models.iter().any(|m| &m.id == id) {
                    models.push(crate::shared::default_model_info(id, self.model == *id));
                }
            }
        }

        models
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    fn agent() -> AnthropicAgent {
        AnthropicAgent::new(
            "https://api.anthropic.com".to_string(),
            "ANTHROPIC_API_KEY".to_string(),
            "claude-3-7-sonnet-latest".to_string(),
            "2023-06-01".to_string(),
            4096,
            reqwest::Client::new(),
        )
    }

    #[test]
    fn to_anthropic_payload_merges_system_content_and_options() {
        let payload = agent().to_anthropic_payload(
            &[
                message("system", "existing system"),
                message("user", "hello"),
            ],
            &Some(vec!["Prefer tests".to_string()]),
            &Some(HashMap::from([
                ("model".to_string(), json!("claude-custom")),
                ("max_tokens".to_string(), json!(2048)),
                ("temperature".to_string(), json!(0.2)),
            ])),
            None,
        );

        let system = payload["system"]
            .as_str()
            .expect("system field should be a string");
        assert!(system.contains("Prefer tests"));
        assert!(system.contains("existing system"));
        assert_eq!(payload["messages"][0]["role"], "user");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert_eq!(payload["model"], "claude-custom");
        assert_eq!(payload["max_tokens"], 2048);
        assert_eq!(payload["temperature"], 0.2);
    }

    #[test]
    fn parse_anthropic_event_extracts_delta_text() {
        let (action, token) =
            parse_anthropic_event(r#"{"type":"content_block_delta","delta":{"text":"hello"}}"#)
                .expect("anthropic event should parse");

        assert!(matches!(action, SseEventAction::Continue));
        assert_eq!(token.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_anthropic_event_stops_on_done_and_message_stop() {
        let (done_action, done_token) = parse_anthropic_event("[DONE]").expect("done should parse");
        assert!(matches!(done_action, SseEventAction::Stop));
        assert!(done_token.is_none());

        let (stop_action, stop_token) =
            parse_anthropic_event(r#"{"type":"message_stop"}"#).expect("message_stop should parse");
        assert!(matches!(stop_action, SseEventAction::Stop));
        assert!(stop_token.is_none());
    }

    /// The compressed streaming path must produce the same tokens as the plain
    /// path. Regression test for the bug where `sse_compress: true` routed
    /// Anthropic events through the shared OpenAI-shaped extractor, which
    /// cannot parse `content_block_delta` and produced empty output.
    #[tokio::test]
    async fn sse_event_handler_extracts_text_and_tool_use_from_native_events() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let sender = crate::agent::StreamingSender::from(tx);
        let mut handler = AnthropicAgent::sse_event_handler(sender);

        // text delta (the event shape the shared extractor would miss)
        let action = handler(
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}"#,
        )
        .expect("handler should accept text delta");
        assert!(matches!(action, SseEventAction::Continue));

        // tool_use start + input_json_delta accumulation + stop
        handler(r#"{"type":"content_block_start","content_block":{"type":"tool_use","id":"t1","name":"read_file","input":{}}}"#)
            .expect("tool_use start should parse");
        handler(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}}"#)
            .expect("input_json_delta should parse");
        let action = handler(r#"{"type":"content_block_stop"}"#).expect("stop should parse");
        assert!(matches!(action, SseEventAction::Continue));

        let action = handler(r#"{"type":"message_stop"}"#).expect("message_stop should parse");
        assert!(matches!(action, SseEventAction::Stop));

        let mut tokens = Vec::new();
        while let Ok(tok) = rx.try_recv() {
            tokens.push(tok);
        }
        assert!(
            tokens.iter().any(|t| t == "Hello"),
            "text delta must reach the sender, got: {tokens:?}"
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.contains("read_file") && t.contains("path")),
            "accumulated tool_use input must reach the sender, got: {tokens:?}"
        );
    }

    #[test]
    fn normalize_tool_choice_string_to_object_shape() {
        let mut options = HashMap::new();
        options.insert("tool_choice".to_string(), json!("auto"));

        let normalized = AnthropicAgent::normalize_tool_choice_option(&Some(options))
            .expect("normalized tool_choice");

        assert_eq!(normalized["type"], "auto");
    }

    #[test]
    fn normalize_thinking_string_to_object_shape() {
        let mut options = HashMap::new();
        options.insert("thinking".to_string(), json!("enabled"));

        let normalized =
            AnthropicAgent::normalize_thinking_option(&Some(options)).expect("normalized thinking");

        assert_eq!(normalized["type"], "enabled");
    }

    #[test]
    fn normalize_thinking_rejects_unknown_string_mode() {
        let mut options = HashMap::new();
        options.insert("thinking".to_string(), json!("custom"));

        assert!(AnthropicAgent::normalize_thinking_option(&Some(options)).is_none());
    }

    #[test]
    fn normalize_tool_choice_supports_tool_prefixed_name() {
        let mut options = HashMap::new();
        options.insert("tool_choice".to_string(), json!("tool:search_docs"));

        let normalized = AnthropicAgent::normalize_tool_choice_option(&Some(options))
            .expect("normalized tool choice");

        assert_eq!(normalized["type"], "tool");
        assert_eq!(normalized["name"], "search_docs");
    }
}
