//! Providers view — listing, editing, catalog, and Copilot OAuth.
//!
//! Sub-modules (see F-GAP-65 for rendering migration):
//! - `catalog` — provider catalog fetched from backend RPC (offline fallback
//!   lives in `generated_catalog`, which is generated from the backend's
//!   `built_in_provider_specs()` — see scripts/gen-provider-catalog.py)
//! - `render` — monolithic `show()` method, to be split into sub-modules

pub mod catalog;
mod generated_catalog;
mod render;

pub use generated_catalog::PROVIDER_NAMES;

use crate::backend::{BackendClient, ProviderCapabilityModel};
use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::connection::{build_copilot_http_client, COPILOT_HTTP_CLIENT};
use crate::i18n::I18n;
use crate::views::security_prefs;
use serde_json::Value;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct ProvidersView {
    /// Selected provider name from a predefined list (for Add)
    pub selected_provider: String,
    new_key: String,
    /// Additional secret key for providers like wenxin that need dual-auth
    new_secret_key: String,
    pub new_model: String,
    /// Label distinguishing multiple instances of the same provider
    pub new_label: String,
    /// Index of provider being updated in existing list (-1 = none)
    update_target: isize,
    status: String,
    sending: bool,
    pending_delete_confirmation: Option<usize>,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::SyncSender<String>,
    /// Models fetched from backend: provider → [model_id, ...]
    remote_models: std::collections::HashMap<String, Vec<String>>,
    /// Whether we've tried to fetch models
    models_loaded: bool,
    /// Provider names loaded from backend catalog (fallback to built-in list).
    provider_names: Vec<String>,
    /// Whether we've tried to fetch provider catalog.
    catalog_loaded: bool,
    provider_ops_status: std::collections::HashMap<String, String>,
    provider_capabilities: std::collections::HashMap<String, Vec<ProviderCapabilityModel>>,
    /// Cached security prefs — reloaded at most once per 10s to avoid per-frame disk reads.
    cached_security: security_prefs::SecurityPrefs,
    security_last_load: Instant,

    // ── GitHub Copilot OAuth Device Code state ──
    /// Current step: None = idle, "requesting", "polling", "done", "error"
    copilot_device_state: Option<String>,
    /// The device_code returned by GitHub (for polling)
    copilot_device_code: String,
    /// The user_code displayed to the user
    copilot_user_code: String,
    /// The verification URI (e.g. https://github.com/login/device)
    copilot_verification_uri: String,
    /// Polling interval in seconds (from server)
    copilot_poll_interval: u64,
    /// Timestamp of last poll attempt
    copilot_last_poll: Instant,
    /// Number of poll attempts for current device flow
    copilot_poll_attempts: u64,
    /// Number of times GitHub requested slower polling
    copilot_slow_down_count: u64,
    /// Last poll result summary for debugging
    copilot_last_poll_result: String,
    /// Access token obtained after authorization
    copilot_access_token: String,
    /// Whether the copilot token was written into new_key (shared field)
    copilot_token_stored: bool,
    /// Status message for the copilot auth section
    copilot_status: String,
    /// Whether we've already scheduled a repaint for the current polling cycle.
    /// Avoids calling request_repaint_after every frame while polling.
    copilot_poll_repaint_requested: bool,
}

// Provider names (offline fallback) live in `generated_catalog` — generated
// from the backend's `built_in_provider_specs()` by
// `scripts/gen-provider-catalog.py`. Re-exported above as PROVIDER_NAMES.

fn provider_label(i18n: &I18n, provider: &str) -> String {
    let key = format!("provider.{}", provider.to_lowercase());
    let label = i18n.t(&key);
    if label.as_ref() == key {
        provider.to_string()
    } else {
        label.into_owned()
    }
}

/// GUI-side hardcoded model suggestions per provider.
///
/// These are user-facing model suggestions that MUST match the model IDs
/// returned by the backend agents' `available_models()` in `src/agents/*.rs`.
/// The backend agent uses `self.model` as the model ID sent in API requests,
/// so the GUI suggestions must use model IDs the provider API actually accepts.
pub(crate) fn models_for_provider(provider: &str) -> &'static [&'static str] {
    match provider.to_lowercase().as_str() {
        "deepseek" => &[
            "auto",
            "deepseek-v4-flash",
            "deepseek-v4-pro",
            "deepseek-r1",
        ],
        "openai" => &[
            "auto",
            "gpt-4.1",
            "gpt-4.1-mini",
            "gpt-4.1-nano",
            "o3-mini",
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-3.5-turbo",
        ],
        "openai_compatible" => &["auto"],
        "anthropic" => &[
            "auto",
            "claude-opus-4-7",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
            "claude-3-5-sonnet",
            "claude-3-opus",
            "claude-3-haiku",
        ],
        "cohere" => &[
            "auto",
            "command-a-03-2025",
            "command-a-reasoning-08-2025",
            "command-r7b-12-2024",
            "command-r-plus-08-2024",
            "command-r-08-2024",
        ],
        "wenxin" => &[
            "auto",
            "ERNIE-4.5-8K",
            "ernie-4.0-turbo-8k",
            "ernie-3.5-turbo",
        ],
        "qianfan" => &[
            "auto",
            "ERNIE-4.5-8K",
            "ernie-4.0-8k",
            "ernie-3.5-8k",
            "ernie-speed",
            "ernie-lite",
        ],
        "qwen" => &[
            "auto",
            "qwen-max",
            "qwen-plus",
            "qwen-turbo",
            "qwen2.5-72b-instruct",
        ],
        "glm" => &["auto", "glm-4-flash", "glm-4v", "glm-4-plus", "glm-3-turbo"],
        "yi" => &["auto", "yi-lightning", "yi-large"],
        "hunyuan" => &[
            "auto",
            "hunyuan-turbo-latest",
            "hunyuan-turbo",
            "hunyuan-pro",
        ],
        "doubao" => &["auto", "doubao-1.5-pro-32k-250115"],
        "gemini" => &[
            "auto",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
            "gemini-2.5-pro",
            "gemini-3.1-pro-preview-03-2026",
            "gemini-3-flash-preview-03-2026",
            "gemini-2.0-flash",
            "gemini-2.0-pro",
        ],
        "groq" => &[
            "auto",
            "llama-3.3-70b-versatile",
            "llama-3.1-8b-instant",
            "openai/gpt-oss-120b",
            "qwen/qwen3-32b",
        ],
        "mistral" => &[
            "auto",
            "mistral-large-2512",
            "mistral-medium-2508",
            "mistral-small-2603",
        ],
        "copilot" => &[
            "auto",
            "claude-opus-4",
            "claude-sonnet-4",
            "gemini-2.5-pro",
            "gpt-5",
            "gpt-4.1",
            "gpt-4o",
            "o1",
            "o3-mini",
            "gpt-5-mini",
            "gpt-4.1-mini",
            "gpt-4o-mini",
            "claude-3.5-sonnet",
            "gemini-2.0-flash-001",
        ],
        "siliconflow" => &[
            "auto",
            "deepseek-ai/DeepSeek-V3.2",
            "deepseek-ai/DeepSeek-R1",
            "deepseek-ai/DeepSeek-V2.5",
            "Qwen/Qwen2.5-72B-Instruct-128K",
            "Qwen/Qwen2.5-32B-Instruct",
            "Qwen/QwQ-32B",
            "TeleAI/TeleChat-T2",
            "THUDM/glm-4-9b-chat",
            "internlm/internlm2_5-20b-chat",
        ],

        "stepfun" => &["auto", "step-2-16k", "step-1-8k", "step-1-flash"],
        "moonshot" => &[
            "auto",
            "moonshot-v1-8k",
            "moonshot-v1-32k",
            "moonshot-v1-128k",
        ],
        "kimi" => &[
            "auto",
            "kimi-k2.6",
            "kimi-k2.5",
            "kimi-k2",
            "kimi-k2-thinking",
            "moonshot-v1",
        ],
        "minimax" => &["auto", "MiniMax-Text-01", "MiniMax-Text-01-mini"],
        "ai21" => &["auto", "jamba-1.5-mini", "jamba-1.5-large"],
        "aleph" => &["auto", "luminous-base", "luminous-extended"],
        "llama" => &["auto", "llama3.2", "llama3.2-vision"],
        "nim" => &[
            "auto",
            "meta/llama-3.1-70b-instruct",
            "meta/llama-3.1-405b-instruct",
            "mistralai/mixtral-8x22b-instruct",
        ],
        "perplexity" => &[
            "auto",
            "sonar-pro",
            "sonar",
            "sonar-reasoning-pro",
            "sonar-deep-research",
        ],
        "replicate" => &[
            "auto",
            "meta/meta-llama-3-70b-instruct",
            "meta/meta-llama-3-8b-instruct",
        ],
        "together" => &[
            "auto",
            "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo",
            "meta-llama/Meta-Llama-3.1-405B-Instruct-Turbo",
            "mistralai/Mixtral-8x22B-Instruct-v0.1",
        ],
        "xai" => &["auto", "grok-2", "grok-3"],
        "fireworks" => &[
            "auto",
            "accounts/fireworks/models/llama-v3p1-8b-instruct",
            "accounts/fireworks/models/llama-v3p1-405b-instruct",
            "accounts/fireworks/models/mixtral-8x22b-instruct",
        ],
        "deepquest" => &["auto", "deepquest-chat", "deepquest-chat-large"],
        "facewall" => &["auto", "facewall-chat", "facewall-chat-large"],
        "langboat" => &["auto", "langboat-chat", "langboat-chat-large"],
        "loopai" => &["auto", "loopai-chat", "loopai-chat-pro"],
        "skywork" => &["auto", "skywork-chat", "skywork-chat-large"],
        "titan" => &[
            "auto",
            "amazon.titan-text-premier-v1:0",
            "amazon.titan-text-express-v1",
        ],
        "xihu" => &["auto", "xihu-chat", "xihu-chat-large"],
        _ => &["auto"],
    }
}

pub(crate) fn provider_requires_secret(provider: &str) -> bool {
    matches!(provider.to_lowercase().as_str(), "wenxin" | "qianfan")
}

impl ProvidersView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::sync_channel(256);
        Self {
            selected_provider: PROVIDER_NAMES[0].to_string(),
            new_key: String::new(),
            new_secret_key: String::new(),
            new_model: "auto".to_string(),
            new_label: String::new(),
            update_target: -1,
            status: String::new(),
            sending: false,
            pending_delete_confirmation: None,
            pending_rx,
            pending_tx,
            remote_models: std::collections::HashMap::new(),
            models_loaded: false,
            provider_names: PROVIDER_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            catalog_loaded: false,
            provider_ops_status: std::collections::HashMap::new(),
            provider_capabilities: std::collections::HashMap::new(),
            cached_security: security_prefs::load(),
            security_last_load: Instant::now(),

            // Copilot Device Code state
            copilot_device_state: None,
            copilot_device_code: String::new(),
            copilot_user_code: String::new(),
            copilot_verification_uri: String::new(),
            copilot_poll_interval: 5,
            copilot_last_poll: Instant::now(),
            copilot_poll_attempts: 0,
            copilot_slow_down_count: 0,
            copilot_last_poll_result: String::new(),
            copilot_access_token: String::new(),
            copilot_token_stored: false,
            copilot_status: String::new(),
            copilot_poll_repaint_requested: false,
        }
    }

    pub fn reset_loaded_state(&mut self) {
        self.models_loaded = false;
        self.catalog_loaded = false;
        self.remote_models.clear();
        self.provider_capabilities.clear();
        self.provider_ops_status.clear();
    }

    fn process_pending(&mut self, i18n: &I18n, config: &mut AppConfig) {
        // Limit event processing per frame to prevent UI freeze
        const MAX_EVENTS_PER_FRAME: usize = 12;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Ok(msg) = self.pending_rx.try_recv() else {
                break;
            };
            if let Some(models_json) = msg.strip_prefix("__models__:") {
                if let Ok(models) = serde_json::from_str::<
                    std::collections::HashMap<String, Vec<String>>,
                >(models_json)
                {
                    self.remote_models = models;
                }
            } else if let Some(catalog_json) = msg.strip_prefix("__catalog__:") {
                if let Ok(value) = serde_json::from_str::<Value>(catalog_json) {
                    if let Some(items) = value.get("catalog").and_then(Value::as_array) {
                        let mut names = items
                            .iter()
                            .filter_map(|item| item.get("name").and_then(Value::as_str))
                            .map(ToString::to_string)
                            .collect::<Vec<_>>();
                        names.sort();
                        names.dedup();
                        if !names.is_empty() {
                            self.provider_names = names;
                            if !self
                                .provider_names
                                .iter()
                                .any(|name| name.eq_ignore_ascii_case(&self.selected_provider))
                            {
                                self.selected_provider = self.provider_names[0].clone();
                            }
                        }
                    }
                }
            } else if let Some(rest) = msg.strip_prefix("__ops__:") {
                if let Some((provider, status)) = rest.split_once(':') {
                    self.provider_ops_status
                        .insert(provider.to_string(), status.to_string());
                }
            } else if let Some(rest) = msg.strip_prefix("__caps__:") {
                if let Some((provider, payload)) = rest.split_once(':') {
                    if let Ok(models) =
                        serde_json::from_str::<Vec<ProviderCapabilityModel>>(payload)
                    {
                        self.provider_capabilities
                            .insert(provider.to_string(), models);
                    }
                }
            } else if let Some(rest) = msg.strip_prefix("__copilot_device__:") {
                // Initial device code response
                if let Ok(resp) = serde_json::from_str::<serde_json::Value>(rest) {
                    self.copilot_device_code =
                        resp["device_code"].as_str().unwrap_or("").to_string();
                    self.copilot_user_code = resp["user_code"].as_str().unwrap_or("").to_string();
                    self.copilot_verification_uri = resp["verification_uri"]
                        .as_str()
                        .unwrap_or("https://github.com/login/device")
                        .to_string();
                    self.copilot_poll_interval = resp["interval"].as_u64().unwrap_or(5).max(5);
                    self.copilot_status = String::new();
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "[device] user_code={}, uri={}, interval={}",
                        self.copilot_user_code,
                        self.copilot_verification_uri,
                        self.copilot_poll_interval
                    );
                    self.copilot_device_state = Some("polling".to_string());
                    self.copilot_last_poll = Instant::now();
                    self.copilot_poll_repaint_requested = false;
                    self.copilot_poll_attempts = 0;
                    self.copilot_slow_down_count = 0;
                    self.copilot_last_poll_result.clear();
                    self.copilot_status = String::new();
                } else {
                    self.copilot_device_state = Some("error".to_string());
                    self.copilot_status = "Failed to parse device code response".to_string();
                }
            } else if let Some(err_msg) = msg.strip_prefix("__copilot_device_err__:") {
                self.copilot_device_state = Some("error".to_string());
                self.copilot_status = err_msg.to_string();
            } else if let Some(rest) = msg.strip_prefix("__copilot_poll__:") {
                // Poll response — GitHub returns access_token on success, or error field on failure.
                if let Ok(resp) = serde_json::from_str::<serde_json::Value>(rest) {
                    // Check for access_token first (success case)
                    if let Some(token) = resp.get("access_token").and_then(Value::as_str) {
                        if !token.is_empty() {
                            self.copilot_last_poll_result =
                                "authorized(access_token received)".to_string();
                            self.copilot_access_token = token.to_string();
                            self.copilot_device_state = Some("done".to_string());
                            self.copilot_status =
                                i18n.t("providers.copilot_authorized").to_string();
                            self.new_key = self.copilot_access_token.clone();
                            self.copilot_token_stored = true;
                            // Immediately persist to keyring so the token survives app restart
                            // even if the user doesn't click Save.
                            //
                            // NOTE: Dual storage is intentional.
                            // - store_api_key("copilot", …) stores under the "copilot" service name
                            //   used by the frontend's keyring_util system for general provider keys.
                            // - keyring::Entry::new("go-on", "github_copilot_token") stores under
                            //   "github_copilot_token" so the backend process (which reads this
                            //   specific entry) can also access the token on restart.
                            if let Err(e) = crate::keyring_util::store_api_key("copilot", token) {
                                eprintln!("Warning: failed to store Copilot token in keyring (copilot_api_key): {e}");
                            }
                            // Also write to github_copilot_token for backend compatibility
                            // with macOS ACL configured so the backend process can read it.
                            if let Err(e) = crate::keyring_util::store_copilot_token(token) {
                                eprintln!("Warning: failed to store Copilot token in keyring (github_copilot_token): {e}");
                            }
                            // Auto-create a Copilot provider entry in config so the user
                            // doesn't need to manually click 'Add' after OAuth completes.
                            if !config
                                .providers
                                .iter()
                                .any(|p| p.name.eq_ignore_ascii_case("copilot"))
                            {
                                config.providers.push(ProviderConfig {
                                    name: "copilot".to_string(),
                                    api_key: token.to_string(),
                                    secret_key: String::new(),
                                    model: "auto".to_string(),
                                    validated: true,
                                    label: String::new(),
                                });
                            }
                            // Note: tokens are persisted to keyring above (both copilot_api_key
                            // and github_copilot_token). The backend receives them via env vars
                            // populated from keyring_util at spawn time.
                        }
                    } else if let Some(error) = resp.get("error").and_then(Value::as_str) {
                        self.copilot_last_poll_result = format!("oauth_error={}", error);
                        match error {
                            "authorization_pending" => {
                                self.copilot_status =
                                    i18n.t("providers.copilot_waiting").to_string();
                            }
                            "slow_down" => {
                                self.copilot_slow_down_count =
                                    self.copilot_slow_down_count.saturating_add(1);
                                // RFC 8628: increase polling interval by at least 5s on slow_down.
                                self.copilot_poll_interval =
                                    self.copilot_poll_interval.saturating_add(5).min(60);
                                self.copilot_status = format!(
                                    "{} (backoff to {} {})",
                                    i18n.t("providers.copilot_waiting"),
                                    self.copilot_poll_interval,
                                    i18n.t("common.seconds")
                                );
                            }
                            "expired_token" => {
                                self.copilot_device_state = Some("error".to_string());
                                self.copilot_status =
                                    i18n.t("providers.copilot_expired").to_string();
                            }
                            "access_denied" => {
                                self.copilot_device_state = Some("error".to_string());
                                self.copilot_status =
                                    i18n.t("providers.copilot_denied").to_string();
                            }
                            _ => {
                                self.copilot_device_state = Some("error".to_string());
                                self.copilot_status =
                                    format!("{} {}", i18n.t("common.error"), error);
                            }
                        }
                    }
                } else {
                    self.copilot_last_poll_result = "parse_error(response json)".to_string();
                    self.copilot_device_state = Some("error".to_string());
                    self.copilot_status = "Failed to parse poll response".to_string();
                }
            } else if let Some(err_msg) = msg.strip_prefix("__copilot_poll_err__:") {
                self.copilot_last_poll_result = format!("request_error={}", err_msg);
                self.copilot_device_state = Some("error".to_string());
                self.copilot_status = err_msg.to_string();
            } else {
                self.sending = false;
                self.status = msg;
            }
        }
    }

    /// Fetch models from backend on first load. Spawns a background task.
    fn ensure_models_loaded(&mut self, backend: &BackendClient, ctx: &egui::Context) {
        if !self.models_loaded {
            self.models_loaded = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                let models: std::collections::HashMap<String, Vec<String>> = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.fetch_models(),
                )
                .await
                .unwrap_or_else(|_| {
                    #[cfg(debug_assertions)]
                    eprintln!("Warning: Failed to fetch models from backend (timeout)");
                    std::collections::HashMap::new()
                });
                let msg = format!(
                    "__models__:{}",
                    serde_json::to_string(&models).unwrap_or_default()
                );
                if let Err(e) = tx.try_send(msg) {
                    eprintln!("WARN: providers try_send failed: {:?}", e);
                }
                ctx_clone.request_repaint();
            });
        }

        if !self.catalog_loaded {
            self.catalog_loaded = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                let catalog = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.provider_catalog(),
                )
                .await
                {
                    Ok(Ok(value)) => value,
                    _ => Value::Null,
                };
                let msg = format!(
                    "__catalog__:{}",
                    serde_json::to_string(&catalog).unwrap_or_default()
                );
                if let Err(e) = tx.try_send(msg) {
                    eprintln!("WARN: providers try_send failed: {:?}", e);
                }
                ctx_clone.request_repaint();
            });
        }
    }

    /// Fetch models from backend on first load. Spawns a background task.
    /// Reload security prefs at most once per 10 seconds.
    fn refresh_security_cache(&mut self) {
        if self.security_last_load.elapsed() >= std::time::Duration::from_secs(10) {
            self.cached_security = security_prefs::load();
            self.security_last_load = Instant::now();
        }
    }

    fn backend_models_for_provider(&self, provider: &str) -> Option<Vec<String>> {
        let key = provider.to_lowercase();
        self.remote_models.iter().find_map(|(name, models)| {
            if name.eq_ignore_ascii_case(&key) || name.eq_ignore_ascii_case(provider) {
                Some(models.clone())
            } else {
                None
            }
        })
    }

    fn available_models_for_provider(&self, provider: &str) -> Vec<String> {
        let mut models = Vec::<String>::new();
        models.push("auto".to_string());

        if let Some(remote) = self.backend_models_for_provider(provider) {
            for model in remote {
                if !model.trim().is_empty() && model != "auto" {
                    models.push(model);
                }
            }
        }

        for fallback in models_for_provider(provider) {
            if *fallback != "auto" {
                models.push((*fallback).to_string());
            }
        }

        let mut deduped = Vec::<String>::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for model in models {
            if seen.insert(model.clone()) {
                deduped.push(model);
            }
        }
        deduped
    }
}
