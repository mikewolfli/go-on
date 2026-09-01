//! Agent system implementation
//!
//! This module defines the Agent trait, AgentRegistry, and related functionality
//! for managing and interacting with different AI agents.
//! These structures define task contracts, audit schemas, and agent interfaces
//! used by the ACP runtime, orchestration, and CLI entry points.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::i18n::runtime::tf;
use crate::intelligence::capability_graph::{CapabilityDecl, CapabilityGraph};
use crate::intelligence::token_cache::{CachedAgentWrapper, TokenMultiLevelCache as TokenCache};

/// Default OpenAI-compatible chat completions path.
const DEFAULT_OPENAI_CHAT_PATH: &str = "/v1/chat/completions";
/// Default OpenAI-compatible chat completions path for providers that expose
/// the endpoint at the root (DeepSeek, Doubao, Qwen, Groq family, …).
const DEFAULT_ROOT_CHAT_PATH: &str = "/chat/completions";

/// Pick the default chat path for a base URL that may already include a `/v1`
/// prefix (e.g. `https://api.openai.com/v1`, `https://api.siliconflow.cn/v1`).
/// Joining a fixed `/v1/chat/completions` onto such a base produced a double
/// `/v1` (`…/v1/v1/chat/completions`); providers that expose the endpoint at
/// the root must use `/chat/completions` instead. Explicit `chat_path` config
/// always wins (it bypasses this helper).
fn default_openai_chat_path(base_url: &str) -> &'static str {
    if base_url.trim_end_matches('/').ends_with("/v1") {
        DEFAULT_ROOT_CHAT_PATH
    } else {
        DEFAULT_OPENAI_CHAT_PATH
    }
}

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::{debug, warn};

use crate::agents::{
    AnthropicAgent, BaiduErnieAgent, CohereAgent, CopilotAgent, DeepSeekAgent, ErnieApi,
    GeminiAgent, OpenAiCompatibleAgent,
};
use crate::core::error::Result as AppResult;

use crate::config::{AgentConfig, AppConfig};
use crate::pua::PuaExecutionReport;

/// Return an error message for a failed provider chat request.
/// Tries i18n first, falls back to hardcoded English template so errors
/// are always readable even when the i18n system is not yet initialized.
pub fn chat_request_failed_msg(provider: &str, status: &str, body: &str) -> String {
    let msg = crate::i18n::runtime::tf(
        "error.agent_chat_failed",
        &[("provider", provider), ("status", status), ("body", body)],
    );
    // If tf() returned the raw key, i18n is not available — use hardcoded fallback
    if msg == "error.agent_chat_failed" {
        format!("{} chat request failed with {}: {}", provider, status, body)
    } else {
        msg
    }
}

/// Return an error message for a failed provider request (fallback, no status).
/// Tries i18n first, falls back to hardcoded English.
pub fn request_failed_msg(provider: &str) -> String {
    let msg = crate::i18n::runtime::tf("error.request_failed", &[("provider", provider)]);
    if msg == "error.request_failed" {
        format!("{} request failed", provider)
    } else {
        msg
    }
}

/// Return an error message for a token request failure.
/// Tries i18n first, falls back to hardcoded English.
pub fn token_request_failed_msg(provider: &str, status: &str, body: &str) -> String {
    let msg = crate::i18n::runtime::tf(
        "error.agent_token_failed",
        &[("provider", provider), ("status", status), ("body", body)],
    );
    if msg == "error.agent_token_failed" {
        format!(
            "{} token request failed with {}: {}",
            provider, status, body
        )
    } else {
        msg
    }
}

/// Retry a `chat_once` call up to `max_attempts` times with exponential backoff.
///
/// Returns early on success or on non-retryable 4xx errors.
/// Shared by all agent providers to eliminate ~30 duplicate retry loops.
pub async fn retry_chat_once<F, Fut, T>(
    mut chat_once: F,
    max_attempts: usize,
) -> crate::core::error::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = crate::core::error::Result<T>>,
{
    use crate::core::error::AppError;
    let mut last_error: Option<AppError> = None;
    for attempt in 0..max_attempts {
        match chat_once().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                let err_msg = err.to_string();
                if is_non_retryable_4xx(&err_msg) {
                    return Err(err);
                }
                last_error = Some(err);
                if attempt + 1 < max_attempts {
                    // Canonical capped exponential backoff (see
                    // [`retry_backoff_secs`]); the previous inline
                    // `1 << attempt` grew without bound.
                    sleep(Duration::from_secs(retry_backoff_secs(attempt as u32))).await;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AppError::Proxy(crate::core::error::ProxyError::Internal(format!(
            "chat_once failed after {max_attempts} attempts"
        )))
    }))
}

/// Check if an error message indicates a non-retryable HTTP 4xx status
/// (excluding 429 rate limit). This prevents wasting time retrying requests
/// that will never succeed (e.g. 400, 401, 403).
pub fn is_non_retryable_4xx(msg: &str) -> bool {
    // Error messages from chat_request_failed_msg include the status code.
    // Look for " 4XX " pattern and exclude 429.
    let bytes = msg.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 3 < len {
        if bytes[i] == b' '
            && bytes[i + 1] == b'4'
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            // 429 rate limit is retryable
            if bytes[i + 2] == b'2' && bytes[i + 3] == b'9' {
                return false;
            }
            return true;
        }
        i += 1;
    }
    false
}

/// Maximum exponential backoff delay in seconds before a retry.
///
/// Aligned with the declared retry-policy contract (`max_delay_ms: 10_000`)
/// in `governance_pack.rs` / `runtime_pack.rs`.
pub(crate) const MAX_RETRY_BACKOFF_SECS: u64 = 10;

/// Detect a rate-limit / quota / transient-throttling error message.
///
/// Canonical detector shared by the agent retry paths (previously duplicated
/// inline in `copilot.rs` and `skill/execution.rs` with slightly different
/// keyword sets). Matches HTTP 429 status codes plus the common textual
/// rate-limit markers.
pub(crate) fn is_rate_limit_error(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("429")
        || lower.contains("rate limit")
        || lower.contains("quota")
        || lower.contains("insufficient_quota")
        || lower.contains("too many requests")
        || lower.contains("retry after")
}

/// Exponential backoff delay in seconds for retry `attempt` (0-based):
/// 1s, 2s, 4s, 8s, … capped at [`MAX_RETRY_BACKOFF_SECS`].
///
/// Single canonical backoff schedule for all retry loops (previously
/// `backoff_secs` in copilot.rs and an inline `1 << (attempt - 1)` in
/// `skill/execution.rs`).
pub(crate) fn retry_backoff_secs(attempt: u32) -> u64 {
    // `1u64 << attempt` panics for attempt >= 64 and the `.min()` cap applies
    // after the shift; `checked_shl` makes the shift itself overflow-safe.
    1u64.checked_shl(attempt)
        .unwrap_or(u64::MAX)
        .min(MAX_RETRY_BACKOFF_SECS)
}

/// Agent task envelope (Phase 0/1 discipline)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskEnvelope {
    pub task_id: String,
    pub phase: String,
    pub role: String,
    pub objective: String,
    pub constraints: Option<String>,
    pub evidence: Option<String>,
    pub input: serde_json::Value,
}

/// Agent output schema
/// Unified agent error type
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "info")]
pub enum AgentError {
    #[error("Agent runtime error: {0}")]
    Runtime(String),
}

impl From<anyhow::Error> for AgentError {
    fn from(e: anyhow::Error) -> Self {
        AgentError::Runtime(format!("{}", e))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskResult {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<AgentError>,
    pub audit_log: Option<String>,
    pub pua_report: Option<PuaExecutionReport>,
}

/// Agent decision audit log schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAuditLog {
    pub agent: String,
    pub phase: String,
    pub task_id: String,
    pub decision: String,
    pub rationale: Option<String>,
    pub timestamp: String,
}

/// Keyring prefix for secret references
///
/// Shared with `acp::helpers::planning::context` via `shared::keyring_ref`.
///
/// # SECURITY POLICY
///
/// ALL API keys MUST be stored in the system keyring exclusively.
///   - GUI stores via keyring crate (libsecret/Keychain/Credential Manager)
///   - Backend reads via keyring:// URIs in config.toml
///   - NO .env files, NO plaintext storage, NO process env leakage
///
/// The env var fallback in load_secret_value() exists ONLY for advanced
/// users who deliberately export env vars (e.g., CI/CD pipelines).
/// Do NOT add .env file loading here.
use crate::shared::keyring_ref::KEYRING_PREFIX;
static SECRET_POOL_STATE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

fn secret_pool_state() -> &'static Mutex<HashMap<String, usize>> {
    SECRET_POOL_STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn split_secret_pool(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let multiline: Vec<String> = trimmed
        .lines()
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    if multiline.len() > 1 {
        return multiline;
    }

    if trimmed.contains(',') {
        let comma_split: Vec<String> = trimmed
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect();
        if comma_split.len() > 1 {
            return comma_split;
        }
    }

    vec![trimmed.to_string()]
}

/// Env-var fallback candidates for a keyring service/account pair.
///
/// Shared with `intelligence::reinforcement::health` (single source of truth
/// for the keyring → env fallback chain).
pub(crate) fn keyring_env_fallback_candidates(service: &str, account: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    if account == "openai_api_key" {
        candidates.push("OPENAI_API_KEY".to_string());
    }

    // Keep compatibility with legacy OPENAI naming while using keyring account naming.
    if account == "openai_compatible_api_key" {
        candidates.push("OPENAI_COMPATIBLE_API_KEY".to_string());
        candidates.push("OPENAI_API_KEY".to_string());
    }

    if service == "go-on" && (account == "copilot_api_key" || account == "github_copilot_token") {
        // Copilot supports both historical and current names.
        candidates.push("GITHUB_COPILOT_TOKEN".to_string());
        candidates.push("GITHUB_TOKEN".to_string());
    }

    candidates.push(account.replace('-', "_").to_ascii_uppercase());
    candidates.push(
        format!("{}_{}", service, account)
            .replace('-', "_")
            .to_ascii_uppercase(),
    );

    candidates.sort();
    candidates.dedup();
    candidates
}

/// Candidate keyring service/account pairs for a locator, including
/// backward-compatible aliases (e.g. copilot_api_key ↔ github_copilot_token).
pub(crate) fn keyring_lookup_accounts(service: &str, account: &str) -> Vec<(String, String)> {
    let mut targets = vec![(service.to_string(), account.to_string())];

    // Backward/forward compatibility for Copilot key naming.
    if service == "go-on" {
        if account == "copilot_api_key" {
            targets.push((service.to_string(), "github_copilot_token".to_string()));
        } else if account == "github_copilot_token" {
            targets.push((service.to_string(), "copilot_api_key".to_string()));
        }
    }

    targets
}

/// Set to true once a keychain lookup times out.
/// Subsequent calls skip keychain entirely and go directly to env var fallback.
static KEYCHAIN_TIMEOUT_OCCURRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn load_secret_value(secret_ref: &str, field_name: &str) -> Result<String> {
    if let Some(locator) = secret_ref.strip_prefix(KEYRING_PREFIX) {
        let (service, account) = locator.split_once('/').ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                tf(
                    "error.keyring_invalid_ref",
                    &[("provider", field_name), ("ref", secret_ref)]
                )
            )
        })?;

        if service.trim().is_empty() || account.trim().is_empty() {
            anyhow::bail!(
                "{}",
                tf(
                    "error.keyring_invalid_ref",
                    &[("provider", field_name), ("ref", secret_ref)]
                )
            );
        }

        // Fast path: if a previous keychain lookup timed out, skip keychain
        // entirely and go directly to env var fallback. This avoids 5-second
        // delays on every chat message in background/headless mode.
        let mut keyring_error = "secret not found".to_string();
        if !KEYCHAIN_TIMEOUT_OCCURRED.load(std::sync::atomic::Ordering::Relaxed) {
            for (service_name, account_name) in keyring_lookup_accounts(service, account) {
                match keyring::Entry::new(&service_name, &account_name) {
                    Ok(entry) => {
                        // Use a timeout for keychain access to prevent hanging
                        // when running in headless/background mode on macOS.
                        // SecKeychainFindGenericPassword can block indefinitely if
                        // the Security framework requires user interaction.
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let _ = tx.send(entry.get_password());
                        });
                        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                            Ok(Ok(value)) if !value.trim().is_empty() => return Ok(value),
                            Ok(Ok(_)) => {
                                keyring_error = format!(
                                    "entry {}/{} resolved to empty value",
                                    service_name, account_name
                                );
                            }
                            Ok(Err(err)) => {
                                keyring_error = format!(
                                    "failed to read keyring entry {}/{}: {}",
                                    service_name, account_name, err
                                );
                            }
                            Err(_) => {
                                warn!(
                                    "{} keyring lookup timed out after 5s for {}/{}, falling back",
                                    field_name, service_name, account_name
                                );
                                KEYCHAIN_TIMEOUT_OCCURRED
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                                keyring_error = format!(
                                    "keychain lookup timed out for {}/{}",
                                    service_name, account_name
                                );
                            }
                        }
                    }
                    Err(err) => {
                        keyring_error = format!(
                            "failed to open keyring entry {}/{}: {}",
                            service_name, account_name, err
                        );
                    }
                }
            }
        } else {
            debug!(
                "{} keychain skipped (previous timeout), using env fallback directly",
                field_name
            );
        }

        let fallback_candidates = keyring_env_fallback_candidates(service, account);
        for env_name in &fallback_candidates {
            if let Some(value) = crate::shared::secret_override::get_secret(env_name) {
                let trimmed = value.trim().to_string();
                if !trimmed.is_empty() {
                    warn!(
                        "{} keyring lookup failed, using env fallback {}",
                        field_name, env_name
                    );
                    return Ok(trimmed);
                }
            }
        }

        anyhow::bail!(
            "{}",
            tf(
                "error.keyring_unavailable",
                &[
                    ("provider", field_name),
                    ("error", &keyring_error),
                    ("vars", &fallback_candidates.join(", "))
                ]
            )
        );
    }

    crate::shared::secret_override::get_secret(secret_ref)
        .ok_or_else(|| anyhow::anyhow!("{}", tf("error.missing_env_var", &[("name", secret_ref)])))
}

fn rotation_group(field_name: &str) -> String {
    field_name
        .split('.')
        .next()
        .unwrap_or(field_name)
        .to_string()
}

fn pick_secret_pool_index(field_name: &str, len: usize) -> usize {
    if len <= 1 {
        return 0;
    }

    let group = rotation_group(field_name);
    let mut state = match secret_pool_state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            warn!("secret pool rotation state lock poisoned; recovering rotation state");
            poisoned.into_inner()
        }
    };

    if field_name.ends_with("api_key_env") {
        let entry = state.entry(group).or_insert(0);
        let index = *entry % len;
        *entry = (*entry + 1) % len;
        return index;
    }

    if field_name.ends_with("secret_key_env") {
        // Paired rotation: pick the index the api_key branch just used
        // ((current - 1) mod len), then advance the shared per-group counter.
        // Previously the counter was read but never written back, so a
        // multi-valued secret pool never actually rotated — every call
        // recomputed the same index from the initial value.
        let entry = state.entry(group).or_insert(1);
        let index = (*entry + len - 1) % len;
        *entry = (*entry + 1) % len;
        return index;
    }

    let entry = state.entry(group).or_insert(0);
    let index = *entry % len;
    *entry = (*entry + 1) % len;
    index
}

pub(crate) fn inspect_secret_pool(secret_ref: &str, field_name: &str) -> Result<Vec<String>> {
    let value = load_secret_value(secret_ref, field_name)?;
    let candidates = split_secret_pool(&value);
    if candidates.is_empty() {
        anyhow::bail!(
            "{}",
            tf("error.keyring_empty_pool", &[("provider", field_name)])
        );
    }

    for candidate in &candidates {
        crate::shared::secret_override::validate_secret_security(
            candidate,
            field_name,
            "error.agent_empty_field",
        )?;
    }

    Ok(candidates)
}

/// Model information for provider selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier (e.g., "deepseek-v4-flash")
    pub id: String,
    /// Human-readable model name
    pub name: String,
    /// Model description
    pub description: String,
    /// Whether this is the default model for the provider
    pub is_default: bool,
    /// Model capabilities (e.g., ["chat", "vision", "function_calling"])
    pub capabilities: Vec<String>,
    /// Context window size
    pub context_window: Option<usize>,
}

/// Chat message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Message role (e.g., "user", "assistant", "system")
    pub role: String,
    /// Message content
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct StreamingSender {
    inner: mpsc::UnboundedSender<String>,
}

impl StreamingSender {
    pub fn new(inner: mpsc::UnboundedSender<String>) -> Self {
        Self { inner }
    }

    /// Send a token to the stream.
    ///
    /// Uses `UnboundedSender::send` which is always synchronous and never
    /// blocks or spawns blocking threads. Returns `Err` if the receiver
    /// was dropped (channel closed).
    pub fn send(&self, token: String) -> std::result::Result<(), mpsc::error::SendError<String>> {
        self.inner.send(token)
    }
}

impl From<mpsc::UnboundedSender<String>> for StreamingSender {
    fn from(inner: mpsc::UnboundedSender<String>) -> Self {
        Self::new(inner)
    }
}

/// Agent trait defining the interface for all AI agents
///
/// Phase 0/1: all agent entrypoints should support AgentTaskEnvelope as input
/// and AgentTaskResult as output, and produce AgentAuditLog at decision points.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Send chat messages to the agent and receive streaming responses.
    ///
    /// Default implementation wraps `chat_once` with retry logic via
    /// `retry_chat_once`.  Agents that need custom retry or model-selection
    /// behaviour (e.g. Copilot) override this.
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: StreamingSender,
    ) -> AppResult<()> {
        retry_chat_once(
            || async {
                self.chat_once(&messages, &principles, &options, sender.clone())
                    .await
                    .map_err(Into::into)
            },
            3,
        )
        .await
    }

    /// Perform a single chat attempt without retry logic.
    ///
    /// Implementations should make one API call and return the result.
    /// The default returns an error; agents that override `chat` directly
    /// (e.g. local test agents) do not need to implement this.
    async fn chat_once(
        &self,
        _messages: &[Message],
        _principles: &Option<Vec<String>>,
        _options: &Option<HashMap<String, Value>>,
        _sender: StreamingSender,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("chat_once not implemented"))
    }

    /// Get available models for this provider
    ///
    /// Returns a list of models that can be used with this provider.
    /// Every provider must implement this with its actual model list.
    fn available_models(&self) -> Vec<ModelInfo>;

    /// Get the default model from available models.
    /// Default implementation finds the first model with `is_default == true`.
    /// Agents with custom model resolution logic (e.g. Copilot, Wenxin) override this.
    fn default_model(&self) -> Option<ModelInfo> {
        self.available_models().into_iter().find(|m| m.is_default)
    }

    /// Whether the provider supports overriding the target model through chat options.
    fn supports_model_override(&self) -> bool {
        true
    }

    /// (Phase 0/1 discipline) Structured agent task entrypoint.
    ///
    /// Default implementation: builds chat messages from the envelope,
    /// calls `chat()` with a standalone channel, collects the streamed
    /// response, and wraps it in an `AgentTaskResult`.  Agents that need
    /// custom task logic can override this method.
    async fn run_task(&self, envelope: AgentTaskEnvelope) -> AppResult<AgentTaskResult> {
        // Build messages from the task envelope via the shared implementation
        // (was previously an inline mirror of `mode::build_chat_messages`).
        let messages = crate::orchestration::mode::build_chat_messages(&envelope);

        // Create a standalone channel for collecting the chat response.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let sender = StreamingSender::new(tx);

        // Call chat() — all agents implement this.
        self.chat(messages, None, None, sender).await?;

        // Collect all streamed tokens, bounded by the shared stream caps
        // (the output flows into the task result / LLM context).
        let output = crate::acp::helpers::conversation::drain_channel_capped(&mut rx).await;

        Ok(AgentTaskResult {
            success: true,
            output: Some(json!({
                "status": "completed",
                "answer": output,
            })),
            error: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

/// Agent registry for managing and accessing agents
pub struct AgentRegistry {
    /// Map of agent names to agent instances
    agents: HashMap<String, Arc<dyn Agent>>,
    /// Insertion order of agent names (for true FIFO eviction at capacity).
    agent_order: std::collections::VecDeque<String>,
    /// Optional multi-level token cache — when set, all agents returned
    /// via `get()` are automatically wrapped with `CachedAgentWrapper`.
    token_cache: RwLock<Option<Arc<TokenCache>>>,
    /// Capability graph for capability-based agent routing
    capability_graph: Arc<Mutex<CapabilityGraph>>,
}

/// Maximum agents allowed in the registry before evicting the oldest entry.
const MAX_AGENTS: usize = 1000;

impl std::fmt::Debug for AgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRegistry")
            .field("agents", &self.agents.keys())
            .field("capability_graph", &self.capability_graph)
            .finish_non_exhaustive()
    }
}

impl AgentRegistry {
    /// Create an empty agent registry
    ///
    /// # Returns
    /// * `Self` - Returns an empty registry
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            agent_order: std::collections::VecDeque::new(),
            token_cache: RwLock::new(None),
            capability_graph: Arc::new(Mutex::new(CapabilityGraph::new())),
        }
    }

    /// Create an agent registry with an external capability graph
    pub fn with_capability_graph(self, graph: Arc<Mutex<CapabilityGraph>>) -> Self {
        Self {
            capability_graph: graph,
            ..self
        }
    }

    /// Create an agent registry from configuration
    ///
    /// # Arguments
    /// * `config` - Application configuration containing agent definitions
    /// * `client` - HTTP client for agent requests
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if the registry is created successfully, or an error if something goes wrong
    pub fn from_config(
        config: Arc<AppConfig>,
        client: reqwest::Client,
        capability_graph: Arc<Mutex<CapabilityGraph>>,
    ) -> Result<Self> {
        let mut agents: HashMap<String, Arc<dyn Agent>> = HashMap::new();

        for (name, agent_cfg) in config.agents() {
            let agent = build_agent(agent_cfg, client.clone())
                .with_context(|| format!("failed to build agent '{}'", name))?;
            agents.insert(name.clone(), agent);

            // Register agent in capability graph with inferred tags
            // Every agent gets "general" by default so the capability bus
            // can find it via agents_with_tag("general") during candidate
            // selection. Additional tags enable more specific routing.
            let mut tags = vec![agent_cfg.agent_type.clone(), "general".to_string()];
            let name_lower = name.to_lowercase();
            if name_lower.contains("primary")
                || name_lower.contains("coder")
                || name_lower.contains("developer")
            {
                tags.push("coding".to_string());
            }
            if name_lower.contains("review") || name_lower.contains("reviewer") {
                tags.push("review".to_string());
                tags.push("qa".to_string());
            }
            if name_lower.contains("test") {
                tags.push("testing".to_string());
                tags.push("qa".to_string());
            }
            if name_lower.contains("vendor") || name_lower.contains("external") {
                tags.push("vendor".to_string());
            }
            if name_lower.contains("fallback") {
                tags.push("fallback".to_string());
            }

            // Register with capability graph — recover from poison to avoid
            // silently dropping agent capability registration.
            let decl = CapabilityDecl {
                name: name.clone(),
                description: format!("Agent {} of type {}", name, agent_cfg.agent_type),
                tags: tags.clone(),
            };
            let mut graph = capability_graph.lock().unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "capability_graph lock poisoned during agent registration – recovered"
                );
                poisoned.into_inner()
            });
            graph.register_agent(name, vec![decl]);
        }

        Ok(Self {
            agents,
            agent_order: config.agents().keys().cloned().collect(),
            token_cache: RwLock::new(None),
            capability_graph,
        })
    }

    /// Get an agent by name
    ///
    /// When a `token_cache` is configured, the returned agent is automatically
    /// wrapped with `CachedAgentWrapper` so that every `chat()` call goes
    /// through the multi-level token cache first.
    ///
    /// # Arguments
    /// * `name` - Agent name
    ///
    /// # Returns
    /// * `Option<Arc<dyn Agent>>` - Returns Some(agent) if found, or None if not found
    pub fn get(&self, name: &str) -> Option<Arc<dyn Agent>> {
        let agent = self.get_unwrapped(name)?;
        if let Ok(guard) = self.token_cache.read() {
            if let Some(ref cache) = *guard {
                return Some(Arc::new(CachedAgentWrapper::new(agent, Arc::clone(cache))));
            }
        }
        Some(agent)
    }

    /// Get an agent by name **without** the token-cache wrapper.
    ///
    /// Cache-managed call paths (e.g. the ACP chat phases, where `act_phase`
    /// runs the single canonical cache lookup/store) resolve raw agents here so
    /// agent execution does not trigger a second lookup/store per call.
    pub fn get_unwrapped(&self, name: &str) -> Option<Arc<dyn Agent>> {
        self.agents.get(name).cloned()
    }

    /// Attach a multi-level token cache so that all agents returned by `get()`
    /// are automatically wrapped with `CachedAgentWrapper`.
    pub fn with_token_cache(self, cache: Arc<TokenCache>) -> Self {
        if let Ok(mut guard) = self.token_cache.write() {
            *guard = Some(cache);
        }
        self
    }

    /// Set (or clear) the token cache on an existing registry (works with &self,
    /// uses internal RwLock).
    pub fn set_token_cache(&self, cache: Option<Arc<TokenCache>>) {
        if let Ok(mut guard) = self.token_cache.write() {
            *guard = cache;
        }
    }

    pub fn register_arc(&mut self, name: impl Into<String>, agent: Arc<dyn Agent>) {
        let name = name.into();
        // Evict the OLDEST registered agent when at capacity (HashMap
        // iteration order is arbitrary, so `keys().next()` would evict a
        // random agent — the previous behavior).
        if self.agents.len() >= MAX_AGENTS && !self.agents.contains_key(&name) {
            if let Some(oldest) = self.agent_order.pop_front() {
                self.agents.remove(&oldest);
            }
        }
        // Re-registering an existing name must not leave a stale queue entry
        // behind (duplicates would let eviction remove the wrong entry and
        // let the map exceed MAX_AGENTS).
        if self.agents.contains_key(&name) {
            self.agent_order.retain(|n| n != &name);
        }
        self.agent_order.push_back(name.clone());
        // NOTE: no CachedAgentWrapper here — wrapping is a `get()`/`all()`
        // responsibility. The cache-managed call paths (ACP chat phases)
        // resolve raw agents via `get_unwrapped`/`all_unwrapped` so agent
        // execution does not trigger a second lookup/store per call; wrapping
        // at registration time would silently re-introduce that double path
        // for `get_unwrapped` consumers once a token cache is configured.
        self.agents.insert(name, agent);
    }

    /// Get all registered agents as a batch.
    ///
    /// This is more efficient than calling `get()` in a loop because it
    /// acquires the token cache lock at most once (rather than per-agent)
    /// and avoids N individual `HashMap::get` lookups.
    pub fn all(&self) -> Vec<(String, Arc<dyn Agent>)> {
        let agents = self.all_unwrapped();
        if let Ok(guard) = self.token_cache.read() {
            if let Some(ref cache) = *guard {
                return agents
                    .into_iter()
                    .map(|(name, agent)| {
                        (
                            name,
                            Arc::new(CachedAgentWrapper::new(agent, Arc::clone(cache)))
                                as Arc<dyn Agent>,
                        )
                    })
                    .collect();
            }
        }
        agents
    }

    /// Get all registered agents as a batch **without** the token-cache wrapper.
    ///
    /// Used by cache-managed call paths that run their own cache gate (see
    /// [`Self::get_unwrapped`]).
    pub fn all_unwrapped(&self) -> Vec<(String, Arc<dyn Agent>)> {
        let mut agents: Vec<(String, Arc<dyn Agent>)> = self
            .agents
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        agents.sort_by(|a, b| a.0.cmp(&b.0));
        agents
    }

    pub fn names(&self) -> Vec<String> {
        let mut names = self.agents.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn models(&self) -> Vec<(String, Option<ModelInfo>, Vec<ModelInfo>)> {
        let mut catalog = self
            .agents
            .iter()
            .map(|(name, agent)| {
                (
                    name.clone(),
                    agent.default_model(),
                    agent.available_models(),
                )
            })
            .collect::<Vec<_>>();
        catalog.sort_by(|left, right| left.0.cmp(&right.0));
        catalog
    }

    /// Returns a reference to the capability graph used for agent routing
    pub fn get_capability_graph(&self) -> Arc<Mutex<CapabilityGraph>> {
        Arc::clone(&self.capability_graph)
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build an agent based on configuration
///
/// # Arguments
/// * `config` - Agent configuration
/// * `client` - HTTP client for agent requests
///
/// # Returns
/// * `Result<Arc<dyn Agent>>` - Returns Ok(agent) if built successfully, or an error if something goes wrong
fn build_agent(config: &AgentConfig, client: reqwest::Client) -> Result<Arc<dyn Agent>> {
    fn required_field(agent_name: &str, value: &Option<String>, field: &str) -> Result<String> {
        value.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                tf(
                    "error.agent_requires_field",
                    &[("agent", agent_name), ("field", field)]
                )
            )
        })
    }

    fn local_test_agents_enabled() -> bool {
        std::env::var("GO_ON_ENABLE_LOCAL_TEST_AGENTS")
            .map(|value| {
                value == "1"
                    || value.eq_ignore_ascii_case("true")
                    || value.eq_ignore_ascii_case("yes")
            })
            .unwrap_or(false)
    }

    fn ensure_local_test_agent_allowed(agent_type: &str) -> Result<()> {
        if local_test_agents_enabled() {
            return Ok(());
        }
        anyhow::bail!(
            "{}",
            tf("error.agent_test_only", &[("agent_type", agent_type)])
        );
    }

    match config.agent_type.as_str() {
        "local_echo" => {
            ensure_local_test_agent_allowed("local_echo")?;
            Ok(Arc::new(LocalEchoAgent))
        }
        "local_approve" => {
            ensure_local_test_agent_allowed("local_approve")?;
            Ok(Arc::new(LocalApproveAgent))
        }
        "local_slow_approve" => {
            ensure_local_test_agent_allowed("local_slow_approve")?;
            Ok(Arc::new(LocalSlowApproveAgent))
        }
        "copilot" => {
            // Store the env var name; the token value is read lazily on first request.
            // Token content is not loaded here; it is resolved at request time.
            let token_env = config
                .api_key_env
                .clone()
                .unwrap_or_else(|| "GITHUB_TOKEN".to_string());
            Ok(Arc::new(CopilotAgent::new(token_env, client)))
        }
        "deepseek" => {
            let api_key_env = required_field("deepseek", &config.api_key_env, "api_key_env")?;
            // Default base URL comes from the provider spec (single source);
            // the fallback literal is only used when the spec is absent.
            let spec_url = crate::core::providers::provider_spec_by_name("deepseek")
                .and_then(|spec| spec.url.clone());
            let base_url = config
                .url
                .clone()
                .or(spec_url)
                .unwrap_or_else(|| crate::core::providers::DEFAULT_DEEPSEEK_BASE.to_string());
            let model = config
                .model
                .clone()
                // Default model from the provider spec (single source).
                .or_else(|| {
                    crate::core::providers::provider_spec_by_name("deepseek")
                        .and_then(|spec| spec.model.clone())
                })
                .unwrap_or_else(|| crate::core::providers::DEFAULT_DEEPSEEK_MODEL.to_string());
            Ok(Arc::new(DeepSeekAgent::new(
                base_url,
                api_key_env,
                model,
                client,
            )))
        }
        "wenxin" => {
            let api_key_env = required_field("wenxin", &config.api_key_env, "api_key_env")?;
            let secret_key_env =
                required_field("wenxin", &config.secret_key_env, "secret_key_env")?;
            Ok(Arc::new(BaiduErnieAgent::new(
                ErnieApi::Wenxin,
                String::new(),
                api_key_env,
                secret_key_env,
                client,
            )))
        }
        agent_type @ ("openai" | "openai_compatible" | "doubao" | "qwen" | "groq" | "llama"
        | "mistral" | "perplexity" | "fireworks" | "ai21" | "aleph" | "deepquest"
        | "facewall" | "glm" | "hunyuan" | "kimi" | "langboat" | "loopai"
        | "minimax" | "moonshot" | "nim" | "replicate" | "siliconflow"
        | "skywork" | "stepfun" | "titan" | "together" | "xai" | "xihu" | "yi") => {
            // All defaults come from the provider spec (single source of
            // truth): url/model fall back to the spec, chat_path and
            // supports_system pick up spec hints, and SSE gzip compression is
            // enabled per spec flag. api_key_env stays config-only so a
            // missing key is a hard error regardless of the spec default.
            let spec = crate::core::providers::provider_spec_by_name(agent_type);
            let url = config
                .url
                .clone()
                .or_else(|| spec.and_then(|s| s.url.clone()))
                .ok_or_else(|| anyhow::anyhow!("{agent_type} requires a url"))?;
            let model = config
                .model
                .clone()
                .or_else(|| spec.and_then(|s| s.model.clone()))
                .ok_or_else(|| anyhow::anyhow!("{agent_type} requires a model"))?;
            let api_key_env = required_field(agent_type, &config.api_key_env, "api_key_env")?;
            let chat_path = config
                .chat_path
                .clone()
                .or_else(|| spec.and_then(|s| s.chat_path.clone()))
                .unwrap_or_else(|| default_openai_chat_path(&url).to_string());
            let supports_system = config
                .supports_system
                .or_else(|| spec.and_then(|s| s.supports_system))
                .unwrap_or(true);
            let compression = spec.map(|s| s.compression).unwrap_or(false);
            let agent = if compression {
                OpenAiCompatibleAgent::new_with_compression(
                    url,
                    chat_path,
                    api_key_env,
                    model,
                    supports_system,
                    client,
                )
            } else {
                OpenAiCompatibleAgent::new(
                    url,
                    chat_path,
                    api_key_env,
                    model,
                    supports_system,
                    client,
                )
            };
            Ok(Arc::new(agent))
        }
        "claude" => {
            // Default base URL comes from the provider spec (single source).
            let spec_url = crate::core::providers::provider_spec_by_name("anthropic")
                .and_then(|spec| spec.url.clone());
            let url = config
                .url
                .clone()
                .or(spec_url)
                .unwrap_or_else(|| crate::core::providers::DEFAULT_ANTHROPIC_BASE.to_string());
            let api_key_env = required_field("claude", &config.api_key_env, "api_key_env")?;
            let model = required_field("claude", &config.model, "model")?;
            let anthropic_version = config
                .anthropic_version
                .clone()
                // Spec default (single source); literal is only a last resort.
                .or_else(|| {
                    crate::core::providers::provider_spec_by_name("anthropic")
                        .and_then(|spec| spec.anthropic_version.clone())
                })
                .unwrap_or_else(|| "2023-06-01".to_string());
            // Spec default max_tokens (8192); the old 4096 fallback disagreed
            // with the spec and was silently used when config lacked max_tokens.
            let max_tokens = config
                .max_tokens
                .or_else(|| {
                    crate::core::providers::provider_spec_by_name("anthropic")
                        .and_then(|spec| spec.max_tokens)
                })
                .unwrap_or(4096);
            Ok(Arc::new(AnthropicAgent::new(
                url,
                api_key_env,
                model,
                anthropic_version,
                max_tokens,
                client,
            )))
        }
        "cohere" => {
            let api_key_env = required_field("cohere", &config.api_key_env, "api_key_env")?;
            let url = required_field("cohere", &config.url, "url")?;
            let model = required_field("cohere", &config.model, "model")?;
            Ok(Arc::new(CohereAgent::new(api_key_env, url, model, client)))
        }
        "gemini" => {
            let api_key_env = required_field("gemini", &config.api_key_env, "api_key_env")?;
            let url = required_field("gemini", &config.url, "url")?;
            let model = required_field("gemini", &config.model, "model")?;
            Ok(Arc::new(GeminiAgent::new(api_key_env, url, model, client)))
        }
        "qianfan" => {
            let api_key_env = required_field("qianfan", &config.api_key_env, "api_key_env")?;
            let secret_key_env =
                required_field("qianfan", &config.secret_key_env, "secret_key_env")?;
            let model = required_field("qianfan", &config.model, "model")?;
            Ok(Arc::new(BaiduErnieAgent::new(
                ErnieApi::Qianfan,
                model,
                api_key_env,
                secret_key_env,
                client,
            )))
        }
        other => anyhow::bail!(
            "{}",
            tf("error.agent_unsupported_type", &[("agent_type", other)])
        ),
    }
}

struct LocalEchoAgent;

#[async_trait]
impl Agent for LocalEchoAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    async fn chat(
        &self,
        messages: Vec<Message>,
        _principles: Option<Vec<String>>,
        _options: Option<HashMap<String, Value>>,
        sender: StreamingSender,
    ) -> AppResult<()> {
        let content = messages
            .iter()
            .rev()
            .find(|message| message.role.eq_ignore_ascii_case("user"))
            .map(|message| message.content.clone())
            .unwrap_or_else(|| "local echo".to_string());
        let _ = sender.send(content);
        Ok(())
    }
}

struct LocalApproveAgent;

#[async_trait]
impl Agent for LocalApproveAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    async fn chat(
        &self,
        _messages: Vec<Message>,
        _principles: Option<Vec<String>>,
        _options: Option<HashMap<String, Value>>,
        sender: StreamingSender,
    ) -> AppResult<()> {
        let _ = sender.send("APPROVE\nlocal reviewer approved".to_string());
        Ok(())
    }
}

struct LocalSlowApproveAgent;

#[async_trait]
impl Agent for LocalSlowApproveAgent {
    fn available_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    async fn chat(
        &self,
        _messages: Vec<Message>,
        _principles: Option<Vec<String>>,
        _options: Option<HashMap<String, Value>>,
        sender: StreamingSender,
    ) -> AppResult<()> {
        sleep(Duration::from_millis(1_500)).await;
        let _ = sender.send("APPROVE\nlocal slow reviewer approved".to_string());
        Ok(())
    }
}

/// Resolve a secret from environment variable or keyring
///
/// # Arguments
/// * `secret_ref` - Secret reference (either environment variable name or keyring:// reference)
/// * `field_name` - Field name for error messages
///
/// # Returns
/// * `Result<String>` - Returns Ok(secret) if resolved successfully, or an error if something goes wrong
pub(crate) fn resolve_secret(secret_ref: &str, field_name: &str) -> Result<String> {
    let candidates = inspect_secret_pool(secret_ref, field_name)?;
    let index = pick_secret_pool_index(field_name, candidates.len());
    Ok(candidates[index].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, AppConfig, FlowConfig, PhaseConfig, RuntimeConfig};
    use crate::intelligence::capability_graph::CapabilityGraph;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn default_chat_path_derives_from_base_url() {
        // Base URL already carries `/v1` → endpoint lives at the root.
        assert_eq!(
            default_openai_chat_path("https://api.openai.com/v1"),
            DEFAULT_ROOT_CHAT_PATH
        );
        assert_eq!(
            default_openai_chat_path("https://api.siliconflow.cn/v1/"),
            DEFAULT_ROOT_CHAT_PATH
        );
        // Bare origin (no `/v1`) → the full `/v1/chat/completions` path.
        assert_eq!(
            default_openai_chat_path("https://api.example.com"),
            DEFAULT_OPENAI_CHAT_PATH
        );
        assert_eq!(
            default_openai_chat_path("https://api.example.com/v2"),
            DEFAULT_OPENAI_CHAT_PATH
        );
    }

    #[test]
    fn openai_agent_falls_back_to_spec_url() {
        // The GUI's generated config may omit `url` for openai/
        // openai_compatible (the offline catalog carries no literal for the
        // DEFAULT_* constants). The agent must fall back to the spec default
        // or the server fails to start (required_field).
        let client = reqwest::Client::new();
        let mut cfg = build_agent_config("openai");
        cfg.url = None;
        let agent = build_agent(&cfg, client)
            .expect("openai without url must fall back to the spec default");
        // `available_models().description` embeds the resolved base URL
        // (spec default `https://api.openai.com/v1`), which combined with the
        // derived root chat path must not produce a double `/v1`.
        let models = agent.available_models();
        let desc = models.first().map(|m| m.description.as_str()).unwrap_or("");
        assert!(
            desc.contains("https://api.openai.com/v1"),
            "expected spec default base URL, got: {desc}"
        );
    }

    fn build_agent_config(agent_type: &str) -> AgentConfig {
        AgentConfig {
            agent_type: agent_type.to_string(),
            url: Some("https://example.com".to_string()),
            chat_path: None,
            api_key_env: Some("TEST_API_KEY".to_string()),
            secret_key_env: Some("TEST_SECRET_KEY".to_string()),
            anthropic_version: Some("2023-06-01".to_string()),
            model: Some("test-model".to_string()),
            max_tokens: Some(1024),
            supports_system: Some(true),
            supports_vision: None,
        }
    }

    #[test]
    fn build_agent_registry_includes_known_agents() {
        let mut agents = HashMap::new();
        agents.insert("openai".to_string(), build_agent_config("openai"));
        agents.insert("qwen".to_string(), build_agent_config("qwen"));
        agents.insert("qianfan".to_string(), build_agent_config("qianfan"));
        agents.insert("hunyuan".to_string(), build_agent_config("hunyuan"));

        let app_config = AppConfig {
            schema_version: "1.0.0".to_string(),
            layered_merge: false,
            provider: crate::core::config::types::ProviderConfig {
                default_phase: "coding".to_string(),
                agents,
                role_registry: HashMap::new(),
            },
            flow: FlowConfig {
                name: "test".to_string(),
                phases: vec!["coding".to_string()],
                workflow_type: crate::config::WorkflowType::Auto,
            },
            phases: {
                let mut m = HashMap::new();
                m.insert(
                    "coding".to_string(),
                    PhaseConfig {
                        description: "coding".to_string(),
                        agents: vec![
                            "openai".to_string(),
                            "qwen".to_string(),
                            "qianfan".to_string(),
                            "hunyuan".to_string(),
                        ],
                        fallback: Some(true),
                        principles: None,
                        options: None,
                    },
                );
                m
            },
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            security: crate::core::config::types::SecurityConfig::default(),
            feature: crate::core::config::types::FeatureConfig {
                model_selection_mode: "adaptive".to_string(),
                ..Default::default()
            },
            compliance: None,
            startup_context: None,
            protocol: None,
        };

        let registry = AgentRegistry::from_config(
            Arc::new(app_config),
            reqwest::Client::new(),
            Arc::new(Mutex::new(CapabilityGraph::new())),
        );
        assert!(
            registry.is_ok(),
            "AgentRegistry should build all supported types"
        );
    }

    #[test]
    fn split_secret_pool_supports_multiline_and_csv() {
        assert_eq!(
            split_secret_pool("key-a\nkey-b\nkey-c"),
            vec!["key-a", "key-b", "key-c"]
        );
        assert_eq!(
            split_secret_pool("key-a, key-b, key-c"),
            vec!["key-a", "key-b", "key-c"]
        );
    }

    #[test]
    fn resolve_secret_rotates_through_secret_pool() {
        let env_name = "GO_ON_TEST_MULTI_SECRET_POOL";
        unsafe {
            std::env::set_var(env_name, "alpha-key\nbeta-key\ngamma-key");
        }

        let one = resolve_secret(env_name, "openai.api_key_env")
            .expect("first call should resolve secret");
        let two = resolve_secret(env_name, "openai.api_key_env")
            .expect("second call should resolve secret");
        let three = resolve_secret(env_name, "openai.api_key_env")
            .expect("third call should resolve secret");

        assert_eq!(one, "alpha-key");
        assert_eq!(two, "beta-key");
        assert_eq!(three, "gamma-key");
    }

    /// M0.2 drift guard: every model ID a provider spec suggests for
    /// configuration (`model_suggestions`) must exist in that provider's
    /// runtime `available_models()`. The spec table
    /// (`src/core/providers.rs`) and the native agent implementations are the
    /// two faces of the same model catalog; when they disagree, the GUI's
    /// suggestions can offer models the runtime rejects (or vice versa). The
    /// generic OpenAI-compatible providers are excluded on purpose: their
    /// `available_models()` is config-driven (the configured model) rather
    /// than a static catalog, so a subset check would always fail.
    #[test]
    fn spec_model_suggestions_exist_in_native_available_models() {
        use crate::core::providers::provider_spec_by_name;
        use std::collections::HashSet;

        let client = reqwest::Client::new();
        let mut checked = 0usize;

        // deepseek → DeepSeekAgent (spec url/model defaults).
        let spec = provider_spec_by_name("deepseek").expect("deepseek spec");
        let agent = std::sync::Arc::new(DeepSeekAgent::new(
            spec.url.clone().unwrap_or_default(),
            spec.api_key_env.clone().unwrap_or_default(),
            spec.model.clone().unwrap_or_default(),
            client.clone(),
        ));
        let available: HashSet<String> =
            agent.available_models().into_iter().map(|m| m.id).collect();
        for suggestion in &spec.model_suggestions {
            assert!(
                available.contains(suggestion),
                "deepseek spec suggests `{suggestion}` but available_models() does not list it"
            );
        }
        checked += 1;

        // anthropic → AnthropicAgent (spec url/model/version/max_tokens defaults).
        let spec = provider_spec_by_name("anthropic").expect("anthropic spec");
        let agent = std::sync::Arc::new(AnthropicAgent::new(
            spec.url.clone().unwrap_or_default(),
            spec.api_key_env.clone().unwrap_or_default(),
            spec.model.clone().unwrap_or_default(),
            spec.anthropic_version.clone().unwrap_or_default(),
            spec.max_tokens.unwrap_or_default(),
            client.clone(),
        ));
        let available: HashSet<String> =
            agent.available_models().into_iter().map(|m| m.id).collect();
        for suggestion in &spec.model_suggestions {
            assert!(
                available.contains(suggestion),
                "anthropic spec suggests `{suggestion}` but available_models() does not list it"
            );
        }
        checked += 1;

        // gemini → GeminiAgent (spec url/model defaults).
        let spec = provider_spec_by_name("gemini").expect("gemini spec");
        let agent = std::sync::Arc::new(GeminiAgent::new(
            spec.api_key_env.clone().unwrap_or_default(),
            spec.url.clone().unwrap_or_default(),
            spec.model.clone().unwrap_or_default(),
            client.clone(),
        ));
        let available: HashSet<String> =
            agent.available_models().into_iter().map(|m| m.id).collect();
        for suggestion in &spec.model_suggestions {
            assert!(
                available.contains(suggestion),
                "gemini spec suggests `{suggestion}` but available_models() does not list it"
            );
        }
        checked += 1;

        // cohere → CohereAgent (spec url/model defaults).
        let spec = provider_spec_by_name("cohere").expect("cohere spec");
        let agent = std::sync::Arc::new(CohereAgent::new(
            spec.api_key_env.clone().unwrap_or_default(),
            spec.url.clone().unwrap_or_default(),
            spec.model.clone().unwrap_or_default(),
            client.clone(),
        ));
        let available: HashSet<String> =
            agent.available_models().into_iter().map(|m| m.id).collect();
        for suggestion in &spec.model_suggestions {
            assert!(
                available.contains(suggestion),
                "cohere spec suggests `{suggestion}` but available_models() does not list it"
            );
        }
        checked += 1;

        // wenxin / qianfan → BaiduErnieAgent (native catalogs per API).
        for (spec_name, api) in [("wenxin", ErnieApi::Wenxin), ("qianfan", ErnieApi::Qianfan)] {
            let spec = provider_spec_by_name(spec_name).expect("spec exists");
            let agent = std::sync::Arc::new(BaiduErnieAgent::new(
                api,
                spec.model.clone().unwrap_or_default(),
                spec.api_key_env.clone().unwrap_or_default(),
                spec.secret_key_env.clone().unwrap_or_default(),
                client.clone(),
            ));
            let available: HashSet<String> =
                agent.available_models().into_iter().map(|m| m.id).collect();
            for suggestion in &spec.model_suggestions {
                assert!(
                    available.contains(suggestion),
                    "{spec_name} spec suggests `{suggestion}` but available_models() does not list it"
                );
            }
            checked += 1;
        }

        assert_eq!(checked, 6, "guard must cover the six native catalogs");
    }
}
