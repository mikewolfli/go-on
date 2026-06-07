//! Groq agent implementation
//!
//! This module provides an implementation for the Groq API.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, is_non_retryable_4xx, request_failed_msg};
use crate::agents::{apply_openai_common_options, principles_to_text, stream_sse_to_sender};

pub struct GroqAgent {
    api_key_env: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl GroqAgent {
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

        // If tools were set via options but tool_choice wasn't, default to "auto"
        if payload.get("tools").is_some() && payload.get("tool_choice").is_none() {
            payload["tool_choice"] = serde_json::json!("auto");
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
        let api_key = resolve_secret(&self.api_key_env, "groq.api_key_env")?;
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
                chat_request_failed_msg("groq", &status.to_string(), &body)
            );
        }

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("groq: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for GroqAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "llama-4-maverick".to_string(),
                name: "Llama 4 Maverick".to_string(),
                description: "Llama 4 Maverick (experimental, fast, function calling)"
                    .to_string(),
                is_default: self.model == "llama-4-maverick",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(1_000_000),
            },
            ModelInfo {
                id: "llama-4-scout-17b-16e-instruct".to_string(),
                name: "Llama 4 Scout 17B MoE".to_string(),
                description: "Llama 4 Scout 17B MoE Instruct (10M context, efficient)"
                    .to_string(),
                is_default: self.model == "llama-4-scout-17b-16e-instruct",
                capabilities: vec![
                    "chat".to_string(),
                    "function_calling".to_string(),
                    "streaming".to_string(),
                ],
                context_window: Some(10_000_000),
            },
            ModelInfo {
                id: "llama-3.3-70b-versatile".to_string(),
                name: "Llama 3.3 70B Versatile".to_string(),
                description: "Llama 3.3 70B Versatile (flagship LLM for chat, function calling, and streaming)".to_string(),
                is_default: self.model == "llama-3.3-70b-versatile",
                capabilities: vec!["chat".to_string(), "function_calling".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "llama-3.1-8b-instant".to_string(),
                name: "Llama 3.1 8B Instant".to_string(),
                description: "Llama 3.1 8B Instant (fast, lightweight)".to_string(),
                is_default: self.model == "llama-3.1-8b-instant",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "deepseek-r1-distill-llama-70b".to_string(),
                name: "DeepSeek R1 Distill Llama 70B".to_string(),
                description: "DeepSeek R1 Distill Llama 70B (reasoning, Groq hosted)"
                    .to_string(),
                is_default: self.model == "deepseek-r1-distill-llama-70b",
                capabilities: vec!["chat".to_string(), "reasoning".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "qwen/qwen3-32b".to_string(),
                name: "Qwen 3 32B (Preview)".to_string(),
                description: "Qwen3 32B (preview)".to_string(),
                is_default: self.model == "qwen/qwen3-32b",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
            },
            ModelInfo {
                id: "mixtral-8x7b-32768".to_string(),
                name: "Mixtral 8x7B".to_string(),
                description: "Mixtral 8x7B (32K context, function calling)".to_string(),
                is_default: self.model == "mixtral-8x7b-32768",
                capabilities: vec!["chat".to_string(), "function_calling".to_string(), "streaming".to_string()],
                context_window: Some(32_768),
            },
            ModelInfo {
                id: "gemma2-9b-it".to_string(),
                name: "Gemma 2 9B IT".to_string(),
                description: "Gemma 2 9B IT (instruction-tuned, lightweight)".to_string(),
                is_default: self.model == "gemma2-9b-it",
                capabilities: vec!["chat".to_string(), "streaming".to_string()],
                context_window: Some(8_192),
            },
            ModelInfo {
                id: "qwen-2.5-32b".to_string(),
                name: "Qwen 2.5 32B".to_string(),
                description: "Qwen 2.5 32B (function calling)".to_string(),
                is_default: self.model == "qwen-2.5-32b",
                capabilities: vec!["chat".to_string(), "function_calling".to_string(), "streaming".to_string()],
                context_window: Some(128_000),
            },
        ]
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("groq")))
            .into())
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

    fn agent() -> GroqAgent {
        GroqAgent::new(
            "GROQ_API_KEY".to_string(),
            "https://api.groq.com".to_string(),
            "llama-3.3-70b-versatile".to_string(),
            reqwest::Client::new(),
        )
    }

    #[test]
    fn test_build_payload_basic() {
        let payload = agent().build_payload(&[message("user", "hello")], &None, &None);

        assert_eq!(payload["model"], "llama-3.3-70b-versatile");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert_eq!(payload["stream"], true);
        assert!(payload.get("temperature").is_none());
    }

    #[test]
    fn test_build_payload_with_principles() {
        let payload = agent().build_payload(
            &[message("user", "do it")],
            &Some(vec![
                "Be thorough".to_string(),
                "Test everything".to_string(),
            ]),
            &None,
        );

        assert_eq!(payload["messages"][0]["role"], "system");
        let content = payload["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("Be thorough"));
        assert!(content.contains("Test everything"));
        assert_eq!(payload["messages"][1]["content"], "do it");
    }

    #[test]
    fn test_build_payload_tool_choice_auto() {
        let payload = agent().build_payload(
            &[message("user", "use tools")],
            &None,
            &Some(HashMap::from([(
                "tools".to_string(),
                json!([{
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "description": "Get the weather",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "location": { "type": "string" }
                            }
                        }
                    }
                }]),
            )])),
        );

        assert!(payload.get("tools").is_some(), "tools should be present");
        assert_eq!(
            payload["tool_choice"], "auto",
            "tool_choice should default to auto when tools are present"
        );
    }

    #[test]
    fn test_build_payload_tool_choice_preserved() {
        let payload = agent().build_payload(
            &[message("user", "pick a tool")],
            &None,
            &Some(HashMap::from([
                (
                    "tools".to_string(),
                    json!([{
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "description": "Get the weather",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "location": { "type": "string" }
                                }
                            }
                        }
                    }]),
                ),
                ("tool_choice".to_string(), json!("required")),
            ])),
        );

        assert!(payload.get("tools").is_some(), "tools should be present");
        assert_eq!(
            payload["tool_choice"], "required",
            "tool_choice should remain 'required' when explicitly set"
        );
    }
}
