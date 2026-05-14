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
    apply_openai_common_options, option_string, principles_to_text, stream_sse_to_sender,
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
        // ── Model handling ────────────────────────────────────────────
        // VS Code Copilot extension resolves "auto" to a concrete model
        // before sending the request.  The resolution is based on the
        // user's subscription tier (fetched from the Copilot /models API).
        //
        // Go-on passes the resolved model through, or defaults to "gpt-4o"
        // (the safest fallback that all Copilot tiers support).
        let model = option_string(&options, "model").unwrap_or_default();
        let mapped_model = match model.as_str() {
            "" | "auto" | "copilot/auto" | "copilot-auto" | "copilot" | "github-copilot" => {
                "gpt-4o"
            }
            other => other,
        };

        let mut payload = json!({
            "model": mapped_model,
            "messages": messages,
            "stream": true
        });

        apply_openai_common_options(&mut payload, &options);

        payload
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

        // Stream the SSE response and capture the actual model name.
        // OpenAI-compatible streaming responses include the "model" field
        // in every SSE data event.  We capture the first non-empty one.
        let actual_model = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let capture = actual_model.clone();
        let stream_sender = sender.clone();

        crate::agents::stream_sse_events(response, move |data| {
            use crate::agents::SseEventAction;

            if data.trim() == "[DONE]" {
                return Ok(SseEventAction::Stop);
            }

            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                // Capture model name from the first event that has it.
                if let Ok(mut m) = capture.lock() {
                    if m.is_none() {
                        if let Some(model_name) = json.get("model").and_then(|v| v.as_str()) {
                            if !model_name.is_empty() {
                                *m = Some(model_name.to_string());
                            }
                        }
                    }
                }

                if let Some(token) = crate::agents::extract_token(&json) {
                    if stream_sender.send(token).is_err() {
                        return Ok(SseEventAction::Stop);
                    }
                }
            }

            Ok(SseEventAction::Continue)
        })
        .await?;

        // Notify the caller about the actual model used.
        if let Ok(mutex) = actual_model.lock() {
            if let Some(ref model_name) = *mutex {
                if !model_name.is_empty() {
                    let _ = sender.send(format!("__model_used__:{}", model_name));
                }
            }
        }

        Ok(())
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
        // ── Copilot auto model selection ──────────────────────────────
        // VS Code / GitHub Copilot (official) pre-resolves "Auto" to the
        // best available model for the user's subscription tier and sends
        // that concrete model ID to the Copilot API.  There is no "let the
        // server decide" mode — the client always chooses.
        //
        // Go-on replicates this: when options.model is copilot-auto / auto
        // / empty, we pick the highest-capability model from our known list
        // and try it.  On failure (unsupported, quota) we fall through to
        // the next best.  The actual model from the successful response is
        // captured from the SSE model field and sent back as __model_used__.

        // Determine which concrete model IDs to try.
        let current_model = option_string(&options, "model").unwrap_or_default();
        let is_auto = current_model.is_empty()
            || current_model.eq_ignore_ascii_case("auto")
            || current_model.eq_ignore_ascii_case("copilot/auto")
            || current_model.eq_ignore_ascii_case("copilot-auto")
            || current_model.eq_ignore_ascii_case("copilot");

        let candidates: Vec<String> = if is_auto {
            // All known Copilot models, ordered by capability descending.
            // The first one that returns a successful response wins.
            [
                // Ultra-premium (Pro+, Enterprise)
                "claude-opus-4",
                "gemini-2.5-pro",
                // Flagship reasoning
                "o1",
                "o3",
                // Top-tier chat
                "gpt-5",
                "claude-sonnet-4",
                "gemini-2.0-flash-001",
                // Premium
                "gpt-4.1",
                "gpt-4o",
                "claude-3.5-sonnet",
                // Lightweight reasoning
                "o3-mini",
                "o4-mini",
                // Budget / fallback
                "gpt-4.1-mini",
                "gpt-4o-mini",
                "gpt-5-mini",
            ]
            .iter()
            .map(|&s| s.to_string())
            .collect()
        } else {
            vec![current_model.clone()]
        };

        let mut last_error: Option<anyhow::Error> = None;

        'models: for model_id in &candidates {
            for attempt in 0..=2 {
                let mut model_opts = options.clone().unwrap_or_default();
                model_opts.insert("model".to_string(), json!(model_id));
                let model_options = Some(model_opts);

                match self
                    .chat_once(
                        messages.clone(),
                        principles.clone(),
                        model_options,
                        sender.clone(),
                    )
                    .await
                {
                    Ok(()) => {
                        // model name captured from SSE inside chat_once
                        return Ok(());
                    }
                    Err(err) => {
                        let err_text = err.to_string().to_ascii_lowercase();
                        // Unsupported model, quota, rate-limit → skip model
                        if err_text.contains("model_not_supported")
                            || err_text.contains("not supported")
                            || err_text.contains("429")
                            || err_text.contains("rate limit")
                            || err_text.contains("quota")
                            || err_text.contains("insufficient_quota")
                        {
                            if is_auto {
                                // Try next model
                                continue 'models;
                            }
                            // Non-auto: fail immediately
                            return Err(err.into());
                        }
                        // Transient error → retry
                        if attempt < 2 {
                            last_error = Some(err);
                            sleep(Duration::from_secs(1u64 << attempt)).await;
                        } else {
                            last_error = Some(err);
                        }
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
                id: "copilot/auto".to_string(),
                name: "Auto (best model)".to_string(),
                description: "GitHub Copilot auto model selection".to_string(),
                is_default: true,
                context_window: Some(128_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                description: "OpenAI GPT-4o via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(128_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                ],
            },
            ModelInfo {
                id: "gpt-4o-mini".to_string(),
                name: "GPT-4o Mini".to_string(),
                description: "OpenAI GPT-4o Mini via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(128_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                ],
            },
            ModelInfo {
                id: "gpt-5".to_string(),
                name: "GPT-5".to_string(),
                description: "OpenAI GPT-5 via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(128_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "gpt-5-mini".to_string(),
                name: "GPT-5 Mini".to_string(),
                description: "OpenAI GPT-5 Mini via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(128_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "claude-sonnet-4".to_string(),
                name: "Claude Sonnet 4".to_string(),
                description: "Anthropic Claude Sonnet 4 via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(200_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "claude-3.5-sonnet".to_string(),
                name: "Claude 3.5 Sonnet".to_string(),
                description: "Anthropic Claude 3.5 Sonnet via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(200_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "claude-opus-4".to_string(),
                name: "Claude Opus 4".to_string(),
                description: "Anthropic Claude Opus 4 via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(200_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "gemini-2.0-flash-001".to_string(),
                name: "Gemini 2.0 Flash".to_string(),
                description: "Google Gemini 2.0 Flash via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(1_000_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                ],
            },
            ModelInfo {
                id: "o1".to_string(),
                name: "OpenAI o1".to_string(),
                description: "OpenAI o1 reasoning model via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(200_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "reasoning".to_string(),
                ],
            },
            ModelInfo {
                id: "o3-mini".to_string(),
                name: "OpenAI o3-mini".to_string(),
                description: "OpenAI o3-mini reasoning model via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(200_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "reasoning".to_string(),
                ],
            },
        ]
    }

    fn default_model(&self) -> Option<ModelInfo> {
        Some(ModelInfo {
            id: "copilot/auto".to_string(),
            name: "Auto (best model)".to_string(),
            description: "GitHub Copilot auto model selection".to_string(),
            is_default: true,
            context_window: Some(128_000),
            capabilities: vec![
                "chat".to_string(),
                "code".to_string(),
                "streaming".to_string(),
                "tools".to_string(),
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
