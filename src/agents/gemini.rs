//! Google Gemini agent implementation
//!
//! This module provides an implementation for the Google Gemini API.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, retry_chat_once};
use crate::agents::{option_f64, principles_to_text, stream_sse_events, SseEventAction};

pub struct GeminiAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl GeminiAgent {
    pub fn new(
        api_key_env: String,
        base_url: String,
        model: String,
        client: reqwest::Client,
    ) -> Self {
        Self {
            api_key_env,
            base_url,
            model,
            client,
        }
    }

    fn build_payload(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
    ) -> Value {
        let mut contents: Vec<Value> = Vec::new();
        let mut system_instruction: Option<String> = None;

        if let Some(items) = principles {
            if !items.is_empty() {
                let system_text = principles_to_text(items);
                system_instruction = Some(system_text);
            }
        }

        for message in messages.iter() {
            if message.role == "system" {
                system_instruction = Some(message.content.clone());
            } else {
                // Map "assistant" to Gemini's "model" role
                let role = if message.role == "assistant" {
                    "model"
                } else {
                    &message.role
                };

                // Support both plain text and structured parts (for vision/inline_data)
                let parts = if let Ok(parsed) = serde_json::from_str::<Value>(&message.content) {
                    if parsed.is_array() {
                        // Content is a JSON array of parts (e.g., for vision requests)
                        parsed.as_array().cloned().unwrap_or_default()
                    } else {
                        vec![json!({"text": message.content})]
                    }
                } else {
                    vec![json!({"text": message.content})]
                };

                contents.push(json!({
                    "role": role,
                    "parts": parts
                }));
            }
        }

        let mut payload = json!({
            "contents": contents
        });

        if let Some(system_text) = system_instruction {
            payload["system_instruction"] = json!({
                "parts": [{"text": system_text}]
            });
        }

        let mut generation_config = json!({});
        if let Some(value) = option_f64(options, "temperature") {
            generation_config["temperature"] = Value::from(value);
        }
        if let Some(value) = option_f64(options, "top_p") {
            generation_config["top_p"] = Value::from(value);
        }
        if let Some(value) = option_f64(options, "max_output_tokens") {
            generation_config["max_output_tokens"] = Value::from(value);
        }
        if let Some(value) = options.as_ref().and_then(|o| o.get("max_tokens")) {
            if let Some(n) = value.as_u64() {
                generation_config["max_output_tokens"] = Value::from(n);
            }
        }
        if generation_config.as_object().is_some_and(|o| !o.is_empty()) {
            payload["generationConfig"] = generation_config;
        }

        // Forward tools parameter if present (Gemini "tools" array of Tool objects)
        if let Some(tools) = options
            .as_ref()
            .and_then(|map| map.get("tools"))
            .and_then(|v| v.as_array())
        {
            payload["tools"] = Value::Array(tools.clone());
        }

        // Forward tool_config parameter if present
        if let Some(tool_config) = options.as_ref().and_then(|map| map.get("tool_config")) {
            if tool_config.is_object() {
                payload["tool_config"] = tool_config.clone();
            }
        }

        // Forward safety_settings parameter if present
        if let Some(safety_settings) = options.as_ref().and_then(|map| map.get("safety_settings")) {
            if safety_settings.is_array() {
                payload["safety_settings"] = safety_settings.clone();
            }
        }

        payload
    }

    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "gemini.api_key_env")?;
        let endpoint = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url.trim_end_matches('/'),
            self.model,
        );
        let payload = self.build_payload(messages, principles, options);

        let response = self
            .client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("x-goog-api-key", &api_key)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("gemini", &status.to_string(), &body)
            );
        }

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("gemini: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
        }

        // Parse Gemini streaming response which uses `candidates[0].content.parts[*]`
        // format. Each part may contain `text` for plain output or `functionCall`
        // for native function calling.
        stream_sse_events(response, move |data| {
            if data.trim() == "[DONE]" {
                return Ok(SseEventAction::Stop);
            }

            if let Ok(json) = serde_json::from_str::<Value>(data) {
                if let Some(candidate) = json.get("candidates").and_then(|c| c.get(0)) {
                    // Check finishReason for SAFETY/RECITATION blocks
                    if let Some(finish_reason) =
                        candidate.get("finishReason").and_then(|v| v.as_str())
                    {
                        if finish_reason == "SAFETY" || finish_reason == "RECITATION" {
                            let token = format!("[Blocked by Gemini: {}]", finish_reason);
                            if sender.send(token).is_err() {
                                return Ok(SseEventAction::Stop);
                            }
                        }
                    }

                    // Iterate through all parts to extract text AND functionCall
                    if let Some(parts) = candidate
                        .get("content")
                        .and_then(|c| c.get("parts"))
                        .and_then(|p| p.as_array())
                    {
                        for part in parts {
                            // Text content
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                if sender.send(text.to_string()).is_err() {
                                    return Ok(SseEventAction::Stop);
                                }
                            }

                            // Native function call (Gemini functionCall part)
                            if let Some(func_call) = part.get("functionCall") {
                                if let Some(name) = func_call.get("name").and_then(|v| v.as_str()) {
                                    let args = func_call
                                        .get("args")
                                        .map(|v| v.to_string())
                                        .unwrap_or_else(|| "{}".to_string());
                                    let token = crate::orchestration::
                                        autonomy_runtime::build_tool_call_token(
                                        name, &args,
                                    );
                                    if sender.send(token).is_err() {
                                        return Ok(SseEventAction::Stop);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Ok(SseEventAction::Continue)
        })
        .await
    }
}

#[async_trait]
impl Agent for GeminiAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "gemini-2.5-flash".to_string(),
                name: "Gemini 2.5 Flash".to_string(),
                description: "Google Gemini 2.5 Flash (latest, fast & efficient)".to_string(),
                is_default: self.model == "gemini-2.5-flash",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(1_048_576),
            },
            ModelInfo {
                id: "gemini-2.5-flash-lite".to_string(),
                name: "Gemini 2.5 Flash Lite".to_string(),
                description: "Google Gemini 2.5 Flash Lite (fast & cost-efficient)".to_string(),
                is_default: self.model == "gemini-2.5-flash-lite",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(1_048_576),
            },
            ModelInfo {
                id: "gemini-2.5-pro".to_string(),
                name: "Gemini 2.5 Pro".to_string(),
                description: "Google Gemini 2.5 Pro (latest, most capable)".to_string(),
                is_default: self.model == "gemini-2.5-pro",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(1_048_576),
            },
            ModelInfo {
                id: "gemini-3.1-pro-preview-03-2026".to_string(),
                name: "Gemini 3.1 Pro Preview".to_string(),
                description: "Google Gemini 3.1 Pro Preview (03-2026, preview)".to_string(),
                is_default: self.model == "gemini-3.1-pro-preview-03-2026",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(1_048_576),
            },
            ModelInfo {
                id: "gemini-3-flash-preview-03-2026".to_string(),
                name: "Gemini 3 Flash Preview".to_string(),
                description: "Google Gemini 3 Flash Preview (03-2026, preview)".to_string(),
                is_default: self.model == "gemini-3-flash-preview-03-2026",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(1_048_576),
            },
            ModelInfo {
                id: "gemini-2.0-flash".to_string(),
                name: "Gemini 2.0 Flash".to_string(),
                description: "Google Gemini 2.0 Flash (DEPRECATED — shutting down soon)"
                    .to_string(),
                is_default: self.model == "gemini-2.0-flash",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(1_048_576),
            },
            ModelInfo {
                id: "gemini-2.0-flash-lite-preview-02-2025".to_string(),
                name: "Gemini 2.0 Flash Lite".to_string(),
                description: "Google Gemini 2.0 Flash Lite (DEPRECATED — shutting down)"
                    .to_string(),
                is_default: self.model == "gemini-2.0-flash-lite-preview-02-2025",
                capabilities: vec![
                    "chat".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(1_048_576),
            },
            ModelInfo {
                id: "gemini-2.0-pro".to_string(),
                name: "Gemini 2.0 Pro".to_string(),
                description: "Google Gemini 2.0 Pro (DEPRECATED)".to_string(),
                is_default: self.model == "gemini-2.0-pro",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
                context_window: Some(1_048_576),
            },
        ]
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> crate::core::error::Result<()> {
        retry_chat_once(
            || async {
                self.chat_once(&messages, &principles, &options, sender.clone())
                    .await
                    .map_err(Into::into)
            },
            3,
        )
        .await
    }
}
