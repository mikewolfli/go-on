//! Langboat agent implementation
//!
//! This module provides an implementation for the Langboat AI API.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, request_failed_msg};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct LangboatAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl LangboatAgent {
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
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
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
        final_messages.extend(messages);

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
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "langboat.api_key_env")?;
        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let payload = self.build_payload(messages, principles, &options);

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
                chat_request_failed_msg("langboat", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for LangboatAgent {
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("langboat")))
            .into())
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "langboat-chat".to_string(),
                name: "Langboat Chat".to_string(),
                description: "Langboat Chat".to_string(),
                is_default: self.model == "langboat-chat",
                capabilities: vec!["chat".to_string()],
                context_window: Some(8192),
            },
            ModelInfo {
                id: "langboat-chat-large".to_string(),
                name: "Langboat Chat Large".to_string(),
                description: "Langboat Chat Large".to_string(),
                is_default: self.model == "langboat-chat-large",
                capabilities: vec!["chat".to_string()],
                context_window: Some(32768),
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
