//! deepseek.rs
//! Auto-generated English doc: module overview.
//!
use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, request_failed_msg};
use crate::agents::{
    apply_openai_common_options, option_string, principles_to_text, stream_sse_to_sender,
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

        let mut payload = json!({
            "model": model,
            "messages": final_messages,
            "stream": true
        });

        // Apply common OpenAI options (temperature, top_p, max_tokens, stop,
        // tools, tool_choice, response_format, seed, etc.)
        apply_openai_common_options(&mut payload, &options);

        // DeepSeek-specific: thinking mode control (enabled/disabled + reasoning_effort)
        if let Some(thinking) = options.as_ref().and_then(|o| o.get("thinking")) {
            payload["thinking"] = thinking.clone();
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
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("deepseek", &status.to_string(), &body)
            );
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("deepseek")))
            .into())
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                name: "DeepSeek V4 Flash".to_string(),
                description: "Latest fast model, non-thinking mode".to_string(),
                is_default: self.model == "deepseek-v4-flash",
                capabilities: vec!["chat".to_string(), "function_calling".to_string()],
                context_window: Some(128000),
            },
            ModelInfo {
                id: "deepseek-v4-pro".to_string(),
                name: "DeepSeek V4 Pro".to_string(),
                description: "Latest pro model with thinking/reasoning mode".to_string(),
                is_default: self.model == "deepseek-v4-pro",
                capabilities: vec![
                    "chat".to_string(),
                    "reasoning".to_string(),
                    "function_calling".to_string(),
                ],
                context_window: Some(128000),
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

    #[test]
    fn build_payload_injects_system_principles_and_options() {
        let agent = DeepSeekAgent::new(
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(
            vec![message("user", "ship it")],
            Some(vec!["Prefer tests".to_string()]),
            Some(HashMap::from([
                ("model".to_string(), json!("deepseek-v4-pro")),
                ("temperature".to_string(), json!(0.1)),
                ("max_tokens".to_string(), json!(1024)),
            ])),
        );

        assert_eq!(payload["model"], "deepseek-v4-pro");
        assert_eq!(payload["messages"][0]["role"], "system");
        assert!(payload["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Prefer tests"));
        assert_eq!(payload["messages"][1]["content"], "ship it");
        assert_eq!(payload["temperature"], 0.1);
        assert_eq!(payload["max_tokens"], 1024);
    }

    #[test]
    fn build_payload_without_principles_or_options() {
        let agent = DeepSeekAgent::new(
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(vec![message("user", "hello")], None, None);

        assert_eq!(payload["model"], "deepseek-v4-flash");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("max_tokens").is_none());
        assert_eq!(payload["stream"], true);
    }

    #[test]
    fn available_models_includes_default() {
        let agent = DeepSeekAgent::new(
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let models = agent.available_models();
        assert!(models.len() >= 2, "should have at least 2 models");

        let has_flash = models.iter().any(|m| m.id == "deepseek-v4-flash");
        assert!(has_flash, "should include deepseek-v4-flash");

        let has_pro = models.iter().any(|m| m.id == "deepseek-v4-pro");
        assert!(has_pro, "should include deepseek-v4-pro");

        let default = agent.default_model();
        assert!(default.is_some(), "should have a default model");
        assert_eq!(default.unwrap().id, "deepseek-v4-flash");
    }

    #[test]
    fn build_payload_with_principles_only() {
        let agent = DeepSeekAgent::new(
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(
            vec![message("user", "hello")],
            Some(vec!["Be concise".to_string(), "Use examples".to_string()]),
            None,
        );

        let content = payload["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("Be concise"));
        assert!(content.contains("Use examples"));
        assert_eq!(payload["model"], "deepseek-v4-flash");
    }
}
