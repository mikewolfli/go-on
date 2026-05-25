//! OpenAI agent implementation
//!
//! This module provides an implementation for the OpenAI API.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message};
use crate::agents::agent::{chat_request_failed_msg, request_failed_msg};
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
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        tools: Option<Vec<Value>>,
    ) -> Value {
        let mut final_messages: Vec<Message> = Vec::new();

        if let Some(items) = principles {
            if !items.is_empty() {
                let system_text = principles_to_text(&items);
                final_messages.push(Message {
                    role: "system".to_string(),
                    content: system_text,
                });
            }
        }

        final_messages.extend(messages);

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

        // Attach native function-calling tools to the payload
        if let Some(tool_defs) = tools {
            if !tool_defs.is_empty() {
                payload["tools"] = Value::Array(tool_defs);
                // Only set tool_choice when not already specified via options
                if payload.get("tool_choice").is_none() {
                    payload["tool_choice"] = json!("auto");
                }
            }
        }

        payload
    }

    async fn chat_once(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "openai.api_key_env")?;
        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let payload = self.build_payload(messages, principles, &options, None);

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

        stream_sse_to_sender(response, sender).await
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("openai")))
            .into())
    }

    fn available_models(&self) -> Vec<crate::agent::ModelInfo> {
        vec![
            crate::agent::ModelInfo {
                id: "gpt-5.5".to_string(),
                name: "GPT-5.5".to_string(),
                description: "OpenAI GPT-5.5 (1M context, flagship model)".to_string(),
                is_default: self.model == "gpt-5.5",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                    "reasoning".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            crate::agent::ModelInfo {
                id: "gpt-5.4".to_string(),
                name: "GPT-5.4".to_string(),
                description: "OpenAI GPT-5.4 (1M context, affordable)".to_string(),
                is_default: self.model == "gpt-5.4",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                    "reasoning".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            crate::agent::ModelInfo {
                id: "gpt-5.4-mini".to_string(),
                name: "GPT-5.4 Mini".to_string(),
                description: "OpenAI GPT-5.4 Mini (400K context, strongest mini)".to_string(),
                is_default: self.model == "gpt-5.4-mini",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                    "reasoning".to_string(),
                ],
                context_window: Some(400_000),
            },
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
            crate::agent::ModelInfo {
                id: "gpt-4-turbo".to_string(),
                name: "GPT-4 Turbo".to_string(),
                description: "OpenAI GPT-4 Turbo (legacy)".to_string(),
                is_default: self.model == "gpt-4-turbo",
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(128_000),
            },
            crate::agent::ModelInfo {
                id: "gpt-3.5-turbo".to_string(),
                name: "GPT-3.5 Turbo".to_string(),
                description: "OpenAI GPT-3.5 Turbo [deprecated — will be shut down]".to_string(),
                is_default: self.model == "gpt-3.5-turbo",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(16_385),
            },
        ]
    }

    fn supports_model_override(&self) -> bool {
        true
    }
}
