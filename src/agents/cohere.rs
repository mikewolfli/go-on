//! Cohere agent implementation
//!
//! This module provides an implementation for the Cohere API.
//! Uses Cohere's native Chat API format (not OpenAI-compatible).
//!
//! API reference: https://docs.cohere.com/reference/chat

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, request_failed_msg};
use crate::agents::{
    apply_openai_common_options, principles_to_text, stream_sse_events, SseEventAction,
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
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
    ) -> Value {
        // ── System preamble ──────────────────────────────────────────
        let mut preamble = String::new();
        if let Some(items) = principles {
            if !items.is_empty() {
                preamble.push_str(&principles_to_text(&items));
            }
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

    async fn chat_once(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "cohere.api_key_env")?;
        let endpoint = format!("{}/v1/chat", self.base_url.trim_end_matches('/'));
        let payload = self.build_payload(messages, principles, &options);

        let response = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("cohere", &status.to_string(), &body)
            );
        }

        self.stream_cohere(response, sender).await
    }
}

#[async_trait]
impl Agent for CohereAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> crate::core::error::Result<()> {
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..=2 {
            match self
                .chat_once(
                    messages.clone(),
                    principles.clone(),
                    options.clone(),
                    sender.clone(),
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = Some(err);
                    if attempt < 2 {
                        sleep(Duration::from_secs(1_u64 << attempt)).await;
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("cohere")))
            .into())
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
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
            ModelInfo {
                id: "command-light".to_string(),
                name: "Command Light".to_string(),
                description: "Cohere Command Light".to_string(),
                is_default: self.model == "command-light",
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            },
        ]
    }

    fn default_model(&self) -> Option<ModelInfo> {
        self.available_models().into_iter().find(|m| m.is_default)
    }

    fn supports_model_override(&self) -> bool {
        true
    }
}
