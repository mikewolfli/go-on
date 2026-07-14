//! Perplexity AI agent implementation
//!
//! This module provides an implementation for the Perplexity AI API.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, retry_chat_once};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct PerplexityAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl PerplexityAgent {
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
        let api_key = resolve_secret(&self.api_key_env, "perplexity.api_key_env")?;
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
                chat_request_failed_msg("perplexity", &status.to_string(), &body)
            );
        }

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("perplexity: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for PerplexityAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "sonar-deep-research".to_string(),
                name: "Sonar Deep Research".to_string(),
                description: "Sonar Deep Research (200K context, exhaustive research)".to_string(),
                is_default: self.model == "sonar-deep-research",
                capabilities: vec![
                    "chat".to_string(),
                    "reasoning".to_string(),
                    "streaming".to_string(),
                    "search".to_string(),
                ],
                context_window: Some(200_000),
            },
            ModelInfo {
                id: "sonar-reasoning-pro".to_string(),
                name: "Sonar Reasoning Pro".to_string(),
                description: "Sonar Reasoning Pro (200K context, advanced reasoning)".to_string(),
                is_default: self.model == "sonar-reasoning-pro",
                capabilities: vec![
                    "chat".to_string(),
                    "reasoning".to_string(),
                    "streaming".to_string(),
                    "search".to_string(),
                ],
                context_window: Some(200_000),
            },
            ModelInfo {
                id: "sonar-reasoning".to_string(),
                name: "Sonar Reasoning".to_string(),
                description: "Sonar Reasoning (200K context, balanced reasoning)".to_string(),
                is_default: self.model == "sonar-reasoning",
                capabilities: vec![
                    "chat".to_string(),
                    "reasoning".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(200_000),
            },
            ModelInfo {
                id: "sonar-pro".to_string(),
                name: "Sonar Pro".to_string(),
                description: "Sonar Pro (200K context, chat, streaming, search)".to_string(),
                is_default: self.model == "sonar-pro",
                capabilities: vec![
                    "chat".to_string(),
                    "streaming".to_string(),
                    "search".to_string(),
                ],
                context_window: Some(200_000),
            },
            ModelInfo {
                id: "sonar".to_string(),
                name: "Sonar".to_string(),
                description: "Sonar (200K context)".to_string(),
                is_default: self.model == "sonar",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(200_000),
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
