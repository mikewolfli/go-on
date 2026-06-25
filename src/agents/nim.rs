//! NVIDIA NIM agent implementation
//!
//! This module provides an implementation for the NVIDIA NIM API.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, is_non_retryable_4xx, request_failed_msg};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct NimAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl NimAgent {
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
        let api_key = resolve_secret(&self.api_key_env, "nim.api_key_env")?;
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
                chat_request_failed_msg("nim", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for NimAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "meta/llama-3.1-70b-instruct".to_string(),
                name: "Meta Llama 3.1 70B Instruct".to_string(),
                description: "Meta Llama 3.1 70B Instruct (128K context)".to_string(),
                is_default: self.model == "meta/llama-3.1-70b-instruct",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "meta/llama-3.1-405b-instruct".to_string(),
                name: "Meta Llama 3.1 405B Instruct".to_string(),
                description: "Meta Llama 3.1 405B Instruct (128K context)".to_string(),
                is_default: self.model == "meta/llama-3.1-405b-instruct",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "mistralai/mixtral-8x22b-instruct".to_string(),
                name: "Mixtral 8x22B Instruct".to_string(),
                description: "Mixtral 8x22B Instruct (64K context)".to_string(),
                is_default: self.model == "mistralai/mixtral-8x22b-instruct",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(64_000),
            },
        ]
    }

    fn default_model(&self) -> Option<ModelInfo> {
        self.available_models().into_iter().find(|m| m.is_default)
    }

    fn supports_model_override(&self) -> bool {
        true
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("nim")))
            .into())
    }
}
