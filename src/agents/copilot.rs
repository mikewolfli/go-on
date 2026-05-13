//! copilot.rs - GitHub Copilot agent via OAuth device-flow token.
//!
//! Auth flow:
//!   1. On first request, read GitHub OAuth token from env var (default: GITHUB_TOKEN).
//!   2. Exchange it at api.github.com/copilot_internal/v2/token for a short-lived Copilot API token.
//!   3. Cache the Copilot token and auto-refresh before expiry.
//!   4. Call api.githubcopilot.com/chat/completions for chat streaming.
//!
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;
use tracing::warn;

use crate::agent::{Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, request_failed_msg};
use crate::agents::{
    option_f64, option_string, option_u64, principles_to_text, stream_sse_to_sender,
};
use crate::i18n::runtime::tf;

const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_COMPLETIONS_URL: &str = "https://api.githubcopilot.com/chat/completions";

/// Cached Copilot API token with its expiry (Unix timestamp seconds).
struct CachedToken {
    token: String,
    expires_at: u64,
}

pub struct CopilotAgent {
    /// Name of the environment variable holding the GitHub OAuth token.
    token_env: String,
    client: reqwest::Client,
    /// Short-lived Copilot API token, auto-refreshed.
    cached: Mutex<Option<CachedToken>>,
}

impl CopilotAgent {
    /// Create a new agent. `token_env` is the **name** of the environment variable
    /// that holds the GitHub OAuth token (e.g. `"GITHUB_TOKEN"`). The variable is
    /// read lazily on the first chat request, not at construction time.
    pub fn new(token_env: String, client: reqwest::Client) -> Self {
        Self {
            token_env,
            client,
            cached: Mutex::new(None),
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Return a valid Copilot API token, refreshing if needed.
    async fn copilot_token(&self) -> Result<String> {
        // Fast path: check cache without doing any async work.
        {
            let guard = match self.cached.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    warn!("copilot token cache lock poisoned during read; recovering cached token state");
                    poisoned.into_inner()
                }
            };
            if let Some(ref c) = *guard {
                // Keep a 60-second safety margin before the stated expiry.
                if Self::now_secs() + 60 < c.expires_at {
                    return Ok(c.token.clone());
                }
            }
        }

        // Slow path: fetch a new token.
        // Support keyring:// references (e.g. "keyring://go-on/copilot_api_key")
        let github_token = if let Some(rest) = self.token_env.strip_prefix("keyring://") {
            // rest = "service/account"
            let mut parts = rest.splitn(2, '/');
            let service = parts.next().unwrap_or("go-on");
            let account = parts.next().unwrap_or("copilot_api_key");
            keyring::Entry::new(service, account)
                .and_then(|e| e.get_password())
                .with_context(|| format!("keyring lookup failed for {}", self.token_env))?
        } else {
            std::env::var(&self.token_env)
                .with_context(|| tf("error.copilot_env_not_set", &[("name", &self.token_env)]))?
        };
        let response = self
            .client
            .get(COPILOT_TOKEN_URL)
            .header("Authorization", format!("token {}", github_token))
            .header("Accept", "application/json")
            .header("User-Agent", "go-on/1.0")
            .send()
            .await
            .with_context(|| tf("error.copilot_token_endpoint_failed", &[]))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Copilot token refresh failed ({status}): {body}\n\
                 Hint: ensure GITHUB_TOKEN is a valid GitHub OAuth token with Copilot access."
            );
        }

        let body: Value = response
            .json()
            .await
            .with_context(|| tf("error.copilot_invalid_json", &[]))?;

        let token = body["token"]
            .as_str()
            .with_context(|| tf("error.copilot_missing_token", &[]))?
            .to_string();

        let expires_at = body["expires_at"].as_u64().unwrap_or_else(|| {
            // Default: treat token as valid for 25 minutes if field is absent.
            Self::now_secs() + 1500
        });

        {
            let mut guard = match self.cached.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    warn!("copilot token cache lock poisoned during write; recovering cached token state");
                    poisoned.into_inner()
                }
            };
            *guard = Some(CachedToken {
                token: token.clone(),
                expires_at,
            });
        }

        Ok(token)
    }

    fn merge_principles_into_messages(
        &self,
        mut messages: Vec<Message>,
        principles: Option<Vec<String>>,
    ) -> Vec<Message> {
        if let Some(items) = principles {
            if !items.is_empty() {
                let instruction = principles_to_text(&items);
                if let Some(first_user) = messages.iter_mut().find(|m| m.role == "user") {
                    first_user.content = format!("{}\n{}", instruction, first_user.content);
                } else {
                    messages.insert(
                        0,
                        Message {
                            role: "user".to_string(),
                            content: instruction,
                        },
                    );
                }
            }
        }

        messages
    }

    fn build_payload(
        &self,
        messages: Vec<Message>,
        options: Option<HashMap<String, Value>>,
    ) -> Value {
        let model = option_string(&options, "model").unwrap_or_else(|| "copilot".to_string());
        let temperature = option_f64(&options, "temperature");
        let max_tokens = option_u64(&options, "max_tokens");
        let top_p = option_f64(&options, "top_p");

        let mut payload = json!({
            "model": model,
            "messages": messages,
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

    fn is_quota_or_limit_error(message: &str) -> bool {
        let text = message.to_ascii_lowercase();
        text.contains("429")
            || text.contains("rate limit")
            || text.contains("quota")
            || text.contains("token") && text.contains("limit")
            || text.contains("insufficient_quota")
            || text.contains("billing")
            || text.contains("exceeded") && text.contains("limit")
    }

    fn should_try_free_model(options: &Option<HashMap<String, Value>>) -> bool {
        match option_string(options, "model") {
            None => true,
            Some(model) => {
                let model = model.to_ascii_lowercase();
                model == "auto" || model == "copilot"
            }
        }
    }

    fn with_free_model(options: &Option<HashMap<String, Value>>) -> Option<HashMap<String, Value>> {
        let mut next = options.clone().unwrap_or_default();
        let fallback_model = option_string(options, "copilot_fallback_model")
            .unwrap_or_else(|| "gpt-4o-mini".to_string());
        next.insert("model".to_string(), json!(fallback_model));
        Some(next)
    }

    async fn chat_once(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        let messages = self.merge_principles_into_messages(messages, principles);
        let api_token = self.copilot_token().await?;
        let payload = self.build_payload(messages, options);

        let response = self
            .client
            .post(COPILOT_COMPLETIONS_URL)
            .header("Authorization", format!("Bearer {api_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            // These headers identify the editor to GitHub's backend.
            .header("Editor-Version", "vscode/1.90.0")
            .header("Editor-Plugin-Version", "copilot-chat/0.17.0")
            .header("Copilot-Integration-Id", "vscode-chat")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                chat_request_failed_msg("copilot", &status.to_string(), &body)
            );
        }

        stream_sse_to_sender(response, sender).await
    }
}

#[async_trait]
impl Agent for CopilotAgent {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> crate::core::error::Result<()> {
        let mut last_error: Option<anyhow::Error> = None;
        let mut free_model_attempted = false;

        let mut active_options = options.clone();

        for attempt in 0..=2 {
            match self
                .chat_once(
                    messages.clone(),
                    principles.clone(),
                    active_options.clone(),
                    sender.clone(),
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    let err_text = err.to_string();
                    if !free_model_attempted
                        && Self::should_try_free_model(&active_options)
                        && Self::is_quota_or_limit_error(&err_text)
                    {
                        free_model_attempted = true;
                        active_options = Self::with_free_model(&active_options);
                        continue;
                    }
                    last_error = Some(err);
                    if attempt < 2 {
                        sleep(Duration::from_secs(1_u64 << attempt)).await;
                    }
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("{}", request_failed_msg("copilot")))
            .into())
    }

    fn available_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "copilot-chat".to_string(),
                name: "GitHub Copilot Chat".to_string(),
                description: "GitHub Copilot Chat model".to_string(),
                is_default: true,
                context_window: Some(16_384),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                ],
            },
            ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o via Copilot".to_string(),
                description: "GPT-4o accessed through GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(128_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                ],
            },
        ]
    }

    fn default_model(&self) -> Option<ModelInfo> {
        Some(ModelInfo {
            id: "copilot-chat".to_string(),
            name: "GitHub Copilot Chat".to_string(),
            description: "GitHub Copilot Chat model".to_string(),
            is_default: true,
            context_window: Some(16_384),
            capabilities: vec![
                "chat".to_string(),
                "code".to_string(),
                "streaming".to_string(),
            ],
        })
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

    fn agent() -> CopilotAgent {
        CopilotAgent::new("GITHUB_TOKEN".to_string(), reqwest::Client::new())
    }

    #[test]
    fn merge_principles_prefixes_first_user_message() {
        let merged = agent().merge_principles_into_messages(
            vec![
                message("system", "existing"),
                message("user", "implement feature"),
            ],
            Some(vec![
                "Use tests".to_string(),
                "Keep diffs small".to_string(),
            ]),
        );

        assert_eq!(merged.len(), 2);
        assert!(merged[1]
            .content
            .contains("Please follow these programming principles:"));
        assert!(merged[1].content.contains("implement feature"));
    }

    #[test]
    fn merge_principles_inserts_user_message_when_missing() {
        let merged = agent().merge_principles_into_messages(
            vec![message("assistant", "prior output")],
            Some(vec!["Use tests".to_string()]),
        );

        assert_eq!(merged[0].role, "user");
        assert!(merged[0].content.contains("Use tests"));
    }

    #[test]
    fn build_payload_applies_option_overrides() {
        let payload = agent().build_payload(
            vec![message("user", "hello")],
            Some(HashMap::from([
                ("model".to_string(), json!("copilot-chat")),
                ("temperature".to_string(), json!(0.2)),
                ("max_tokens".to_string(), json!(512)),
                ("top_p".to_string(), json!(0.9)),
            ])),
        );

        assert_eq!(payload["model"], "copilot-chat");
        assert_eq!(payload["messages"][0]["content"], "hello");
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["temperature"], 0.2);
        assert_eq!(payload["max_tokens"], 512);
        assert_eq!(payload["top_p"], 0.9);
    }
}
