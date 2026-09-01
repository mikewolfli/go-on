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
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::sleep;
use tracing::warn;

use crate::agent::{resolve_secret, Agent, Message, ModelInfo};
use crate::agents::agent::{is_non_retryable_4xx, request_failed_msg, retry_backoff_secs};
use crate::agents::{apply_openai_common_options, check_api_response, option_string};
use crate::i18n::runtime::tf;
use crate::orchestration::autonomy_runtime::build_model_used_token;

/// Apply the retry-with-backoff decision for a transient failure.
///
/// Returns `true` when the same attempt budget remains (backoff sleep
/// applied — retry the same model), `false` when the 3-attempt budget is
/// exhausted (the caller moves to the next candidate / returns the recorded
/// error). Previously the copilot chat loop inlined this decision in four
/// near-identical branches.
async fn retry_or_exhaust(attempt: u64) -> bool {
    if attempt < 2 {
        sleep(Duration::from_secs(retry_backoff_secs(attempt as u32))).await;
        true
    } else {
        false
    }
}

pub(crate) const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
pub(crate) const COPILOT_MODELS_URL: &str = "https://api.githubcopilot.com/models";
const COPILOT_COMPLETIONS_URL: &str = "https://api.githubcopilot.com/chat/completions";
pub(crate) const COPILOT_MODELS_CACHE_TTL_SECS: u64 = 300;

/// Default GitHub Copilot OAuth device-flow client id (public, non-secret;
/// see BLUE65-K3). Overridable via `GO_ON_COPILOT_CLIENT_ID` — the same env
/// override the vscode-addon uses.
pub(crate) const DEFAULT_COPILOT_CLIENT_ID: &str = "01ab8ac9400c4e429b23";

/// GitHub OAuth device-code initiation endpoint.
pub(crate) const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
/// GitHub device verification page shown to the user.
pub(crate) const GITHUB_DEVICE_VERIFY_URL: &str = "https://github.com/login/device";
/// GitHub OAuth token-exchange endpoint (device flow poll).
pub(crate) const GITHUB_OAUTH_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// Resolve the Copilot OAuth client id: `GO_ON_COPILOT_CLIENT_ID` env wins,
/// otherwise the public default.
pub(crate) fn copilot_client_id() -> &'static str {
    // SAFETY: the env value, when present, is leaked for a process-lifetime
    // static — acceptable for a startup-read configuration value.
    match std::env::var("GO_ON_COPILOT_CLIENT_ID") {
        Ok(v) if !v.trim().is_empty() => Box::leak(v.trim().to_string().into_boxed_str()),
        _ => DEFAULT_COPILOT_CLIENT_ID,
    }
}

/// Editor-identifying headers GitHub's Copilot backend expects. Single
/// definition shared by the Copilot agent and the runtime pack's model
/// discovery so the identity cannot drift between the two surfaces.
pub(crate) const COPILOT_EDITOR_HEADERS: [(&str, &str); 3] = [
    ("Editor-Version", "vscode/1.90.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.17.0"),
    ("Copilot-Integration-Id", "copilot-chat"),
];

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

/// Cached ranked Copilot model IDs fetched from `/models`.
struct CachedModels {
    models: Vec<String>,
    fetched_at: u64,
}

pub struct CopilotAgent {
    /// Name of the environment variable holding the GitHub OAuth token.
    token_env: String,
    client: reqwest::Client,
    /// Short-lived Copilot API token, auto-refreshed (shared TokenCache).
    cached: crate::agents::TokenCache,
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
            cached: crate::agents::TokenCache::new(),
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
        if crate::agents::unix_now_secs().saturating_sub(cached.fetched_at)
            <= COPILOT_MODELS_CACHE_TTL_SECS
        {
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
            fetched_at: crate::agents::unix_now_secs(),
        });
    }

    async fn fetch_ranked_models_from_network(&self) -> Result<Vec<String>> {
        let api_token = self.copilot_token().await?;
        let response = self
            .client
            .get(COPILOT_MODELS_URL)
            .header("Authorization", format!("Bearer {api_token}"))
            .header("Accept", "application/json")
            .header("User-Agent", crate::shared::http_client::USER_AGENT);
        let response = {
            let mut req = response;
            for (name, value) in COPILOT_EDITOR_HEADERS {
                req = req.header(name, value);
            }
            req
        }
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

    /// Return a valid Copilot API token, refreshing if needed.
    async fn copilot_token(&self) -> Result<String> {
        // Fast path: check the shared cache without any async work (60s margin).
        if let Some(token) = self.cached.fresh(60) {
            return Ok(token);
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
            .header("User-Agent", crate::shared::http_client::USER_AGENT)
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
            crate::agents::unix_now_secs() + 1500
        });

        self.cached.store(token.clone(), expires_at);

        Ok(token)
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
}

#[async_trait]
impl Agent for CopilotAgent {
    async fn chat_once(
        &self,
        messages: &[Message],
        principles: &Option<Vec<String>>,
        options: &Option<HashMap<String, Value>>,
        sender: crate::agent::StreamingSender,
    ) -> anyhow::Result<()> {
        // Copilot has no system role; prepend principles to the first user
        // message via the shared helper.
        let merged = crate::agents::merge_principles_into_messages(messages, principles, false);
        let api_token = self.copilot_token().await?;
        let payload = self.build_payload(merged, options);

        let mut request = self
            .client
            .post(COPILOT_COMPLETIONS_URL)
            .header("Authorization", format!("Bearer {api_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
        // These headers identify the editor to GitHub's backend (shared
        // constant — same identity as the models/token discovery calls).
        for (name, value) in COPILOT_EDITOR_HEADERS {
            request = request.header(name, value);
        }
        let response = request.json(&payload).send().await?;

        let response = check_api_response(response, "copilot").await?;

        // Stream the SSE response and capture the actual model name.
        // OpenAI-compatible streaming responses include the "model" field
        // in every SSE data event.  We capture the first non-empty one.
        let actual_model = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let capture = actual_model.clone();
        let stream_sender = sender.clone();
        // Per-stream tool-call accumulator (dropped on every stream end path;
        // see ToolCallAccumulator in agents/mod.rs).
        let mut tool_acc = crate::agents::ToolCallAccumulator::default();

        crate::agents::stream_sse_events(response, move |data| {
            use crate::agents::SseEventAction;

            // Capture the model name from the first event that carries it.
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
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
            }

            // Reuse the shared [DONE]/token-extraction handling instead of
            // reimplementing it inline.
            if crate::agents::sse_event_to_sender(data, &stream_sender, &mut tool_acc) {
                Ok(SseEventAction::Stop)
            } else {
                Ok(SseEventAction::Continue)
            }
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

        'models: for model_id in &candidates {
            for attempt in 0u64..=2 {
                let mut model_opts = options.clone().unwrap_or_default();
                model_opts.insert("model".to_string(), json!(model_id));
                let model_options = Some(model_opts);

                match self
                    .chat_once(&messages, &principles, &model_options, sender.clone())
                    .await
                {
                    Ok(()) => {
                        // model name captured from SSE inside chat_once
                        return Ok(());
                    }
                    Err(err) => {
                        let err_text = err.to_string();
                        let err_text_lower = err_text.to_ascii_lowercase();

                        // Fatal errors (non-retryable 4xx except 429, and
                        // unsupported-model responses): auto mode moves to the
                        // next candidate, an explicitly configured model fails
                        // now. (The 4xx and unsupported branches were
                        // byte-identical and are merged.)
                        if is_non_retryable_4xx(&err_text_lower)
                            || err_text_lower.contains("model_not_supported")
                            || err_text_lower.contains("not supported")
                        {
                            if is_auto {
                                continue 'models;
                            }
                            return Err(err.into());
                        }

                        // Rate-limit / other transient errors: retry the same
                        // model with backoff for up to 3 attempts, then move to
                        // the next candidate (auto) or exhaust the single
                        // candidate (explicit). Rate-limit and generic
                        // transient handling are behaviorally identical, so
                        // they share one branch.
                        last_error = Some(err);
                        if retry_or_exhaust(attempt).await {
                            continue;
                        }
                        if is_auto {
                            continue 'models;
                        }
                        // Non-auto: attempt budget exhausted; `last_error` is
                        // returned after the loops.
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
        let merged = crate::agents::merge_principles_into_messages(
            &[
                message("system", "existing"),
                message("user", "implement feature"),
            ],
            &Some(vec![
                "Use tests".to_string(),
                "Keep diffs small".to_string(),
            ]),
            false,
        );

        assert_eq!(merged.len(), 2);
        assert!(merged[1]
            .content
            .contains("Please follow these programming principles:"));
        assert!(merged[1].content.contains("implement feature"));
    }

    #[test]
    fn merge_principles_inserts_user_message_when_missing() {
        let merged = crate::agents::merge_principles_into_messages(
            &[message("assistant", "prior output")],
            &Some(vec!["Use tests".to_string()]),
            false,
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
