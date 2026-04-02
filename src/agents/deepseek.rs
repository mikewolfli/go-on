//! deepseek.rs
//! Auto-generated English doc: module overview.
//!
use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::{
    option_f64, option_string, option_u64, principles_to_text, stream_sse_to_sender,
};

pub struct DeepSeekAgent {
    api_key_env: String,
    model: String,
    client: reqwest::Client,
}

impl DeepSeekAgent {
    pub fn new(api_key_env: String, model: String, client: reqwest::Client) -> Self {
        Self {
            api_key_env,
            model,
            client,
        }
    }

    fn build_payload(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
    ) -> Value {
        let mut final_messages: Vec<Message> = Vec::new();

        if let Some(items) = principles {
            if !items.is_empty() {
                final_messages.push(Message {
                    role: "system".to_string(),
                    content: principles_to_text(&items),
                });
            }
        }
        final_messages.extend(messages);

        let model = option_string(&options, "model").unwrap_or_else(|| self.model.clone());
        let temperature = option_f64(&options, "temperature");
        let max_tokens = option_u64(&options, "max_tokens");
        let top_p = option_f64(&options, "top_p");

        let mut payload = json!({
            "model": model,
            "messages": final_messages,
            "stream": true
        });

        if let Some(value) = temperature {
            payload["temperature"] = Value::from(value);
        }
        if let Some(value) = max_tokens {
            payload["max_tokens"] = Value::from(value);
        }
        if let Some(value) = top_p {
            payload["top_p"] = Value::from(value);
        }

        payload
    }

    async fn chat_once(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: mpsc::UnboundedSender<String>,
    ) -> Result<()> {
        let api_key = resolve_secret(&self.api_key_env, "deepseek.api_key_env")?;
        let payload = self.build_payload(messages, principles, options);

        let response = self
            .client
            .post("https://api.deepseek.com/v1/chat/completions")
            .bearer_auth(api_key)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("deepseek request failed with {status}: {body}");
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for DeepSeekAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: mpsc::UnboundedSender<String>,
    ) -> Result<()> {
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

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("deepseek request failed")))
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "deepseek-v3".to_string(),
                name: "DeepSeek v3".to_string(),
                description: "Most capable model, supports vision and function calling".to_string(),
                is_default: false,
                capabilities: vec![
                    "chat".to_string(),
                    "vision".to_string(),
                    "function_calling".to_string(),
                ],
                context_window: Some(64000),
            },
            ModelInfo {
                id: "deepseek-chat".to_string(),
                name: "DeepSeek Chat".to_string(),
                description: "Fast chat model, optimized for speed".to_string(),
                is_default: self.model == "deepseek-chat",
                capabilities: vec!["chat".to_string()],
                context_window: Some(4096),
            },
            ModelInfo {
                id: "deepseek-coder".to_string(),
                name: "DeepSeek Coder".to_string(),
                description: "Specialized for code generation and analysis".to_string(),
                is_default: false,
                capabilities: vec!["chat".to_string(), "code".to_string()],
                context_window: Some(4096),
            },
        ]
    }

    fn default_model(&self) -> Option<ModelInfo> {
        self.available_models().into_iter().find(|m| m.is_default)
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

    #[test]
    fn build_payload_injects_system_principles_and_options() {
        let agent = DeepSeekAgent::new(
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-chat".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(
            vec![message("user", "ship it")],
            Some(vec!["Prefer tests".to_string()]),
            Some(HashMap::from([
                ("model".to_string(), json!("deepseek-reasoner")),
                ("temperature".to_string(), json!(0.1)),
                ("max_tokens".to_string(), json!(1024)),
            ])),
        );

        assert_eq!(payload["model"], "deepseek-reasoner");
        assert_eq!(payload["messages"][0]["role"], "system");
        assert!(payload["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Prefer tests"));
        assert_eq!(payload["messages"][1]["content"], "ship it");
        assert_eq!(payload["temperature"], 0.1);
        assert_eq!(payload["max_tokens"], 1024);
    }
}
