//! Agent system implementation
//!
//! This module defines the Agent trait, AgentRegistry, and related functionality
//! for managing and interacting with different AI agents.
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! They define task contracts, audit schemas, and agent interfaces that will be wired
//! into the execution flow once orchestration logic is implemented.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::i18n::runtime::tf;
use crate::intelligence::capability_graph::{CapabilityDecl, CapabilityGraph};
use crate::intelligence::token_cache::{CachedAgentWrapper, TokenMultiLevelCache as TokenCache};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, warn};

use crate::agents::{
    Ai21Agent, AlephAgent, AnthropicAgent, CohereAgent, CopilotAgent, DeepQuestAgent,
    DeepSeekAgent, FaceWallAgent, FireworksAgent, GeminiAgent, GlmAgent, GroqAgent, HunyuanAgent,
    KimiAgent, LangboatAgent, LlamaAgent, LoopAiAgent, MiniMaxAgent, MistralAgent, MoonshotAgent,
    NimAgent, OpenAiAgent, OpenAiCompatibleAgent, PerplexityAgent, QianfanAgent, ReplicateAgent,
    SiliconFlowAgent, SkyworkAgent, StepFunAgent, TitanAgent, TogetherAgent, WenxinAgent, XaiAgent,
    XihuAgent, YiAgent,
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
                    sleep(Duration::from_secs(1_u64 << attempt)).await;
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
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("Timeout error: {0}")]
    Timeout(String),
    #[error("Unknown error: {0}")]
    Unknown(String),
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
/// Duplicated from acp::helpers::planning::context — consider using a shared constant.
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
const KEYRING_PREFIX: &str = "keyring://";
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

fn keyring_env_fallback_candidates(service: &str, account: &str) -> Vec<String> {
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

fn keyring_lookup_accounts(service: &str, account: &str) -> Vec<(String, String)> {
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
        let current = state.get(&group).copied().unwrap_or(1);
        return (current + len - 1) % len;
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
        validate_secret_security(candidate, field_name)?;
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
    inner: mpsc::Sender<String>,
}

impl StreamingSender {
    pub fn new(inner: mpsc::Sender<String>) -> Self {
        Self { inner }
    }

    /// Send a token to the stream.
    ///
    /// Uses `try_send` for non-blocking fast path, and falls back to
    /// `blocking_send` via `spawn_blocking` when the channel is full.
    /// This prevents token loss during high-throughput streaming while
    /// keeping the common case lock-free.
    pub fn send(
        &self,
        token: String,
    ) -> std::result::Result<(), mpsc::error::TrySendError<String>> {
        // Fast path: non-blocking try_send (common case, lock-free)
        match self.inner.try_send(token) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(token)) => {
                // Channel full — fall back to blocking_send via spawn_blocking.
                // This runs on a dedicated blocking thread, so no block_in_place
                // or block_on is needed — tokio::mpsc::Sender::blocking_send()
                // natively blocks the calling (blocking) thread.
                // Complies with principle #23 (no block_in_place + block_on in hot paths).
                let tx = self.inner.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = tx.blocking_send(token);
                });
                Ok(())
            }
            Err(e @ mpsc::error::TrySendError::Closed(_)) => Err(e),
        }
    }
}

impl From<mpsc::Sender<String>> for StreamingSender {
    fn from(inner: mpsc::Sender<String>) -> Self {
        Self::new(inner)
    }
}

/// Agent trait defining the interface for all AI agents
///
/// Phase 0/1: all agent entrypoints should support AgentTaskEnvelope as input
/// and AgentTaskResult as output, and produce AgentAuditLog at decision points.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Send chat messages to the agent and receive streaming responses
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: StreamingSender,
    ) -> AppResult<()>;

    /// Get available models for this provider
    ///
    /// Returns a list of models that can be used with this provider.
    /// Default implementation returns an empty list (providers should override if applicable).
    fn available_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    /// Get the default model for this provider
    ///
    /// Returns the currently configured default model.
    /// Default implementation returns None.
    fn default_model(&self) -> Option<ModelInfo> {
        None
    }

    /// Whether the provider supports overriding the target model through chat options.
    fn supports_model_override(&self) -> bool {
        true
    }

    /// (Phase 0/1 discipline) Structured agent task entrypoint
    fn run_task(&self, envelope: AgentTaskEnvelope) -> AppResult<AgentTaskResult> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());

        let audit = AgentAuditLog {
            agent: "generic".to_string(),
            phase: envelope.phase.clone(),
            task_id: envelope.task_id.clone(),
            decision: "rejected".to_string(),
            rationale: Some(tf("error.run_task_not_implemented", &[])),
            timestamp,
        };

        error!(
            target = "agent",
            "run_task called on unsupported provider: phase={} task_id={}",
            envelope.phase,
            envelope.task_id
        );

        Ok(AgentTaskResult {
            success: false,
            output: Some(json!({
                "task_id": envelope.task_id,
                "phase": envelope.phase,
                "role": envelope.role,
                "objective": envelope.objective,
                "status": "unsupported_operation"
            })),
            error: Some(AgentError::Runtime(tf("error.run_task_unsupported", &[]))),
            audit_log: Some(serde_json::to_string(&audit).map_err(anyhow::Error::from)?),
            pua_report: None,
        })
    }
}

/// Agent registry for managing and accessing agents
pub struct AgentRegistry {
    /// Map of agent names to agent instances
    agents: HashMap<String, Arc<dyn Agent>>,
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
        let agent = self.agents.get(name).cloned()?;
        if let Ok(guard) = self.token_cache.read() {
            if let Some(ref cache) = *guard {
                return Some(Arc::new(CachedAgentWrapper::new(agent, Arc::clone(cache))));
            }
        }
        Some(agent)
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
        // Evict oldest entry when at capacity.
        if self.agents.len() >= MAX_AGENTS && !self.agents.contains_key(&name) {
            if let Some(oldest) = self.agents.keys().next().cloned() {
                self.agents.remove(&oldest);
            }
        }
        // When enable_token_cache is true and a token cache is configured,
        // auto-wrap the agent with CachedAgentWrapper so that all chat()
        // calls go through the multi-level token cache.
        let agent = if let Ok(guard) = self.token_cache.read() {
            if let Some(ref cache) = *guard {
                Arc::new(CachedAgentWrapper::new(agent, Arc::clone(cache))) as Arc<dyn Agent>
            } else {
                agent
            }
        } else {
            agent
        };
        self.agents.insert(name, agent);
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

impl AgentRegistry {
    /// Return a JSON-serializable snapshot of token cache statistics.
    /// Returns `null` when no token cache is configured.
    pub fn cache_stats_json(&self) -> serde_json::Value {
        if let Ok(guard) = self.token_cache.read() {
            if let Some(ref cache) = *guard {
                return cache.stats_snapshot();
            }
        }
        serde_json::Value::Null
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
            let base_url = config
                .url
                .clone()
                .unwrap_or_else(|| "https://api.deepseek.com".to_string());
            let model = config
                .model
                .clone()
                .unwrap_or_else(|| "deepseek-v4-flash".to_string());
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
            Ok(Arc::new(WenxinAgent::new(
                api_key_env,
                secret_key_env,
                client,
            )))
        }
        "openai_compatible" => {
            let url = required_field("openai_compatible", &config.url, "url")?;
            let chat_path = config
                .chat_path
                .clone()
                .unwrap_or_else(|| "/v1/chat/completions".to_string());
            let api_key_env =
                required_field("openai_compatible", &config.api_key_env, "api_key_env")?;
            let model = required_field("openai_compatible", &config.model, "model")?;
            let supports_system = config.supports_system.unwrap_or(true);
            Ok(Arc::new(OpenAiCompatibleAgent::new(
                url,
                chat_path,
                api_key_env,
                model,
                supports_system,
                client,
            )))
        }
        "doubao" => {
            let url = required_field("doubao", &config.url, "url")?;
            let chat_path = config
                .chat_path
                .clone()
                .unwrap_or_else(|| "/chat/completions".to_string());
            let api_key_env = required_field("doubao", &config.api_key_env, "api_key_env")?;
            let model = required_field("doubao", &config.model, "model")?;
            let supports_system = config.supports_system.unwrap_or(true);
            Ok(Arc::new(OpenAiCompatibleAgent::new(
                url,
                chat_path,
                api_key_env,
                model,
                supports_system,
                client,
            )))
        }
        "claude" => {
            let url = config
                .url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".to_string());
            let api_key_env = required_field("claude", &config.api_key_env, "api_key_env")?;
            let model = required_field("claude", &config.model, "model")?;
            let anthropic_version = config
                .anthropic_version
                .clone()
                .unwrap_or_else(|| "2023-06-01".to_string());
            let max_tokens = config.max_tokens.unwrap_or(4096);
            Ok(Arc::new(AnthropicAgent::new(
                url,
                api_key_env,
                model,
                anthropic_version,
                max_tokens,
                client,
            )))
        }
        "openai" => {
            let api_key_env = required_field("openai", &config.api_key_env, "api_key_env")?;
            let url = required_field("openai", &config.url, "url")?;
            let model = required_field("openai", &config.model, "model")?;
            Ok(Arc::new(OpenAiAgent::new(api_key_env, url, model, client)))
        }
        "ai21" => {
            let api_key_env = required_field("ai21", &config.api_key_env, "api_key_env")?;
            let url = required_field("ai21", &config.url, "url")?;
            let model = required_field("ai21", &config.model, "model")?;
            Ok(Arc::new(Ai21Agent::new(api_key_env, url, model, client)))
        }
        "aleph" => {
            let api_key_env = required_field("aleph", &config.api_key_env, "api_key_env")?;
            let url = required_field("aleph", &config.url, "url")?;
            let model = required_field("aleph", &config.model, "model")?;
            Ok(Arc::new(AlephAgent::new(api_key_env, url, model, client)))
        }
        "cohere" => {
            let api_key_env = required_field("cohere", &config.api_key_env, "api_key_env")?;
            let url = required_field("cohere", &config.url, "url")?;
            let model = required_field("cohere", &config.model, "model")?;
            Ok(Arc::new(CohereAgent::new(api_key_env, url, model, client)))
        }
        "deepquest" => {
            let api_key_env = required_field("deepquest", &config.api_key_env, "api_key_env")?;
            let url = required_field("deepquest", &config.url, "url")?;
            let model = required_field("deepquest", &config.model, "model")?;
            Ok(Arc::new(DeepQuestAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "facewall" => {
            let api_key_env = required_field("facewall", &config.api_key_env, "api_key_env")?;
            let url = required_field("facewall", &config.url, "url")?;
            let model = required_field("facewall", &config.model, "model")?;
            Ok(Arc::new(FaceWallAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "fireworks" => {
            let api_key_env = required_field("fireworks", &config.api_key_env, "api_key_env")?;
            let url = required_field("fireworks", &config.url, "url")?;
            let model = required_field("fireworks", &config.model, "model")?;
            Ok(Arc::new(FireworksAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "gemini" => {
            let api_key_env = required_field("gemini", &config.api_key_env, "api_key_env")?;
            let url = required_field("gemini", &config.url, "url")?;
            let model = required_field("gemini", &config.model, "model")?;
            Ok(Arc::new(GeminiAgent::new(api_key_env, url, model, client)))
        }
        "glm" => {
            let api_key_env = required_field("glm", &config.api_key_env, "api_key_env")?;
            let url = required_field("glm", &config.url, "url")?;
            let model = required_field("glm", &config.model, "model")?;
            Ok(Arc::new(GlmAgent::new(api_key_env, url, model, client)))
        }
        "groq" => {
            let api_key_env = required_field("groq", &config.api_key_env, "api_key_env")?;
            let url = required_field("groq", &config.url, "url")?;
            let model = required_field("groq", &config.model, "model")?;
            Ok(Arc::new(GroqAgent::new(api_key_env, url, model, client)))
        }
        "langboat" => {
            let api_key_env = required_field("langboat", &config.api_key_env, "api_key_env")?;
            let url = required_field("langboat", &config.url, "url")?;
            let model = required_field("langboat", &config.model, "model")?;
            Ok(Arc::new(LangboatAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "llama" => {
            let api_key_env = required_field("llama", &config.api_key_env, "api_key_env")?;
            let url = required_field("llama", &config.url, "url")?;
            let model = required_field("llama", &config.model, "model")?;
            Ok(Arc::new(LlamaAgent::new(api_key_env, url, model, client)))
        }
        "loopai" => {
            let api_key_env = required_field("loopai", &config.api_key_env, "api_key_env")?;
            let url = required_field("loopai", &config.url, "url")?;
            let model = required_field("loopai", &config.model, "model")?;
            Ok(Arc::new(LoopAiAgent::new(api_key_env, url, model, client)))
        }
        "minimax" => {
            let api_key_env = required_field("minimax", &config.api_key_env, "api_key_env")?;
            let url = required_field("minimax", &config.url, "url")?;
            let model = required_field("minimax", &config.model, "model")?;
            Ok(Arc::new(MiniMaxAgent::new(api_key_env, url, model, client)))
        }
        "mistral" => {
            let api_key_env = required_field("mistral", &config.api_key_env, "api_key_env")?;
            let url = required_field("mistral", &config.url, "url")?;
            let model = required_field("mistral", &config.model, "model")?;
            Ok(Arc::new(MistralAgent::new(api_key_env, url, model, client)))
        }
        "moonshot" => {
            let api_key_env = required_field("moonshot", &config.api_key_env, "api_key_env")?;
            let url = required_field("moonshot", &config.url, "url")?;
            let model = required_field("moonshot", &config.model, "model")?;
            Ok(Arc::new(MoonshotAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "nim" => {
            let api_key_env = required_field("nim", &config.api_key_env, "api_key_env")?;
            let url = required_field("nim", &config.url, "url")?;
            let model = required_field("nim", &config.model, "model")?;
            Ok(Arc::new(NimAgent::new(api_key_env, url, model, client)))
        }
        "perplexity" => {
            let api_key_env = required_field("perplexity", &config.api_key_env, "api_key_env")?;
            let url = required_field("perplexity", &config.url, "url")?;
            let model = required_field("perplexity", &config.model, "model")?;
            Ok(Arc::new(PerplexityAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "replicate" => {
            let api_key_env = required_field("replicate", &config.api_key_env, "api_key_env")?;
            let url = required_field("replicate", &config.url, "url")?;
            let model = required_field("replicate", &config.model, "model")?;
            Ok(Arc::new(ReplicateAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "skywork" => {
            let api_key_env = required_field("skywork", &config.api_key_env, "api_key_env")?;
            let url = required_field("skywork", &config.url, "url")?;
            let model = required_field("skywork", &config.model, "model")?;
            Ok(Arc::new(SkyworkAgent::new(api_key_env, url, model, client)))
        }
        "stepfun" => {
            let api_key_env = required_field("stepfun", &config.api_key_env, "api_key_env")?;
            let url = required_field("stepfun", &config.url, "url")?;
            let model = required_field("stepfun", &config.model, "model")?;
            Ok(Arc::new(StepFunAgent::new(api_key_env, url, model, client)))
        }
        "titan" => {
            let api_key_env = required_field("titan", &config.api_key_env, "api_key_env")?;
            let url = required_field("titan", &config.url, "url")?;
            let model = required_field("titan", &config.model, "model")?;
            Ok(Arc::new(TitanAgent::new(api_key_env, url, model, client)))
        }
        "together" => {
            let api_key_env = required_field("together", &config.api_key_env, "api_key_env")?;
            let url = required_field("together", &config.url, "url")?;
            let model = required_field("together", &config.model, "model")?;
            Ok(Arc::new(TogetherAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "xihu" => {
            let api_key_env = required_field("xihu", &config.api_key_env, "api_key_env")?;
            let url = required_field("xihu", &config.url, "url")?;
            let model = required_field("xihu", &config.model, "model")?;
            Ok(Arc::new(XihuAgent::new(api_key_env, url, model, client)))
        }
        "yi" => {
            let api_key_env = required_field("yi", &config.api_key_env, "api_key_env")?;
            let url = required_field("yi", &config.url, "url")?;
            let model = required_field("yi", &config.model, "model")?;
            Ok(Arc::new(YiAgent::new(api_key_env, url, model, client)))
        }
        "xai" => {
            let api_key_env = required_field("xai", &config.api_key_env, "api_key_env")?;
            let url = required_field("xai", &config.url, "url")?;
            let model = required_field("xai", &config.model, "model")?;
            Ok(Arc::new(XaiAgent::new(api_key_env, url, model, client)))
        }
        "siliconflow" => {
            let api_key_env = required_field("siliconflow", &config.api_key_env, "api_key_env")?;
            let url = required_field("siliconflow", &config.url, "url")?;
            let model = required_field("siliconflow", &config.model, "model")?;
            Ok(Arc::new(SiliconFlowAgent::new(
                api_key_env,
                url,
                model,
                client,
            )))
        }
        "qianfan" => {
            let api_key_env = required_field("qianfan", &config.api_key_env, "api_key_env")?;
            let secret_key_env =
                required_field("qianfan", &config.secret_key_env, "secret_key_env")?;
            let model = required_field("qianfan", &config.model, "model")?;
            Ok(Arc::new(QianfanAgent::new(
                api_key_env,
                secret_key_env,
                model,
                client,
            )))
        }
        "qwen" => {
            let api_key_env = required_field("qwen", &config.api_key_env, "api_key_env")?;
            let url = required_field("qwen", &config.url, "url")?;
            let chat_path = config
                .chat_path
                .clone()
                .unwrap_or_else(|| "/chat/completions".to_string());
            let model = required_field("qwen", &config.model, "model")?;
            let supports_system = config.supports_system.unwrap_or(true);
            Ok(Arc::new(OpenAiCompatibleAgent::new(
                url,
                chat_path,
                api_key_env,
                model,
                supports_system,
                client,
            )))
        }
        "hunyuan" => {
            let api_key_env = required_field("hunyuan", &config.api_key_env, "api_key_env")?;
            let url = required_field("hunyuan", &config.url, "url")?;
            let model = required_field("hunyuan", &config.model, "model")?;
            Ok(Arc::new(HunyuanAgent::new(api_key_env, url, model, client)))
        }
        "kimi" => {
            let api_key_env = required_field("kimi", &config.api_key_env, "api_key_env")?;
            let url = required_field("kimi", &config.url, "url")?;
            let model = required_field("kimi", &config.model, "model")?;
            Ok(Arc::new(KimiAgent::new(api_key_env, url, model, client)))
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

/// Validate secret safety.
///
/// # Arguments
/// * `secret` - Secret value to validate.
/// * `field_name` - Field name used in warnings and errors.
///
/// # Returns
/// * `Result<()>` - Ok if checks pass, error on invalid values.
fn validate_secret_security(secret: &str, field_name: &str) -> Result<()> {
    if secret.trim().is_empty() {
        anyhow::bail!(
            "{}",
            tf("error.agent_empty_field", &[("field", field_name)])
        );
    }

    // Detect newline characters (possible multiline secret or injection attempt)
    if secret.contains('\n') || secret.contains('\r') {
        warn!(
            "{} contains newline characters, which may be a security issue",
            field_name
        );
    }

    // Check minimum secret length
    if secret.len() < 8 {
        warn!(
            "{} is very short ({} characters), which may be insecure",
            field_name,
            secret.len()
        );
    }

    // Detect common insecure patterns
    let insecure_patterns = [
        ("password", "contains the word 'password'"),
        ("123456", "contains simple numeric sequence"),
        ("admin", "contains the word 'admin'"),
        ("test", "contains the word 'test'"),
        ("secret", "contains the word 'secret'"),
    ];

    let secret_lower = secret.to_lowercase();
    for (pattern, description) in insecure_patterns {
        if secret_lower.contains(pattern) {
            warn!(
                "{} {} - consider using a stronger secret",
                field_name, description
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, AppConfig, FlowConfig, PhaseConfig, RuntimeConfig};
    use crate::intelligence::capability_graph::CapabilityGraph;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

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
            scheduler: None,
            reputation: None,
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
}
