//! xAI (Grok) agent implementation
//!
//! This module provides an implementation for the xAI API (Grok models).
//! API reference: https://docs.x.ai/api/endpoints
//!
//! xAI provides access to Grok models via an OpenAI-compatible API.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, request_failed_msg};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct XaiAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl XaiAgent {
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
        let api_key = resolve_secret(&self.api_key_env, "xai.api_key_env")?;
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
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
                chat_request_failed_msg("xai", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for XaiAgent {
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("xai")))
            .into())
    }

    /// xAI Grok models per official docs: https://docs.x.ai/api/endpoints
    ///
    /// Current production models (as of 2026):
    ///   - grok-3: Latest flagship model
    ///   - grok-3-mini: Fast, lightweight variant
    ///   - grok-3-mini-fast: Fastest inference
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "grok-3".to_string(),
                name: "Grok 3".to_string(),
                description: "xAI Grok 3 flagship model (1M context)".to_string(),
                is_default: self.model == "grok-3",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                    "vision".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            ModelInfo {
                id: "grok-3-mini".to_string(),
                name: "Grok 3 Mini".to_string(),
                description: "xAI Grok 3 Mini fast & efficient (1M context)".to_string(),
                is_default: self.model == "grok-3-mini",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            ModelInfo {
                id: "grok-3-mini-fast".to_string(),
                name: "Grok 3 Mini Fast".to_string(),
                description: "xAI Grok 3 Mini Fast (fastest inference)".to_string(),
                is_default: self.model == "grok-3-mini-fast",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(131_072),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    fn agent() -> XaiAgent {
        XaiAgent::new(
            "XAI_API_KEY".to_string(),
            "https://api.x.ai".to_string(),
            "grok-3".to_string(),
            reqwest::Client::new(),
        )
    }

    #[test]
    fn test_build_payload_basic() {
        let payload = agent().build_payload(vec![message("user", "hello")], None, &None);

        assert_eq!(payload["model"], "grok-3");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert_eq!(payload["stream"], true);
    }

    #[test]
    fn test_build_payload_with_principles() {
        let payload = agent().build_payload(
            vec![message("user", "do it")],
            Some(vec!["Be concise".to_string()]),
            &None,
        );

        assert_eq!(payload["messages"][0]["role"], "system");
        assert!(payload["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Be concise"));
        assert_eq!(payload["messages"][1]["content"], "do it");
    }

    #[test]
    fn test_available_models() {
        let models = agent().available_models();
        assert!(models.len() >= 3);
        assert!(models.iter().any(|m| m.id == "grok-3"));
        assert!(models.iter().any(|m| m.id == "grok-3-mini"));
        let default = agent().default_model();
        assert!(default.is_some());
        assert_eq!(default.unwrap().id, "grok-3");
    }
}
