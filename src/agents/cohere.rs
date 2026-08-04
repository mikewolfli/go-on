//! Cohere agent implementation
//!
//! This module provides an implementation for the Cohere API.
//! Uses Cohere's native Chat API format (not OpenAI-compatible).
//!
//! API reference: https://docs.cohere.com/reference/chat

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::{
    apply_openai_common_options, check_api_response, stream_sse_events, SseEventAction,
};
use tracing::warn;

pub struct CohereAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl CohereAgent {
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

    /// Build the Cohere-native request payload.
    ///
    /// Cohere's chat API uses:
    /// - `message`: the latest user message
    /// - `chat_history`: previous messages with roles "USER" or "CHATBOT"
    /// - `preamble`: system instructions
    /// - `model`, `stream`, `temperature`, `max_tokens` at top level
    fn build_payload(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
    ) -> Value {
        // ── System preamble ──────────────────────────────────────────
        let mut preamble = String::new();
        if let Some(text) = crate::agents::principles_to_system_text(principles) {
            preamble.push_str(&text);
        }

        // ── Split messages into chat_history + latest message ─────────
        let mut chat_history: Vec<Value> = Vec::new();
        let mut message_text = String::new();

        if !messages.is_empty() {
            // All messages except the last go into chat_history
            for msg in messages.iter().take(messages.len() - 1) {
                let cohere_role = match msg.role.as_str() {
                    "system" | "user" => "USER",
                    "assistant" => "CHATBOT",
                    other => other,
                };
                chat_history.push(json!({
                    "role": cohere_role,
                    "message": msg.content,
                }));
            }

            // The last message becomes the `message` field
            let last = &messages[messages.len() - 1];

            // If the last message is from the assistant, we still send it as message
            // but we need to record the conversation correctly.
            // Cohere expects `message` to be the new user input.
            // If the last message is assistant, swap roles: use as chat_history
            // and send empty-ish user message.
            if last.role == "assistant" {
                chat_history.push(json!({
                    "role": "CHATBOT",
                    "message": last.content.clone(),
                }));
                message_text = String::new();
            } else {
                message_text = last.content.clone();
            }
        }

        let mut payload = json!({
            "message": message_text,
            "model": self.model,
            "stream": true,
        });

        if !chat_history.is_empty() {
            payload["chat_history"] = json!(chat_history);
        }

        if !preamble.is_empty() {
            payload["preamble"] = json!(preamble);
        }

        apply_openai_common_options(&mut payload, options);

        payload
    }

    /// Parse a Cohere stream event and extract text if present.
    ///
    /// Cohere streaming events contain:
    /// - `{"is_finished": false, "text": "partial token", ...}`
    /// - `{"is_finished": true, "response": {"text": "full text", ...}}`
    fn parse_cohere_event(event: &str) -> anyhow::Result<(SseEventAction, Option<String>)> {
        // Some Cohere deployments may send [DONE] like OpenAI-compatible APIs
        if event.trim() == "[DONE]" {
            return Ok((SseEventAction::Stop, None));
        }

        let value: Value = serde_json::from_str(event)?;

        // Check for finish
        if value.get("is_finished").and_then(|v| v.as_bool()) == Some(true) {
            return Ok((SseEventAction::Stop, None));
        }

        // Extract text field from partial events
        if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
            if !text.is_empty() {
                return Ok((SseEventAction::Continue, Some(text.to_string())));
            }
        }

        Ok((SseEventAction::Continue, None))
    }

    async fn stream_cohere(
        &self,
        response: reqwest::Response,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        stream_sse_events(response, move |data| match Self::parse_cohere_event(data) {
            Ok((action, maybe_text)) => {
                if let Some(text) = maybe_text {
                    if sender.send(text).is_err() {
                        return Ok(SseEventAction::Stop);
                    }
                }
                Ok(action)
            }
            Err(e) => {
                warn!("cohere SSE parse error: {e}");
                Ok(SseEventAction::Continue)
            }
        })
        .await
    }
}

#[async_trait]
impl Agent for CohereAgent {
    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "cohere.api_key_env")?;
        let endpoint = format!("{}/v1/chat", self.base_url.trim_end_matches('/'));
        let payload = self.build_payload(messages, principles, options);

        let response = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        let response = check_api_response(response, "cohere").await?;

        self.stream_cohere(response, sender).await
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "command-a-03-2025".to_string(),
                name: "Command A (03-2025)".to_string(),
                description: "Cohere Command A 03-2025 (latest flagship)".to_string(),
                is_default: self.model == "command-a-03-2025",
                capabilities: vec!["chat".to_string()],
                context_window: Some(256_000),
            },
            ModelInfo {
                id: "command-a-reasoning-08-2025".to_string(),
                name: "Command A Reasoning (08-2025)".to_string(),
                description: "Cohere Command A Reasoning 08-2025 (reasoning variant)".to_string(),
                is_default: self.model == "command-a-reasoning-08-2025",
                capabilities: vec!["chat".to_string(), "reasoning".to_string()],
                context_window: Some(256_000),
            },
            ModelInfo {
                id: "command-r7b-12-2024".to_string(),
                name: "Command R7B (12-2024)".to_string(),
                description: "Cohere Command R7B 12-2024".to_string(),
                is_default: self.model == "command-r7b-12-2024",
                capabilities: vec!["chat".to_string()],
                context_window: Some(131072),
            },
            ModelInfo {
                id: "command-r-plus-08-2024".to_string(),
                name: "Command R+ 08-2024".to_string(),
                description: "Cohere Command R+ 08-2024".to_string(),
                is_default: self.model == "command-r-plus-08-2024",
                capabilities: vec!["chat".to_string()],
                context_window: Some(131072),
            },
            ModelInfo {
                id: "command-r-08-2024".to_string(),
                name: "Command R 08-2024".to_string(),
                description: "Cohere Command R 08-2024".to_string(),
                is_default: self.model == "command-r-08-2024",
                capabilities: vec!["chat".to_string()],
                context_window: Some(131072),
            },
        ]
    }
}
