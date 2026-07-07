//! openai_compatible.rs
//! Auto-generated English doc: module overview.
//!
use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::resolve_secret;
use crate::agent::{Agent, Message};
use crate::agents::agent::{chat_request_failed_msg, retry_chat_once};
use crate::agents::{
    apply_openai_common_options, option_string, principles_to_text, stream_sse_to_sender,
};

pub struct OpenAiCompatibleAgent {
    base_url: String,
    chat_path: String,
    api_key_env: String,
    model: String,
    supports_system: bool,
    client: reqwest::Client,
}

impl OpenAiCompatibleAgent {
    pub fn new(
        base_url: String,
        chat_path: String,
        api_key_env: String,
        model: String,
        supports_system: bool,
        client: reqwest::Client,
    ) -> Self {
        Self {
            base_url,
            chat_path,
            api_key_env,
            model,
            supports_system,
            client,
        }
    }

    fn merge_principles_into_messages(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
    ) -> Vec<Message> {
        let Some(items) = principles else {
            return messages.to_vec();
        };

        if items.is_empty() {
            return messages.to_vec();
        }

        let instruction = principles_to_text(items);

        if self.supports_system {
            let mut merged = Vec::with_capacity(messages.len() + 1);
            merged.push(Message {
                role: "system".to_string(),
                content: instruction,
            });
            merged.extend(messages.iter().cloned());
            return merged;
        }

        // Some OpenAI-compatible providers ignore `system`, so we prepend
        // phase principles to the first user message to preserve constraints.
        let mut owned = messages.to_vec();
        if let Some(first_user) = owned.iter_mut().find(|m| m.role == "user") {
            first_user.content = format!("{}\n{}", instruction, first_user.content);
        } else {
            owned.insert(
                0,
                Message {
                    role: "user".to_string(),
                    content: instruction,
                },
            );
        }

        owned
    }

    fn chat_endpoint(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            if self.chat_path.starts_with('/') {
                self.chat_path.clone()
            } else {
                format!("/{}", self.chat_path)
            }
        )
    }

    fn build_payload(
        &self,
        messages: Vec<Message>,
        options: &Option<HashMap<String, Value>>,
    ) -> Value {
        let model = option_string(options, "model").unwrap_or_else(|| self.model.clone());

        let mut payload = json!({
            "model": model,
            "messages": messages,
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
        let api_key = resolve_secret(&self.api_key_env, "openai_compatible.api_key_env")?;

        let merged = self.merge_principles_into_messages(messages, principles);
        let endpoint = self.chat_endpoint();
        let payload = self.build_payload(merged, options);

        let response = self
            .client
            .post(endpoint)
            .bearer_auth(api_key)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("openai_compatible", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for OpenAiCompatibleAgent {
    fn available_models(&self) -> Vec<crate::agent::ModelInfo> {
        let model_id = self.model.clone();
        if model_id.is_empty() {
            return vec![];
        }
        vec![crate::agent::ModelInfo {
            id: model_id,
            name: self.model.clone(),
            description: format!("OpenAI-compatible provider at {}", self.base_url),
            is_default: true,
            context_window: Some(4096),
            capabilities: vec!["chat".to_string(), "streaming".to_string()],
        }]
    }

    fn default_model(&self) -> Option<crate::agent::ModelInfo> {
        self.available_models().into_iter().next()
    }

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

    fn agent(supports_system: bool, chat_path: &str) -> OpenAiCompatibleAgent {
        OpenAiCompatibleAgent::new(
            "https://example.test/api".to_string(),
            chat_path.to_string(),
            "API_KEY".to_string(),
            "gpt-like".to_string(),
            supports_system,
            reqwest::Client::new(),
        )
    }

    #[test]
    fn merge_principles_inserts_system_message_when_supported() {
        let merged = agent(true, "v1/chat/completions").merge_principles_into_messages(
            &[message("user", "hello")],
            &Some(vec!["Prefer tests".to_string()]),
        );

        assert_eq!(merged[0].role, "system");
        assert!(merged[0].content.contains("Prefer tests"));
        assert_eq!(merged[1].content, "hello");
    }

    #[test]
    fn merge_principles_prefixes_user_message_when_system_not_supported() {
        let merged = agent(false, "v1/chat/completions").merge_principles_into_messages(
            &[message("user", "hello")],
            &Some(vec!["Prefer tests".to_string()]),
        );

        assert_eq!(merged.len(), 1);
        assert!(merged[0].content.contains("Prefer tests"));
        assert!(merged[0].content.contains("hello"));
    }

    #[test]
    fn chat_endpoint_normalizes_relative_and_absolute_paths() {
        assert_eq!(
            agent(true, "v1/chat/completions").chat_endpoint(),
            "https://example.test/api/v1/chat/completions"
        );
        assert_eq!(
            agent(true, "/v1/chat/completions").chat_endpoint(),
            "https://example.test/api/v1/chat/completions"
        );
    }

    #[test]
    fn build_payload_uses_default_model_and_overrides() {
        let payload = agent(true, "/v1/chat/completions").build_payload(
            vec![message("user", "hello")],
            &Some(HashMap::from([
                ("model".to_string(), json!("alt-model")),
                ("temperature".to_string(), json!(0.4)),
                ("max_tokens".to_string(), json!(256)),
            ])),
        );

        assert_eq!(payload["model"], "alt-model");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert_eq!(payload["temperature"], 0.4);
        assert_eq!(payload["max_tokens"], 256);
    }
}
