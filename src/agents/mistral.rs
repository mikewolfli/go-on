//! Mistral AI agent implementation
//!
//! This module provides an implementation for the Mistral AI API.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, is_non_retryable_4xx, request_failed_msg};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct MistralAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl MistralAgent {
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
        let mut final_messages: Vec<Message> = Vec::new();
        let mut system_text = String::new();

        if let Some(items) = principles {
            if !items.is_empty() {
                system_text.push_str(&principles_to_text(&items));
                system_text.push('\n');
            }
        }

        if !system_text.is_empty() {
            final_messages.push(Message {
                role: "system".to_string(),
                content: system_text,
            });
        }
        final_messages.extend(messages.iter().cloned());

        let mut payload = json!({
            "model": self.model,
            "messages": final_messages,
            "stream": true
        });

        apply_openai_common_options(&mut payload, options);

        payload
    }

    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "mistral.api_key_env")?;
        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let payload = self.build_payload(messages, principles, options);

        let response = self
            .client
            .post(endpoint)
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
                chat_request_failed_msg("mistral", &status.to_string(), &body)
            );
        }

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("mistral: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for MistralAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "mistral-large-2512".to_string(),
                name: "Mistral Large 3 (25.12)".to_string(),
                description: "Mistral Large 3 (v25.12, most capable)".to_string(),
                is_default: self.model == "mistral-large-2512",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "mistral-medium-2508".to_string(),
                name: "Mistral Medium 3.1 (25.08)".to_string(),
                description: "Mistral Medium 3.1 (v25.08, balanced)".to_string(),
                is_default: self.model == "mistral-medium-2508",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(32_000),
            },
            ModelInfo {
                id: "mistral-small-2603".to_string(),
                name: "Mistral Small 4 (26.03)".to_string(),
                description: "Mistral Small 4 (v26.03, fast & cost-efficient)".to_string(),
                is_default: self.model == "mistral-small-2603",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(32_000),
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
        let mut last_error: Option<anyhow::Error> = None;
        let chat_messages = messages;

        for attempt in 0..=2 {
            match self
                .chat_once(&chat_messages, &principles, &options, sender.clone())
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    let err_msg = err.to_string();
                    if is_non_retryable_4xx(&err_msg) {
                        return Err(err.into());
                    }
                    last_error = Some(err);
                    if attempt < 2 {
                        sleep(Duration::from_secs(1_u64 << attempt)).await;
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("mistral")))
            .into())
    }
}
