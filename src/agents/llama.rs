//! Llama (self-hosted) agent implementation
//!
//! This module provides an implementation for self-hosted Llama models
//! served via llama.cpp, Ollama, vLLM, or any OpenAI-compatible server.
//!
//! Default endpoint: http://localhost:8080/v1 (compatible with llama.cpp server,
//! Ollama, and vLLM).
//!
//! The API key is optional for local deployments. Set `LLAMA_API_KEY` if your
//! server requires authentication.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, is_non_retryable_4xx, request_failed_msg};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct LlamaAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl LlamaAgent {
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
        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let payload = self.build_payload(messages, principles, options);

        let mut request = self
            .client
            .post(&endpoint)
            .header("Content-Type", "application/json");

        // API key is optional for local self-hosted deployments
        if let Ok(api_key) = resolve_secret(&self.api_key_env, "llama.api_key_env") {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("llama", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for LlamaAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "llama3.2".to_string(),
                name: "Llama 3.2".to_string(),
                description: "Meta Llama 3.2 (latest generation)".to_string(),
                is_default: self.model == "llama3.2",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(131_072),
            },
            ModelInfo {
                id: "llama3.2-vision".to_string(),
                name: "Llama 3.2 Vision".to_string(),
                description: "Meta Llama 3.2 Vision (multimodal)".to_string(),
                is_default: self.model == "llama3.2-vision",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(131_072),
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("llama")))
            .into())
    }
}
