//! SiliconFlow agent implementation
//!
//! This module provides an implementation for the SiliconFlow AI API.
//! Official docs: https://docs.siliconflow.cn/api-reference
//!
//! SiliconFlow provides access to various open-source models via
//! an OpenAI-compatible API. Default endpoint: https://api.siliconflow.cn/v1

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, retry_chat_once};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct SiliconFlowAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl SiliconFlowAgent {
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
        let api_key = resolve_secret(&self.api_key_env, "siliconflow.api_key_env")?;
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
                chat_request_failed_msg("siliconflow", &status.to_string(), &body)
            );
        }

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("siliconflow: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for SiliconFlowAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> crate::core::error::Result<()> {
        let chat_messages = messages;

        retry_chat_once(
            || async {
                self.chat_once(&chat_messages, &principles, &options, sender.clone())
                    .await
                    .map_err(Into::into)
            },
            3,
        )
        .await
    }

    /// SiliconFlow provides a curated selection of open-source models.
    ///
    /// Key models available as of 2026:
    ///   - deepseek-ai/DeepSeek-V3-0324: Latest DeepSeek V3
    ///   - Qwen/Qwen3-235B-A22B: Qwen3 MoE flagship
    ///   - Qwen/Qwen3-30B-A3B: Qwen3 balanced MoE
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "Pro/deepseek-ai/DeepSeek-V3-0324".to_string(),
                name: "DeepSeek V3 (0324)".to_string(),
                description: "DeepSeek V3 (03-2024) via SiliconFlow Pro".to_string(),
                is_default: self.model == "Pro/deepseek-ai/DeepSeek-V3-0324",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "deepseek-ai/DeepSeek-R1".to_string(),
                name: "DeepSeek R1".to_string(),
                description: "DeepSeek R1 reasoning model via SiliconFlow".to_string(),
                is_default: self.model == "deepseek-ai/DeepSeek-R1",
                capabilities: vec![
                    "chat".to_string(),
                    "reasoning".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "Qwen/Qwen3-235B-A22B".to_string(),
                name: "Qwen3 235B (MoE)".to_string(),
                description: "Qwen3 235B MoE flagship via SiliconFlow".to_string(),
                is_default: self.model == "Qwen/Qwen3-235B-A22B",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(131_072),
            },
            ModelInfo {
                id: "Qwen/Qwen3-30B-A3B".to_string(),
                name: "Qwen3 30B (MoE)".to_string(),
                description: "Qwen3 30B MoE balanced model via SiliconFlow".to_string(),
                is_default: self.model == "Qwen/Qwen3-30B-A3B",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(32_768),
            },
            ModelInfo {
                id: "meta-llama/Meta-Llama-3.1-8B-Instruct".to_string(),
                name: "Meta Llama 3.1 8B Instruct".to_string(),
                description: "Meta Llama 3.1 8B via SiliconFlow".to_string(),
                is_default: self.model == "meta-llama/Meta-Llama-3.1-8B-Instruct",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
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

    fn agent() -> SiliconFlowAgent {
        SiliconFlowAgent::new(
            "SILICONFLOW_API_KEY".to_string(),
            "https://api.siliconflow.cn".to_string(),
            "Pro/deepseek-ai/DeepSeek-V3-0324".to_string(),
            reqwest::Client::new(),
        )
    }

    #[test]
    fn test_build_payload_basic() {
        let payload = agent().build_payload(&[message("user", "hello")], &None, &None);

        assert_eq!(payload["model"], "Pro/deepseek-ai/DeepSeek-V3-0324");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert_eq!(payload["stream"], true);
    }

    #[test]
    fn test_build_payload_with_principles() {
        let payload = agent().build_payload(
            &[message("user", "write code")],
            &Some(vec!["Be clean".to_string()]),
            &None,
        );

        assert_eq!(payload["messages"][0]["role"], "system");
        assert!(payload["messages"][0]["content"]
            .as_str()
            .expect("messages[0] content should be a string")
            .contains("Be clean"));
    }

    #[test]
    fn test_available_models() {
        let models = agent().available_models();
        assert!(models.len() >= 4);
        assert!(models.iter().any(|m| m.id.contains("DeepSeek-V3")));
        assert!(models.iter().any(|m| m.id.contains("Qwen3")));
    }
}
