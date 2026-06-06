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
use crate::agents::agent::{chat_request_failed_msg, is_non_retryable_4xx, request_failed_msg};
use crate::agents::{
    apply_openai_common_options, option_string, principles_to_text, stream_sse_to_sender,
};

pub struct DeepSeekAgent {
    base_url: String,
    api_key_env: String,
    model: String,
    client: reqwest::Client,
}

impl DeepSeekAgent {
    pub fn new(
        base_url: String,
        api_key_env: String,
        model: String,
        client: reqwest::Client,
    ) -> Self {
        Self {
            base_url,
            api_key_env,
            model,
            client,
        }
    }

    fn completion_endpoint(&self) -> String {
        // Official DeepSeek Chat Completions path is /chat/completions.
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn normalize_user_id(value: &Value) -> Option<String> {
        let raw = value.as_str()?.trim();
        if raw.is_empty() || raw.len() > 512 {
            return None;
        }

        // DeepSeek docs: user_id character set is [a-zA-Z0-9-_].
        if raw
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            Some(raw.to_string())
        } else {
            None
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
                final_messages.push(Message {
                    role: "system".to_string(),
                    content: principles_to_text(&items),
                });
            }
        }
        final_messages.extend(messages.iter().cloned());

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

        // When thinking is enabled, DeepSeek API requires temperature and top_p
        // to be UNSET. Remove them if they were set by apply_openai_common_options.
        if let Some(thinking) = payload.get("thinking") {
            let is_enabled = thinking.get("type").and_then(|v| v.as_str()) == Some("enabled")
                || thinking.as_str() == Some("enabled");
            if is_enabled {
                if let Some(obj) = payload.as_object_mut() {
                    obj.remove("temperature");
                    obj.remove("top_p");
                }
            }
        }

        // DeepSeek official field is `user_id`. Accept upstream `user` and map it.
        if payload.get("user_id").is_none() {
            if let Some(user) = options.as_ref().and_then(|o| o.get("user")) {
                if let Some(user_id) = Self::normalize_user_id(user) {
                    payload["user_id"] = Value::String(user_id);
                }
            }
        }

        if payload.get("user").is_some() {
            payload
                .as_object_mut()
                .expect("B48: payload object")
                .remove("user");
        }

        // DeepSeek marks these as deprecated/no-op; drop to avoid stale semantics.
        if let Some(obj) = payload.as_object_mut() {
            obj.remove("frequency_penalty");
            obj.remove("presence_penalty");
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
        let api_key = resolve_secret(&self.api_key_env, "deepseek.api_key_env")?;
        let payload = self.build_payload(messages, principles, options);

        let url = self.completion_endpoint();
        let response = self
            .client
            .post(url)
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

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("deepseek: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
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
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("deepseek")))
            .into())
    }

    /// Returns the currently available DeepSeek models per their official API docs:
    ///   https://api-docs.deepseek.com/quick_start/pricing
    ///
    /// As of 2025, DeepSeek offers exactly two production models:
    ///   - deepseek-v4-flash  (fast, non-thinking, function calling)
    ///   - deepseek-v4-pro    (pro, thinking/reasoning, function calling)
    ///
    /// IMPORTANT: Do NOT add fictional model IDs like "deepseek-r1" or "deepseek-chat"
    /// as separate entries. The config alias "deepseek-chat" resolves to deepseek-v4-flash
    /// via is_default. If DeepSeek releases new models, update this list from their
    /// OFFICIAL documentation only — never from blog posts, rumors, or third-party lists.
    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                name: "DeepSeek V4 Flash".to_string(),
                description: "Latest fast model, non-thinking mode, function calling".to_string(),
                // Config templates use "deepseek-chat" as an alias for the default model.
                // DO NOT add "deepseek-chat" as a separate model entry — it is NOT a real
                // API model ID. DeepSeek's API only recognizes "deepseek-v4-flash" and
                // "deepseek-v4-pro". The alias is matched here so configs using the
                // legacy name still work and resolve to v4-flash.
                // Official docs: https://api-docs.deepseek.com/quick_start/pricing
                // Context: 1M tokens (1,048,576), Max output: 384K tokens
                is_default: self.model == "deepseek-v4-flash"
                    || self.model == "deepseek-chat"
                    || self.model.is_empty(),
                capabilities: vec!["chat".to_string(), "function_calling".to_string()],
                context_window: Some(1_048_576),
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
                // Official docs: same 1M context for v4-pro
                context_window: Some(1_048_576),
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
            "https://api.deepseek.com".to_string(),
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(
            &vec![message("user", "ship it")],
            &Some(vec!["Prefer tests".to_string()]),
            &Some(HashMap::from([
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
            "https://api.deepseek.com".to_string(),
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(&vec![message("user", "hello")], &None, &None);

        assert_eq!(payload["model"], "deepseek-v4-flash");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("max_tokens").is_none());
        assert_eq!(payload["stream"], true);
    }

    #[test]
    fn available_models_includes_default() {
        let agent = DeepSeekAgent::new(
            "https://api.deepseek.com".to_string(),
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
            "https://api.deepseek.com".to_string(),
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(
            &vec![message("user", "hello")],
            &Some(vec!["Be concise".to_string(), "Use examples".to_string()]),
            &None,
        );

        let content = payload["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("Be concise"));
        assert!(content.contains("Use examples"));
        assert_eq!(payload["model"], "deepseek-v4-flash");
    }

    #[test]
    fn build_payload_maps_user_to_user_id() {
        let agent = DeepSeekAgent::new(
            "https://api.deepseek.com".to_string(),
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(
            &vec![message("user", "hello")],
            &None,
            &Some(HashMap::from([("user".to_string(), json!("tenant-a"))])),
        );

        assert_eq!(payload["user_id"], "tenant-a");
        assert!(payload.get("user").is_none());
    }

    #[test]
    fn completion_endpoint_uses_official_path() {
        let agent = DeepSeekAgent::new(
            "https://api.deepseek.com".to_string(),
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        assert_eq!(
            agent.completion_endpoint(),
            "https://api.deepseek.com/chat/completions"
        );
    }

    #[test]
    fn normalize_user_id_rejects_invalid_chars_and_too_long() {
        let bad_chars = json!("user@tenant");
        assert!(DeepSeekAgent::normalize_user_id(&bad_chars).is_none());

        let too_long = json!("a".repeat(513));
        assert!(DeepSeekAgent::normalize_user_id(&too_long).is_none());
    }

    #[test]
    fn normalize_user_id_accepts_allowed_charset() {
        let valid = json!("tenant_A-01");
        assert_eq!(
            DeepSeekAgent::normalize_user_id(&valid).as_deref(),
            Some("tenant_A-01")
        );
    }

    #[test]
    fn build_payload_drops_deprecated_penalty_fields() {
        let agent = DeepSeekAgent::new(
            "https://api.deepseek.com".to_string(),
            "DEEPSEEK_API_KEY".to_string(),
            "deepseek-v4-flash".to_string(),
            reqwest::Client::new(),
        );

        let payload = agent.build_payload(
            &vec![message("user", "hello")],
            &None,
            &Some(HashMap::from([
                ("frequency_penalty".to_string(), json!(0.2)),
                ("presence_penalty".to_string(), json!(0.4)),
            ])),
        );

        assert!(payload.get("frequency_penalty").is_none());
        assert!(payload.get("presence_penalty").is_none());
    }
}
