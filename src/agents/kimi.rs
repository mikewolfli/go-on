//! Kimi (月之暗面) agent implementation
//!
//! This module provides an implementation for the Kimi AI API from 月之暗面 (Moonshot AI).
//! The Kimi API is OpenAI-compatible and hosted at https://api.moonshot.cn/v1.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, is_non_retryable_4xx, request_failed_msg};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct KimiAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl KimiAgent {
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
                system_text.push_str(&principles_to_text(items));
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

        // Enable thinking mode for kimi-k2.6 (supports the `thinking` parameter)
        if self.model == "kimi-k2.6" {
            payload["thinking"] = json!({
                "budget_tokens": 4096
            });
        }

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
        let api_key = resolve_secret(&self.api_key_env, "kimi.api_key_env")?;
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
                chat_request_failed_msg("kimi", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for KimiAgent {
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("kimi")))
            .into())
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "kimi-k2.6".to_string(),
                name: "Kimi K2.6".to_string(),
                description: "Kimi K2.6 with 128K context window, supports thinking mode"
                    .to_string(),
                is_default: self.model == "kimi-k2.6",
                capabilities: vec!["chat".to_string(), "thinking".to_string()],
                context_window: Some(128000),
            },
            ModelInfo {
                id: "kimi-k2.5".to_string(),
                name: "Kimi K2.5".to_string(),
                description: "Kimi K2.5 with 128K context window".to_string(),
                is_default: self.model == "kimi-k2.5",
                capabilities: vec!["chat".to_string()],
                context_window: Some(128000),
            },
            ModelInfo {
                id: "kimi-k2".to_string(),
                name: "Kimi K2".to_string(),
                description: "Kimi K2 with 128K context window".to_string(),
                is_default: self.model == "kimi-k2",
                capabilities: vec!["chat".to_string()],
                context_window: Some(128000),
            },
            ModelInfo {
                id: "kimi-k2-thinking".to_string(),
                name: "Kimi K2 Thinking".to_string(),
                description: "Kimi K2 Thinking with 128K context window".to_string(),
                is_default: self.model == "kimi-k2-thinking",
                capabilities: vec!["chat".to_string(), "thinking".to_string()],
                context_window: Some(128000),
            },
            ModelInfo {
                id: "moonshot-v1".to_string(),
                name: "Moonshot v1".to_string(),
                description: "Moonshot v1 with 128K context window".to_string(),
                is_default: self.model == "moonshot-v1",
                capabilities: vec!["chat".to_string()],
                context_window: Some(128000),
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
