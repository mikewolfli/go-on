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

use crate::agent::{resolve_secret, Agent, Message, ModelInfo};
use crate::agents::agent::{chat_request_failed_msg, is_non_retryable_4xx, request_failed_msg};
use crate::agents::{apply_openai_common_options, option_string, principles_to_text};
use crate::i18n::runtime::tf;
use crate::orchestration::autonomy_runtime::build_model_used_token;

const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_MODELS_URL: &str = "https://api.githubcopilot.com/models";
const COPILOT_COMPLETIONS_URL: &str = "https://api.githubcopilot.com/chat/completions";
const COPILOT_MODELS_CACHE_TTL_SECS: u64 = 300;

pub(crate) const COPILOT_FALLBACK_MODEL_PRIORITY: &[&str] = &[
    "claude-opus-4",
    "gemini-2.5-pro",
    "o3",
    "o1",
    "claude-sonnet-4",
    "gemini-2.0-flash-001",
    "gpt-4.1",
    "gpt-4o",
    "claude-3.5-sonnet",
    "o4-mini",
    "o3-mini",
    "gpt-5-mini",
    "gpt-4.1-mini",
    "gpt-4o-mini",
];

/// Cached Copilot API token with its expiry (Unix timestamp seconds).
struct CachedToken {
    token: String,
    expires_at: u64,
}

/// Cached ranked Copilot model IDs fetched from `/models`.
struct CachedModels {
    models: Vec<String>,
    fetched_at: u64,
}

pub struct CopilotAgent {
    /// Name of the environment variable holding the GitHub OAuth token.
    token_env: String,
    client: reqwest::Client,
    /// Short-lived Copilot API token, auto-refreshed.
    cached: Mutex<Option<CachedToken>>,
    /// Ranked Copilot model IDs discovered from GitHub's `/models` endpoint.
    cached_models: Mutex<Option<CachedModels>>,
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
            cached_models: Mutex::new(None),
        }
    }

    fn model_id_from_value(candidate: &Value) -> Option<String> {
        if let Some(raw) = candidate.as_str() {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
            return None;
        }

        let record = candidate.as_object()?;
        ["id", "model", "model_id", "name"]
            .iter()
            .find_map(|key| record.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    }

    fn bool_field(record: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
        keys.iter()
            .any(|key| record.get(*key).and_then(Value::as_bool).unwrap_or(false))
    }

    fn capability_score(record: &serde_json::Map<String, Value>) -> i64 {
        let mut score = 0i64;

        if let Some(capabilities) = record.get("capabilities") {
            if let Some(items) = capabilities.as_array() {
                for capability in items.iter().filter_map(Value::as_str) {
                    let cap = capability.to_ascii_lowercase();
                    if cap.contains("reason") {
                        score += 180;
                    }
                    if cap.contains("tool") || cap.contains("function") {
                        score += 140;
                    }
                    if cap.contains("vision") {
                        score += 120;
                    }
                    if cap.contains("chat") || cap.contains("code") {
                        score += 80;
                    }
                }
            }
        }

        if let Some(window) = record
            .get("context_window")
            .or_else(|| record.get("contextWindow"))
            .and_then(Value::as_u64)
        {
            score += (window / 16_000).min(80) as i64;
        }

        score
    }

    fn fallback_rank(model_id: &str) -> i64 {
        let model = model_id.to_ascii_lowercase();
        let total = COPILOT_FALLBACK_MODEL_PRIORITY.len() as i64;
        for (idx, known) in COPILOT_FALLBACK_MODEL_PRIORITY.iter().enumerate() {
            if model == *known {
                return (total - idx as i64) * 500;
            }
        }
        0
    }

    pub(crate) fn extract_ranked_model_ids(payload: &Value) -> Vec<String> {
        let root = payload.as_object();
        let candidates = if let Some(array) = payload.as_array() {
            array.clone()
        } else if let Some(array) = root
            .and_then(|obj| obj.get("data"))
            .and_then(Value::as_array)
        {
            array.clone()
        } else if let Some(array) = root
            .and_then(|obj| obj.get("models"))
            .and_then(Value::as_array)
        {
            array.clone()
        } else {
            Vec::new()
        };

        let mut ranked: Vec<(String, i64, usize)> = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let Some(model_id) = Self::model_id_from_value(candidate) else {
                continue;
            };
            let id_lower = model_id.to_ascii_lowercase();

            let mut score = Self::fallback_rank(&id_lower);
            if let Some(record) = candidate.as_object() {
                if Self::bool_field(record, &["is_default", "default", "default_model"]) {
                    score += 10_000;
                }
                if Self::bool_field(record, &["recommended", "is_recommended", "preferred"]) {
                    score += 8_000;
                }
                score += Self::capability_score(record);
            }

            ranked.push((model_id, score, index));
        }

        ranked.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.0.cmp(&right.0))
        });

        let mut dedup = std::collections::HashSet::new();
        ranked
            .into_iter()
            .filter_map(|(model, _, _)| {
                if dedup.insert(model.clone()) {
                    Some(model)
                } else {
                    None
                }
            })
            .collect()
    }

    fn fresh_cached_models(&self) -> Option<Vec<String>> {
        let guard = match self.cached_models.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("copilot model cache lock poisoned during read; recovering state");
                poisoned.into_inner()
            }
        };
        let cached = guard.as_ref()?;
        if Self::now_secs().saturating_sub(cached.fetched_at) <= COPILOT_MODELS_CACHE_TTL_SECS {
            return Some(cached.models.clone());
        }
        None
    }

    fn stale_cached_models(&self) -> Option<Vec<String>> {
        let guard = match self.cached_models.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("copilot model cache lock poisoned during stale read; recovering state");
                poisoned.into_inner()
            }
        };
        guard.as_ref().map(|cached| cached.models.clone())
    }

    fn store_cached_models(&self, models: Vec<String>) {
        let mut guard = match self.cached_models.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("copilot model cache lock poisoned during write; recovering state");
                poisoned.into_inner()
            }
        };
        *guard = Some(CachedModels {
            models,
            fetched_at: Self::now_secs(),
        });
    }

    async fn fetch_ranked_models_from_network(&self) -> Result<Vec<String>> {
        let api_token = self.copilot_token().await?;
        let response = self
            .client
            .get(COPILOT_MODELS_URL)
            .header("Authorization", format!("Bearer {api_token}"))
            .header("Accept", "application/json")
            .header("User-Agent", "go-on/1.0")
            .header("Editor-Version", "vscode/1.90.0")
            .header("Editor-Plugin-Version", "copilot-chat/0.17.0")
            .header("Copilot-Integration-Id", "copilot-chat")
            .send()
            .await
            .with_context(|| "copilot models endpoint request failed".to_string())?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Copilot models request failed ({status}): {body}");
        }

        let payload: Value = response
            .json()
            .await
            .with_context(|| "copilot models endpoint returned invalid json".to_string())?;
        let models = Self::extract_ranked_model_ids(&payload);
        if models.is_empty() {
            anyhow::bail!("Copilot models endpoint returned no model identifiers");
        }
        Ok(models)
    }

    async fn resolve_auto_model_candidates(&self) -> Vec<String> {
        if let Some(models) = self.fresh_cached_models() {
            return models;
        }

        match self.fetch_ranked_models_from_network().await {
            Ok(models) => {
                self.store_cached_models(models.clone());
                models
            }
            Err(err) => {
                warn!(
                    "copilot auto model resolution fell back after /models failure: {}",
                    err
                );
                if let Some(models) = self.stale_cached_models() {
                    return models;
                }
                COPILOT_FALLBACK_MODEL_PRIORITY
                    .iter()
                    .map(|model| (*model).to_string())
                    .collect()
            }
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
        // Use shared resolve_secret() which handles env vars and keyring:// references
        // with secret pooling, rotation, and security validation.
        let github_token = resolve_secret(&self.token_env, "copilot.token_env")?;
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
        messages: &[Message],
        principles: &Option<Vec<String>>,
    ) -> Vec<Message> {
        if let Some(items) = principles {
            if !items.is_empty() {
                let instruction = principles_to_text(items);
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
                return owned;
            }
        }

        messages.to_vec()
    }

    fn build_payload(
        &self,
        messages: Vec<Message>,
        options: &Option<HashMap<String, Value>>,
    ) -> Value {
        // ── Model handling ────────────────────────────────────────────
        // VS Code Copilot extension resolves "auto" to a concrete model
        // before sending the request.  The resolution is based on the
        // user's subscription tier (fetched from the Copilot /models API).
        //
        // Go-on passes the resolved model through, or defaults to "gpt-4o"
        // (the safest fallback that all Copilot tiers support).
        let model = option_string(options, "model").unwrap_or_default();
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
        let merged = self.merge_principles_into_messages(messages, principles);
        let api_token = self.copilot_token().await?;
        let payload = self.build_payload(merged, options);

        let response = self
            .client
            .post(COPILOT_COMPLETIONS_URL)
            .header("Authorization", format!("Bearer {api_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            // These headers identify the editor to GitHub's backend.
            .header("Editor-Version", "vscode/1.90.0")
            .header("Editor-Plugin-Version", "copilot-chat/0.17.0")
            .header("Copilot-Integration-Id", "copilot-chat")
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

        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("text/event-stream") && !ct.starts_with("application/json") {
            tracing::warn!("copilot: unexpected content-type: {ct}");
            anyhow::bail!("unexpected content-type: {ct}");
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
                let mut m = capture.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("lock poisoned, recovering");
                    poisoned.into_inner()
                });
                if m.is_none() {
                    if let Some(model_name) = json.get("model").and_then(|v| v.as_str()) {
                        if !model_name.is_empty() {
                            *m = Some(model_name.to_string());
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
        let mutex = match actual_model.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("[B48] actual_model lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        if let Some(ref model_name) = *mutex {
            if !model_name.is_empty() {
                let _ = sender.send(build_model_used_token(model_name));
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
            // VS Code-aligned flow: resolve models from Copilot `/models`
            // and use ranked/filtered candidates with graceful fallback.
            self.resolve_auto_model_candidates().await
        } else {
            vec![current_model.clone()]
        };

        let mut last_error: Option<anyhow::Error> = None;
        let chat_messages = messages;

        'models: for model_id in &candidates {
            for attempt in 0..=2 {
                let mut model_opts = options.clone().unwrap_or_default();
                model_opts.insert("model".to_string(), json!(model_id));
                let model_options = Some(model_opts);

                match self
                    .chat_once(&chat_messages, &principles, &model_options, sender.clone())
                    .await
                {
                    Ok(()) => {
                        // model name captured from SSE inside chat_once
                        return Ok(());
                    }
                    Err(err) => {
                        let err_text = err.to_string();
                        let err_text_lower = err_text.to_ascii_lowercase();

                        // Non-retryable 4xx (except 429) → skip model or fail
                        if is_non_retryable_4xx(&err_text_lower) {
                            if is_auto {
                                continue 'models;
                            }
                            return Err(err.into());
                        }

                        // Unsupported model (non-retryable) → skip model or fail
                        if err_text_lower.contains("model_not_supported")
                            || err_text_lower.contains("not supported")
                        {
                            if is_auto {
                                // Try next model
                                continue 'models;
                            }
                            // Non-auto: fail immediately
                            return Err(err.into());
                        }

                        // Quota/rate-limit (transient) → retry with backoff
                        if err_text_lower.contains("429")
                            || err_text_lower.contains("rate limit")
                            || err_text_lower.contains("quota")
                            || err_text_lower.contains("insufficient_quota")
                        {
                            if is_auto {
                                // For auto mode with multiple candidates,
                                // still try next model after exhausting retries
                                if attempt < 2 {
                                    last_error = Some(err);
                                    sleep(Duration::from_secs(1u64 << attempt)).await;
                                    continue;
                                }
                                continue 'models;
                            }
                            // Non-auto: retry with backoff
                            if attempt < 2 {
                                last_error = Some(err);
                                sleep(Duration::from_secs(1u64 << attempt)).await;
                            } else {
                                last_error = Some(err);
                            }
                            continue;
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
                id: "gpt-4.1".to_string(),
                name: "GPT-4.1".to_string(),
                description: "OpenAI GPT-4.1 (1M context) via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(1_000_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "gpt-4.1-mini".to_string(),
                name: "GPT-4.1 Mini".to_string(),
                description: "OpenAI GPT-4.1 Mini (1M context) via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(1_000_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
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
                id: "claude-opus-4-7".to_string(),
                name: "Claude Opus 4.7".to_string(),
                description: "Anthropic Claude Opus 4.7 via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(1_000_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "gemini-2.5-flash".to_string(),
                name: "Gemini 2.5 Flash".to_string(),
                description: "Google Gemini 2.5 Flash via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(1_000_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "gemini-2.5-pro".to_string(),
                name: "Gemini 2.5 Pro".to_string(),
                description: "Google Gemini 2.5 Pro via GitHub Copilot".to_string(),
                is_default: false,
                context_window: Some(1_000_000),
                capabilities: vec![
                    "chat".to_string(),
                    "code".to_string(),
                    "streaming".to_string(),
                    "tools".to_string(),
                ],
            },
            ModelInfo {
                id: "o4-mini".to_string(),
                name: "o4-mini".to_string(),
                description: "OpenAI o4-mini via GitHub Copilot".to_string(),
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
            &[
                message("system", "existing"),
                message("user", "implement feature"),
            ],
            &Some(vec![
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
            &[message("assistant", "prior output")],
            &Some(vec!["Use tests".to_string()]),
        );

        assert_eq!(merged[0].role, "user");
        assert!(merged[0].content.contains("Use tests"));
    }

    #[test]
    fn build_payload_applies_option_overrides() {
        let payload = agent().build_payload(
            vec![message("user", "hello")],
            &Some(HashMap::from([
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

    #[test]
    fn extract_ranked_model_ids_prefers_default_and_recommended() {
        let payload = json!({
            "data": [
                {"id": "gpt-4o", "capabilities": ["chat"]},
                {"id": "gpt-5", "recommended": true, "capabilities": ["chat", "tools"]},
                {"id": "claude-sonnet-4", "is_default": true, "capabilities": ["chat", "tools", "reasoning"]}
            ]
        });

        let models = CopilotAgent::extract_ranked_model_ids(&payload);
        assert_eq!(models.first().map(String::as_str), Some("claude-sonnet-4"));
        assert!(models.iter().any(|m| m == "gpt-5"));
        assert!(models.iter().any(|m| m == "gpt-4o"));
    }

    #[test]
    fn extract_ranked_model_ids_dedups_and_accepts_string_list() {
        let payload = json!(["gpt-4o", "gpt-4o", "gpt-5"]);
        let models = CopilotAgent::extract_ranked_model_ids(&payload);
        // Both models have same fallback rank, so ordering is by original index
        assert_eq!(models.len(), 2, "should dedup to 2 unique models");
        assert!(models.contains(&"gpt-4o".to_string()));
        assert!(models.contains(&"gpt-5".to_string()));
    }
}
