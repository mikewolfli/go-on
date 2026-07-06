//! OpenAI agent implementation
//!
//! This module provides an implementation for the OpenAI API.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message};
use crate::agents::agent::{
    chat_request_failed_msg, retry_chat_once,
};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct OpenAiAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiAgent {
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

        if let Some(items) = principles {
            if !items.is_empty() {
                let system_text = principles_to_text(items);
                final_messages.push(Message {
                    role: "system".to_string(),
                    content: system_text,
                });
            }
        }

        final_messages.extend(messages.iter().cloned());

        let mut payload = json!({
            "model": self.model,
            "messages": final_messages,
            "stream": true
        });

        // Forward reasoning_effort for o-series reasoning models
        if let Some(map) = options {
            if let Some(re) = map.get("reasoning_effort") {
                payload["reasoning_effort"] = re.clone();
            }
        }

        apply_openai_common_options(&mut payload, options);

        // If tools were set via options but tool_choice wasn't, default to "auto"
        if payload.get("tools").is_some() && payload.get("tool_choice").is_none() {
            payload["tool_choice"] = json!("auto");
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
        let api_key = resolve_secret(&self.api_key_env, "openai.api_key_env")?;
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
                chat_request_failed_msg("openai", &status.to_string(), &body)
            );
        }

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("openai: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
        }

        stream_sse_to_sender(response, sender).await
    }

    /// Chat with optional SSE compression.
    ///
    /// When `options` contains `"sse_compress": true`, the SSE stream is
    /// compressed with gzip before parsing, reducing bandwidth on large
    /// responses. This is transparent to the token extraction layer.
    async fn chat_once_compressed(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
        compress_cfg: &crate::agents::StreamingConfig,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "openai.api_key_env")?;
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
                chat_request_failed_msg("openai", &status.to_string(), &body)
            );
        }

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("openai: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
        }

        crate::agents::stream_sse_to_sender_compressed(response, sender, compress_cfg).await
    }
}

#[async_trait]
impl Agent for OpenAiAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> crate::core::error::Result<()> {
        // Check if SSE compression is requested via options
        let use_compression = options
            .as_ref()
            .and_then(|o| o.get("sse_compress"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let compress_cfg = if use_compression {
            Some(crate::agents::StreamingConfig {
                enable_compression: true,
                ..Default::default()
            })
        } else {
            None
        };

        let chat_messages = messages;
        retry_chat_once(
            || async {
                let result = if let Some(ref cfg) = compress_cfg {
                    self.chat_once_compressed(
                        &chat_messages,
                        &principles,
                        &options,
                        sender.clone(),
                        cfg,
                    )
                    .await
                } else {
                    self.chat_once(&chat_messages, &principles, &options, sender.clone())
                        .await
                };
                result.map_err(Into::into)
            },
            3,
        )
        .await
    }

    fn available_models(&self) -> Vec<crate::agent::ModelInfo> {
        let mut models = vec![
            crate::agent::ModelInfo {
                id: "gpt-4.1".to_string(),
                name: "GPT-4.1".to_string(),
                description: "OpenAI GPT-4.1 (1M context)".to_string(),
                is_default: self.model == "gpt-4.1",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            crate::agent::ModelInfo {
                id: "gpt-4.1-mini".to_string(),
                name: "GPT-4.1 Mini".to_string(),
                description: "OpenAI GPT-4.1 Mini (1M context)".to_string(),
                is_default: self.model == "gpt-4.1-mini",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            crate::agent::ModelInfo {
                id: "gpt-4.1-nano".to_string(),
                name: "GPT-4.1 Nano".to_string(),
                description: "OpenAI GPT-4.1 Nano (1M context)".to_string(),
                is_default: self.model == "gpt-4.1-nano",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            crate::agent::ModelInfo {
                id: "o4-mini".to_string(),
                name: "o4-mini".to_string(),
                description: "OpenAI o4-mini (latest reasoning model, fast & cost-efficient)"
                    .to_string(),
                is_default: self.model == "o4-mini",
                capabilities: vec![
                    "chat".to_string(),
                    "reasoning".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(200_000),
            },
            crate::agent::ModelInfo {
                id: "o3-mini".to_string(),
                name: "o3-mini".to_string(),
                description: "OpenAI o3-mini (reasoning model)".to_string(),
                is_default: self.model == "o3-mini",
                capabilities: vec![
                    "chat".to_string(),
                    "reasoning".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(200_000),
            },
            crate::agent::ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                description: "OpenAI GPT-4o (omni model)".to_string(),
                is_default: self.model == "gpt-4o",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(128_000),
            },
            crate::agent::ModelInfo {
                id: "gpt-4o-mini".to_string(),
                name: "GPT-4o Mini".to_string(),
                description: "OpenAI GPT-4o Mini (cost-efficient)".to_string(),
                is_default: self.model == "gpt-4o-mini",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(128_000),
            },
        ];

        // Keep runtime resilient to newly released model IDs configured by users.
        if !self.model.is_empty() && !models.iter().any(|m| m.id == self.model) {
            models.insert(
                0,
                crate::agent::ModelInfo {
                    id: self.model.clone(),
                    name: self.model.clone(),
                    description: "Configured OpenAI model".to_string(),
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

        models
    }

    fn supports_model_override(&self) -> bool {
        true
    }
}
