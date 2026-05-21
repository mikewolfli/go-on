use crate::backend::BackendClient;
use crate::config::{has_valid_providers, save_app_config, AppConfig};
use crate::views::ui_state::GlobalUiState;

/// Write a line to go-on-gui.log in the temp directory.
/// Only active in debug builds to avoid blocking the UI thread.
pub fn log_msg(msg: &str) {
    #[cfg(debug_assertions)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        let path = std::env::temp_dir().join("go-on-gui.log");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{}", msg);
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = msg;
}

use crate::i18n::{I18n, Lang};
use crate::keyring_util::REDACTED_API_KEY;
use crate::views::chat::ChatUiRuntimeConfig;
use crate::views::{
    about::AboutView, autotune::AutoTuneView, chat::ChatView, config_editor::ConfigEditorView,
    monitor::MonitorView, prompts::PromptsView, providers::ProvidersView,
    risk_decision::RiskDecisionView, security::SecurityView, settings::SettingsView,
    setup::SetupView, skills::SkillsView, workflow::WorkflowView,
};
use std::hash::{Hash, Hasher};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use std::time::Instant;

enum BackendUpdate {
    Health(HealthStatus),
    Providers(Vec<ProviderStatus>),
    RefreshDone,
}

use crate::backend::{HealthStatus, ProviderStatus};
use std::collections::hash_map::DefaultHasher;
use std::net::{TcpStream, ToSocketAddrs};

/// Find the go-on backend binary path relative to the GUI executable.
fn find_backend_binary() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe_name = if cfg!(target_os = "windows") {
        "go-on.exe"
    } else {
        "go-on"
    };
    let mut candidates = vec![
        exe_dir.join("backend").join(exe_name),
        exe_dir.join(exe_name),
    ];
    // Also search in Resources/backend (macOS .app bundle layout)
    if let Some(resources) = exe_dir.parent().map(|p| p.join("Resources")) {
        candidates.push(resources.join("backend").join(exe_name));
        candidates.push(resources.join(exe_name));
    }
    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(exe_name);
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

fn backend_log_path() -> Option<std::path::PathBuf> {
    find_backend_binary().and_then(|path| path.parent().map(|p| p.join("backend.log")))
}

fn backend_log_has_addr_in_use() -> bool {
    backend_log_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|s| s.contains("Address already in use"))
}

fn backend_bind_addr_from_url(url: &str) -> String {
    let without_scheme = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    match without_scheme.find('/') {
        Some(pos) => without_scheme[..pos].to_string(),
        None => without_scheme.to_string(),
    }
}

fn is_addr_listening(addr: &str) -> bool {
    let Ok(candidates) = addr.to_socket_addrs() else {
        return false;
    };
    candidates
        .into_iter()
        .any(|sock| TcpStream::connect_timeout(&sock, Duration::from_millis(150)).is_ok())
}

pub struct GoOnApp {
    pub config: AppConfig,
    config_shared: Arc<AppConfig>,
    config_shared_fingerprint: u64,
    pub i18n: I18n,
    pub backend: BackendClient,
    pub setup_view: SetupView,
    pub monitor_view: MonitorView,
    pub chat_view: ChatView,
    pub skills_view: SkillsView,
    pub workflow_view: WorkflowView,
    pub autotune_view: AutoTuneView,
    pub security_view: SecurityView,
    pub config_editor_view: ConfigEditorView,
    pub prompts_view: PromptsView,
    pub risk_decision_view: RiskDecisionView,
    pub providers_view: ProvidersView,
    pub about_view: AboutView,
    pub show_setup: bool,
    pub active_tab: String,
    pub has_providers: bool,
    backend_updates: mpsc::Receiver<BackendUpdate>,
    backend_tx: mpsc::SyncSender<BackendUpdate>,
    pending_refresh: bool,
    last_refresh: Instant,
    /// Managed backend child process
    backend_child: Option<std::process::Child>,
    /// True when GUI reuses an already-running backend listener instead of spawning child.
    backend_reused_external: bool,
    /// Cache the last applied theme name to avoid calling ctx.set_style() every frame.
    last_applied_theme: String,
    /// Track when the backend crashed to enable auto-restart with rate limiting
    backend_crash_time: Option<Instant>,
    /// Hash of backend URL to detect changes for cache invalidation
    last_backend_url_hash: u64,
    /// Original backend URL to detect changes for showing restart button
    backend_url_original: String,
    /// Timestamp of when blocked tab toast was shown; used for auto-dismiss.
    blocked_tab_toast_shown: Option<Instant>,
    /// Staging buffer for backend health updates; committed in batches to reduce UI jitter.
    staged_health: Option<HealthStatus>,
    /// Staging buffer for provider updates; committed in batches to reduce UI jitter.
    staged_providers: Option<Vec<ProviderStatus>>,
    /// Marks the end of a refresh cycle so staged values can be committed atomically.
    staged_refresh_done: bool,
    /// Last time staged backend data was committed into visible UI state.
    last_backend_ui_commit: Instant,
    /// Consecutive backend disconnect samples; used to debounce transient failures.
    health_disconnect_streak: u8,
    /// Tracks the last seen prompts command version to avoid cloning every frame.
    last_prompts_command_version: u64,
    /// Last language used to load prompts data for chat command/category browser.
    last_prompts_lang: Lang,
    /// Persistent UI state shared across all views
    pub ui_state: GlobalUiState,
    /// Count of consecutive backend crashes for rate limiting
    backend_crash_count: u8,
    /// Consecutive backend health poll failures for progressive backoff
    consecutive_poll_failures: u8,
}

/// Detect system locale from environment variables.
/// Checks LC_ALL, LC_MESSAGES, LANG on Unix; LANGUAGE as additional fallback.
fn detect_system_language() -> Lang {
    for var in &["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Some(val) = std::env::var_os(var) {
            // Normalize: lowercase and replace hyphens with underscores so
            // that hyphen-form locales (e.g. "zh-CN", "zh-Hant-TW") are handled.
            let s = val.to_string_lossy().to_lowercase().replace('-', "_");
            if s.contains("zh_cn") || s.contains("chinese") {
                return Lang::ZhCn;
            }
            // zh_hk (Hong Kong) uses Traditional Chinese like Taiwan.
            if s.contains("zh_tw")
                || s.contains("zh_hk")
                || s.contains("taiwan")
                || s.contains("hant")
            {
                return Lang::ZhTw;
            }
            // Generic Chinese — prefer Simplified as the wider default
            if s.contains("zh") {
                return Lang::ZhCn;
            }
        }
    }
    Lang::En
}

impl GoOnApp {
    fn config_fingerprint(config: &AppConfig) -> u64 {
        let mut hasher = DefaultHasher::new();
        config.backend_url.hash(&mut hasher);
        config.language.hash(&mut hasher);
        config.theme.hash(&mut hasher);
        config
            .ui_stability
            .backend_refresh_interval_secs
            .hash(&mut hasher);
        config
            .ui_stability
            .backend_ui_commit_debounce_ms
            .hash(&mut hasher);
        config
            .ui_stability
            .health_disconnect_debounce_count
            .hash(&mut hasher);
        config
            .ui_stability
            .chat_stream_chunk_flush_ms
            .hash(&mut hasher);
        config
            .ui_stability
            .chat_repaint_interval_ms
            .hash(&mut hasher);
        config
            .ui_stability
            .chat_max_pending_events_per_frame
            .hash(&mut hasher);
        config.features.monitor.hash(&mut hasher);
        config.features.chat.hash(&mut hasher);
        config.features.skills.hash(&mut hasher);
        config.features.workflow.hash(&mut hasher);
        config.features.autotune.hash(&mut hasher);
        config.features.security.hash(&mut hasher);
        config.features.config.hash(&mut hasher);
        config.features.providers.hash(&mut hasher);
        config.features.workflow_run_center.hash(&mut hasher);
        config.features.autotune_chain_injection.hash(&mut hasher);
        config.features.skills_lifecycle.hash(&mut hasher);
        config.features.providers_ops.hash(&mut hasher);
        config.features.monitor_history_alerts.hash(&mut hasher);
        config.features.config_safe_mode.hash(&mut hasher);
        config.features.setup_enterprise.hash(&mut hasher);
        config.features.show_prompts_tab.hash(&mut hasher);
        config.features.show_risk_decision_tab.hash(&mut hasher);
        for provider in &config.providers {
            provider.name.hash(&mut hasher);
            provider.model.hash(&mut hasher);
            provider.validated.hash(&mut hasher);
            provider.api_key.hash(&mut hasher);
        }
        hasher.finish()
    }

    fn sync_shared_config_if_needed(&mut self) {
        let fingerprint = Self::config_fingerprint(&self.config);
        if fingerprint != self.config_shared_fingerprint {
            self.config_shared = Arc::new(self.config.clone());
            self.config_shared_fingerprint = fingerprint;
        }
    }

    fn backend_refresh_interval(&self) -> Duration {
        Duration::from_secs(
            self.config_shared
                .ui_stability
                .backend_refresh_interval_secs
                .clamp(1, 60),
        )
    }

    fn backend_ui_commit_debounce(&self) -> Duration {
        Duration::from_millis(
            self.config_shared
                .ui_stability
                .backend_ui_commit_debounce_ms
                .clamp(16, 1000),
        )
    }

    fn health_disconnect_debounce_count(&self) -> u8 {
        self.config_shared
            .ui_stability
            .health_disconnect_debounce_count
            .clamp(1, 8)
    }

    /// Print diagnostic info about key sources for debugging.
    /// Only active in debug builds to avoid unnecessary keyring calls in production.
    fn diagnostic_key_report(config: &AppConfig) {
        #[cfg(not(debug_assertions))]
        let _ = config;

        #[cfg(debug_assertions)]
        {
            eprintln!("=== KEY DIAGNOSTIC ===");
            // Collect all provider names: from config.providers AND the canonical PROVIDER_NAMES list.
            // Use a HashSet to avoid duplicate keyring lookups.
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut all_names: Vec<String> = Vec::new();
            for p in &config.providers {
                let lower = p.name.to_lowercase();
                if seen.insert(lower.clone()) {
                    all_names.push(lower);
                }
            }
            for name in crate::views::providers::PROVIDER_NAMES {
                let lower = name.to_lowercase();
                if seen.insert(lower.clone()) {
                    all_names.push(lower);
                }
            }
            for name in &all_names {
                let config_key = config
                    .providers
                    .iter()
                    .find(|p| p.name.to_lowercase() == *name)
                    .map(|p| {
                        if p.api_key.is_empty() {
                            "(empty)".to_string()
                        } else {
                            format!("{}...", &p.api_key[..4.min(p.api_key.len())])
                        }
                    })
                    .unwrap_or_else(|| "(not in config)".to_string());
                let keyring_key = crate::keyring_util::get_api_key(name)
                    .map(|k| format!("{}...", &k[..4.min(k.len())]))
                    .unwrap_or_else(|| "(not in keyring)".to_string());
                eprintln!("  {}: config={}, keyring={}", name, config_key, keyring_key);
            }
            eprintln!("=== END DIAGNOSTIC ===");
        }
    }

    /// Start or restart the backend child process with fresh env vars from keyring.
    fn spawn_backend(config: &AppConfig) -> (BackendClient, Option<std::process::Child>, bool) {
        Self::diagnostic_key_report(config);

        let bind_addr = backend_bind_addr_from_url(&config.backend_url);
        if is_addr_listening(&bind_addr) {
            eprintln!(
                "backend: detected existing listener at {}; reusing external backend",
                bind_addr
            );
            return (BackendClient::new(&config.backend_url), None, true);
        }

        match find_backend_binary() {
            Some(path) => {
                let config_dir: std::borrow::Cow<'_, std::path::Path> = match path.parent() {
                    Some(parent) => std::borrow::Cow::Borrowed(parent),
                    None => {
                        let home = std::env::var("HOME")
                            .or_else(|_| std::env::var("USERPROFILE"))
                            .unwrap_or_else(|_| ".".to_string());
                        std::borrow::Cow::Owned(std::path::PathBuf::from(home))
                    }
                };
                let mut cmd = std::process::Command::new(&path);
                cmd.current_dir(&config_dir)
                    .arg("--protocol-mode")
                    .arg(&config.protocol_mode)
                    .stdout(std::process::Stdio::null());

                // Inject API keys for ALL configured providers into backend process environment.
                // Priority: config file (fast) > keyring (slow on macOS) > inherited env.
                // Keyring is only checked when the config doesn't have a usable key,
                // since macOS Keychain access is ~100ms slower per call.
                for provider in &config.providers {
                    let provider_lower = provider.name.to_lowercase();
                    let derived_var = format!("{}_API_KEY", provider_lower.to_uppercase());
                    let env_var = match provider_lower.as_str() {
                        "copilot" => "GITHUB_COPILOT_TOKEN",
                        "replicate" => "REPLICATE_API_TOKEN",
                        _ => &derived_var,
                    };

                    // 1. Config file (fast, no I/O). Use this directly if available.
                    let mut key =
                        if !provider.api_key.is_empty() && provider.api_key != REDACTED_API_KEY {
                            Some(provider.api_key.clone())
                        } else {
                            None
                        };

                    // 2. Keyring (slow on macOS — Keychain access per call).
                    // Only hit keyring when config doesn't have the key.
                    if key.is_none() {
                        key = crate::keyring_util::get_api_key(&provider_lower);
                        #[cfg(debug_assertions)]
                        if key.is_none() {
                            eprintln!("backend: keyring returned no key for '{}'", provider_lower);
                        }
                    }

                    // 3. Inherited env var (from parent process)
                    if key.is_none() {
                        key = std::env::var(env_var).ok().filter(|v| !v.is_empty());
                    }

                    if let Some(k) = key {
                        let _preview = if k.len() > 4 {
                            format!("{}...", &k[..4])
                        } else {
                            "[short]".to_string()
                        };
                        eprintln!("backend: set {}={}", env_var, _preview);
                        cmd.env(env_var, k);
                    } else {
                        #[cfg(debug_assertions)]
                        eprintln!("backend: no key found for '{}'", provider_lower);
                    }
                }

                // Sync language between GUI and backend
                cmd.env("LANG", &config.language);

                // Regenerate backend config.toml on every start to keep it
                // in sync with the GUI's provider configuration.
                // The file includes a header marking it as auto-generated.
                let backend_cfg_path = config_dir.join("config.toml");
                Self::generate_backend_config(&backend_cfg_path, config);

                let log_path = config_dir.join("backend.log");
                // Redirect stderr directly to file instead of spawning a reader thread
                match std::fs::File::create(&log_path) {
                    Ok(log_file) => {
                        cmd.stderr(log_file);
                    }
                    Err(e) => {
                        eprintln!("Failed to create backend.log: {e}; stderr will go to parent");
                        cmd.stderr(std::process::Stdio::inherit());
                    }
                }
                match cmd.spawn() {
                    Ok(child) => {
                        eprintln!("go-on backend started (PID: {})", child.id());
                        (BackendClient::new(&config.backend_url), Some(child), false)
                    }
                    Err(e) => {
                        eprintln!("warning: failed to start backend: {}", e);
                        (BackendClient::new(&config.backend_url), None, false)
                    }
                }
            }
            None => {
                eprintln!("warning: go-on backend binary not found");
                (BackendClient::new(&config.backend_url), None, false)
            }
        }
    }

    /// Generate a backend config.toml with all configured providers.
    /// Called every time the backend is (re)started to keep the config in sync
    /// with the GUI's provider list.
    ///
    /// Uses `keyring://go-on/<provider>_api_key` references so the backend reads
    /// API keys from the system keyring (libsecret on Linux, Credential Manager on
    /// Windows, Keychain on macOS). The backend also falls back to env vars if the
    /// keyring is unavailable — see `load_secret_value()` in the backend code.
    fn generate_backend_config(path: &std::path::Path, config: &AppConfig) {
        // Canonical provider metadata — maps provider name to (agent_type, url, default_model, supports_system).
        // This is the GUI-side hardcoded duplicate of the backend's built_in_provider_specs().
        // Keep in sync with `src/core/config.rs` and `src/core/setup.rs`.
        // NOTE: `built_in_provider_specs()` in the backend is the authoritative source.
        fn provider_meta(name: &str) -> (&'static str, Option<&'static str>, &'static str, bool) {
            match name {
                "openai" => (
                    "openai",
                    Some("https://api.openai.com/v1"),
                    "gpt-4o-mini",
                    true,
                ),
                "openai_compatible" => (
                    "openai_compatible",
                    Some("http://127.0.0.1:8080/v1"),
                    "compatible-model",
                    true,
                ),
                "anthropic" => (
                    "claude",
                    Some("https://api.anthropic.com"),
                    "claude-sonnet-4-20250514",
                    true,
                ),
                "cohere" => (
                    "cohere",
                    Some("https://api.cohere.ai/v1"),
                    "command-r-plus-08-2024",
                    true,
                ),
                "deepseek" => (
                    "deepseek",
                    Some("https://api.deepseek.com"),
                    "deepseek-v4-flash",
                    true,
                ),
                "wenxin" => ("wenxin", None, "ERNIE-4.5-8K", false),
                "qianfan" => ("qianfan", None, "ERNIE-4.5-8K", false),
                "qwen" => (
                    "qwen",
                    Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
                    "qwen-max",
                    true,
                ),
                "glm" => (
                    "glm",
                    Some("https://open.bigmodel.cn/api/paas/v4"),
                    "glm-4-flash",
                    false,
                ),
                "yi" => (
                    "yi",
                    Some("https://api.lingyiwanwu.com/v1"),
                    "yi-lightning",
                    false,
                ),
                "hunyuan" => (
                    "hunyuan",
                    Some("https://api.hunyuan.cloud.tencent.com/v1"),
                    "hunyuan-turbo-latest",
                    false,
                ),
                "doubao" => (
                    "doubao",
                    Some("https://ark.cn-beijing.volces.com/api/v3"),
                    "doubao-1.5-pro-256k-250115",
                    true,
                ),
                "facewall" => (
                    "facewall",
                    Some("https://api.facewall.ai/v1"),
                    "facewall-chat",
                    false,
                ),
                "langboat" => (
                    "langboat",
                    Some("https://api.langboat.com/v1"),
                    "langboat-chat",
                    false,
                ),
                "skywork" => (
                    "skywork",
                    Some("https://api.skywork.ai/v1"),
                    "skywork-chat",
                    false,
                ),
                "stepfun" => (
                    "stepfun",
                    Some("https://api.stepfun.com/v1"),
                    "step-2-16k",
                    false,
                ),
                "xihu" => ("xihu", Some("https://api.xihu.ai/v1"), "xihu-chat", false),
                "moonshot" => (
                    "moonshot",
                    Some("https://api.moonshot.cn/v1"),
                    "moonshot-v1-8k",
                    false,
                ),
                "minimax" => (
                    "minimax",
                    Some("https://api.minimax.chat/v1"),
                    "MiniMax-Text-01",
                    false,
                ),
                "siliconflow" => (
                    "openai_compatible",
                    Some("https://api.siliconflow.cn/v1"),
                    "deepseek-ai/DeepSeek-V3.2",
                    true,
                ),
                "ai21" => (
                    "ai21",
                    Some("https://api.ai21.com/studio/v1"),
                    "jamba-1.5-mini",
                    false,
                ),
                "aleph" => (
                    "aleph",
                    Some("https://api.aleph-alpha.com"),
                    "luminous-base",
                    false,
                ),
                "copilot" => ("copilot", Some("http://127.0.0.1:8080"), "", false),
                "deepquest" => (
                    "deepquest",
                    Some("https://api.deepquest.ai/v1"),
                    "deepquest-chat",
                    false,
                ),
                "fireworks" => (
                    "fireworks",
                    Some("https://api.fireworks.ai/inference/v1"),
                    "accounts/fireworks/models/llama-v3p1-8b-instruct",
                    false,
                ),
                "gemini" => (
                    "gemini",
                    Some("https://generativelanguage.googleapis.com/v1beta"),
                    "gemini-2.5-flash",
                    false,
                ),
                "groq" => (
                    "groq",
                    Some("https://api.groq.com/openai/v1"),
                    "llama-3.3-70b-versatile",
                    false,
                ),
                "llama" => ("llama", Some("http://127.0.0.1:11434/v1"), "llama3.2", true),
                "loopai" => (
                    "loopai",
                    Some("https://api.loopai.com/v1"),
                    "loopai-chat",
                    false,
                ),
                "mistral" => (
                    "mistral",
                    Some("https://api.mistral.ai/v1"),
                    "mistral-small-latest",
                    false,
                ),
                "nim" => (
                    "nim",
                    Some("https://integrate.api.nvidia.com/v1"),
                    "meta/llama-3.1-70b-instruct",
                    false,
                ),
                "perplexity" => (
                    "perplexity",
                    Some("https://api.perplexity.ai"),
                    "sonar-pro",
                    false,
                ),
                "replicate" => (
                    "replicate",
                    Some("https://api.replicate.com/v1"),
                    "meta/meta-llama-3-70b-instruct",
                    false,
                ),
                "titan" => (
                    "titan",
                    Some("https://api.titanml.co/v1"),
                    "titan-chat",
                    false,
                ),
                "together" => (
                    "together",
                    Some("https://api.together.xyz/v1"),
                    "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo",
                    false,
                ),
                "xai" => (
                    "openai_compatible",
                    Some("https://api.x.ai/v1"),
                    "grok-3",
                    true,
                ),
                _ => ("openai_compatible", None, "auto", false),
            }
        }

        // Single pass: collect provider TOML blocks (agent names are no longer needed
        // in the config output since phases use empty agent lists for capability-bus routing).
        let (provider_lines, _agent_names): (Vec<String>, Vec<String>) = config
            .providers
            .iter()
            .filter(|p| {
                // Priority: keyring first, then config as fallback
                crate::keyring_util::has_api_key(&p.name.to_lowercase())
                    || (!p.api_key.is_empty() && p.api_key != REDACTED_API_KEY)
            })
            .map(|p| {
                let name = p.name.to_lowercase();
                let (agent_type, default_url, default_model, supports_system) =
                    provider_meta(&name);

                // When a label is set, use it to disambiguate multiple entries of the same provider.
                // The agent name becomes `{name}_{label}` so backend can differentiate them.
                let agent_name = if p.label.is_empty() {
                    name.clone()
                } else {
                    format!("{}_{}", name, p.label.to_lowercase().replace(' ', "_"))
                };

                // Model: user-configured, or type default
                let model = if p.model.is_empty() || p.model == "auto" {
                    default_model
                } else {
                    &p.model
                };

                // URL: openai_compatible always needs an explicit url; built-in agent types
                // (wenxin, qianfan, etc.) hardcode their URLs internally.
                let url_line = if agent_type == "openai_compatible" {
                    match default_url {
                        Some(url) => format!("url = \"{}\"\n", url),
                        None => String::new(),
                    }
                } else if matches!(agent_type, "wenxin" | "qianfan") {
                    String::new()
                } else {
                    match default_url {
                        Some(url) => format!("url = \"{}\"\n", url),
                        None => String::new(),
                    }
                };

                // API key env var reference
                let api_key_env = match name.as_str() {
                    "copilot" => "GITHUB_COPILOT_TOKEN".to_string(),
                    _ => format!("keyring://go-on/{}_api_key", name),
                };

                // Secret key line: wenxin/qianfan dual-auth
                let secret_key_line = match name.as_str() {
                    "wenxin" | "qianfan" => {
                        format!("secret_key_env = \"keyring://go-on/{}_secret_key\"\n", name)
                    }
                    _ => String::new(),
                };

                // Chat path: only doubao needs a non-default path
                let chat_path_line = if name == "doubao" {
                    "chat_path = \"/chat/completions\"\n".to_string()
                } else {
                    String::new()
                };

                // Anthropic-specific fields
                let anthropic_line = if agent_type == "claude" {
                    "anthropic_version = \"2023-06-01\"\nmax_tokens = 8192\n".to_string()
                } else {
                    String::new()
                };

                let supports_system_line = if supports_system {
                    "supports_system = true\n".to_string()
                } else {
                    String::new()
                };

                let toml_block = format!(
                    r#"[agents.{}]
type = "{}"
api_key_env = "{}"
{}{}{}{}{}model = "{}"
"#,
                    agent_name,
                    agent_type,
                    api_key_env,
                    url_line,
                    secret_key_line,
                    chat_path_line,
                    anthropic_line,
                    supports_system_line,
                    model,
                );
                (toml_block, agent_name)
            })
            .unzip();

        if provider_lines.is_empty() && !config.providers.is_empty() {
            eprintln!("WARNING: No providers have valid API keys. Generated config.toml will have no agents.");
        } else if provider_lines.is_empty() {
            eprintln!("INFO: No providers configured. Generated config.toml will be minimal.");
        }

        let agent_section = if provider_lines.is_empty() {
            String::new()
        } else {
            let agents_toml = provider_lines.join("\n");
            let phases_list = "[\"planning\", \"coding\", \"review\", \"delivery\"]";
            format!(
                r#"{agents_toml}

[flow]
name = "go-on-gui"
workflow_type = "dev"
phases = {phases_list}

[phases.planning]
description = "Planning — analyze requirements, design solution"
agents = []
fallback = true

[phases.planning.options]
request_timeout_seconds = 120
review_timeout_seconds = 60
cache_enabled = true
vector_enabled = true
phase_max_inflight = 8
global_max_inflight = 128

[phases.coding]
description = "Coding — implement features, write code"
agents = []
fallback = true

[phases.coding.options]
request_timeout_seconds = 300
review_timeout_seconds = 120
cache_enabled = true
vector_enabled = true
phase_max_inflight = 24
global_max_inflight = 128

[phases.review]
description = "Review — verify, validate, check quality"
agents = []
fallback = true

[phases.review.options]
request_timeout_seconds = 120
review_timeout_policy = "reject"
review_min_response_chars = 12
cache_enabled = true
vector_enabled = true
phase_max_inflight = 16
global_max_inflight = 128

[phases.delivery]
description = "Delivery — finalize, summarize, present results"
agents = []
fallback = false

[phases.delivery.options]
request_timeout_seconds = 90
phase_max_inflight = 8
global_max_inflight = 64
"#
            )
        };

        // Bind address must match GUI's backend_url
        let bind_addr = backend_bind_addr_from_url(&config.backend_url);

        let toml = format!(
            r#"# Auto-generated by go-on-gui — do not edit manually.
# Provider settings are managed from the GUI's Providers/Settings page.

default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "{protocol_mode}"

[cache]
enabled = true
path = "acp_cache.sqlite3"
default_ttl_seconds = 3600
max_entries = 5000

[vector]
enabled = true
auto_mode = true
path = "acp_vector.sqlite3"
dimensions = 192
min_query_chars = 80
top_k = 2
min_similarity = 0.82
max_snippet_chars = 800
max_entries = 10000
summary_enabled = true
summary_trigger_messages = 8
summary_max_chars = 1200

[runtime]
maintenance_interval_seconds = 60
health_interval_seconds = 120
shutdown_drain_seconds = 30
sqlite_vacuum_interval_cycles = 60
skills_import_enabled = true
skills_allowed_sources = ["github.com/*", "raw.githubusercontent.com/*", "https://*"]
skills_require_sha256 = false
skills_allow_floating_ref = true
acp_http_bind_addr = "{bind_addr}"

[autotune]
enabled = false
evaluate_interval = 20
state_path = "acp_autotune_state.json"

{agent_section}"#,
            protocol_mode = config.protocol_mode,
            bind_addr = bind_addr,
            agent_section = agent_section,
        );

        match std::fs::write(path, &toml) {
            Ok(_) => eprintln!("backend: wrote config.toml to {}", path.display()),
            Err(e) => eprintln!("backend: failed to write config.toml: {}", e),
        }

        // Also generate/update zed-config.toml (ZED IDE integration)
        // Uses STDIO mode and the same agent configs.
        let zed_path = path.parent().map(|p| p.join("zed-config.toml"));
        if let Some(ref zed_path) = zed_path {
            // Only overwrite if it's auto-generated (has Auto-generated marker)
            // or doesn't exist yet. Preserve user edits to zed-config.toml.
            let should_overwrite = if let Ok(existing) = std::fs::read_to_string(zed_path) {
                existing.contains("Auto-generated by go-on-gui")
            } else {
                true
            };
            if should_overwrite {
                let zed_toml = format!(
                    r#"# Auto-generated by go-on-gui — do not edit manually.
# ZED IDE integration config (STDIO mode).

[protocol]
mode = "acp_stdio"

[cache]
enabled = true
path = "acp_cache.sqlite3"
default_ttl_seconds = 3600
max_entries = 5000

[vector]
enabled = true
path = "acp_vector.sqlite3"
dimensions = 192
min_query_chars = 80
top_k = 2

{agent_section}"#,
                    agent_section = agent_section,
                );
                match std::fs::write(zed_path, &zed_toml) {
                    Ok(_) => eprintln!("backend: wrote zed-config.toml to {}", zed_path.display()),
                    Err(e) => eprintln!("backend: failed to write zed-config.toml: {}", e),
                }
            }
        }
    }

    /// Kill the current backend child and start a new one with fresh env vars.
    /// Called after adding/updating API keys so the new keys take effect immediately.
    fn restart_backend(&mut self) {
        // Kill old process
        if let Some(mut child) = self.backend_child.take() {
            self.backend_crash_count = self.backend_crash_count.saturating_add(1);
            eprintln!("Restarting backend (old PID: {})...", child.id());
            let _ = child.kill();
            // Wait briefly for the old process to release port 8090 before
            // spawning the new one, preventing EADDRINUSE.
            // Uses thread::sleep which blocks the UI thread briefly (~300ms)
            // but is the simplest reliable approach across all platforms.
            std::thread::sleep(std::time::Duration::from_millis(300));
            // Don't block UI thread waiting for backend to exit.
            // Spawn a background thread to reap the zombie.
            let pid = child.id();
            std::thread::spawn(move || {
                let _ = child.wait();
                eprintln!("go-on backend (PID: {}) fully stopped", pid);
            });
        }
        // Start new
        let (backend, child, reused_external) = Self::spawn_backend(self.config_shared.as_ref());
        self.backend = backend;
        self.backend_child = child;
        self.backend_reused_external = reused_external;
        // Force immediate refresh on next update() cycle
        self.pending_refresh = false;
        self.last_refresh = Instant::now() - std::time::Duration::from_secs(10);
        self.staged_health = None;
        self.staged_providers = None;
        self.staged_refresh_done = false;
        self.health_disconnect_streak = 0;
        // Clear stale health/providers so monitor shows correct state
        self.monitor_view.health = None;
        self.monitor_view.providers = Vec::new();
        // Also reset chat cache so it re-fetches models from new backend
        self.chat_view.reset_loaded_state();
        // Reset providers loaded state so models are re-fetched
        self.providers_view.reset_loaded_state();
        eprintln!("Backend restarted");
    }

    /// Detect the localized window title based on the saved config language.
    /// Called once at startup before the I18n instance is created.
    #[allow(dead_code)]
    pub fn detect_initial_window_title(config: &AppConfig) -> String {
        if config.language == "zh-CN" {
            "Go-On 图形界面".to_string()
        } else if config.language == "zh-TW" {
            "Go-On 圖形界面".to_string()
        } else {
            "Go-On GUI".to_string()
        }
    }

    pub fn new(config: AppConfig) -> Self {
        let config_shared = Arc::new(config.clone());
        let config_shared_fingerprint = Self::config_fingerprint(&config);
        // Auto-detect: if user hasn't explicitly set a language, try system locale
        let lang = if config.language.is_empty() || config.language == "en" {
            detect_system_language()
        } else {
            match config.language.as_str() {
                "zh-CN" => Lang::ZhCn,
                "zh-TW" => Lang::ZhTw,
                _ => Lang::En,
            }
        };
        let providers_valid = has_valid_providers(&config);

        let (backend_tx, backend_updates) = mpsc::sync_channel(128);

        // Start backend with env vars from keyring
        let (backend, backend_child, backend_reused_external) = Self::spawn_backend(&config);

        // Compute hash before moving config
        let initial_url_hash = {
            let mut hasher = DefaultHasher::new();
            config.backend_url.hash(&mut hasher);
            hasher.finish()
        };
        let initial_url = config.backend_url.clone();
        let ui_state = GlobalUiState::load();

        let mut app = Self {
            backend,
            config_shared,
            config_shared_fingerprint,
            i18n: I18n::new(lang),
            setup_view: SetupView::new(),
            monitor_view: MonitorView::new(),
            chat_view: ChatView::new(),
            skills_view: SkillsView::new(),
            workflow_view: WorkflowView::new(),
            autotune_view: AutoTuneView::new(),
            security_view: SecurityView::new(),
            config_editor_view: ConfigEditorView::new(),
            prompts_view: PromptsView::new(),
            risk_decision_view: RiskDecisionView::new(),
            providers_view: ProvidersView::new(),
            about_view: AboutView::new(),
            config,
            show_setup: !providers_valid,
            // Internal tab IDs must stay stable (English keys); labels are localized in UI.
            active_tab: "monitor".to_string(),
            has_providers: providers_valid,
            backend_updates,
            backend_tx,
            pending_refresh: false,
            last_refresh: Instant::now(),
            backend_child,
            backend_reused_external,
            last_applied_theme: String::new(),
            backend_crash_time: None,
            last_backend_url_hash: initial_url_hash,
            backend_url_original: initial_url,
            staged_health: None,
            staged_providers: None,
            staged_refresh_done: false,
            last_backend_ui_commit: Instant::now(),
            health_disconnect_streak: 0,
            backend_crash_count: 0,
            consecutive_poll_failures: 0,
            blocked_tab_toast_shown: None,
            last_prompts_command_version: 0,
            last_prompts_lang: lang,
            ui_state,
        };

        // Pre-load prompts for chat `/` command expansion and category browser,
        // regardless of whether the Prompts tab itself is visible.
        app.prompts_view.ensure_loaded(lang);

        app
    }

    fn apply_health_debounce(&mut self, mut next: HealthStatus) -> HealthStatus {
        let was_connected = self
            .monitor_view
            .health
            .as_ref()
            .is_some_and(|h| h.connected);

        if next.connected {
            self.health_disconnect_streak = 0;
            return next;
        }

        if was_connected {
            self.health_disconnect_streak = self.health_disconnect_streak.saturating_add(1);
            if self.health_disconnect_streak < self.health_disconnect_debounce_count() {
                if let Some(ref prev) = self.monitor_view.health {
                    next = prev.clone();
                }
            }
        }

        next
    }

    fn current_lang(&self) -> Lang {
        match self.config_shared.language.as_str() {
            "zh-CN" => Lang::ZhCn,
            "zh-TW" => Lang::ZhTw,
            _ => Lang::En,
        }
    }

    fn sync_backend_url(&mut self) {
        let config_url = self
            .config_shared
            .backend_url
            .trim_end_matches('/')
            .to_string();
        if self.backend.base_url() != config_url {
            self.backend.set_base_url(&config_url);
        }
    }

    fn poll_backend_updates(&mut self, ctx: &egui::Context) {
        let mut received_any = false;
        let mut processed = 0;
        while let Ok(update) = self.backend_updates.try_recv() {
            received_any = true;
            processed += 1;
            if processed > 128 {
                eprintln!(
                    "poll_backend_updates: discarding {} queued updates (processing limit)",
                    processed
                );
                // Drain remaining to prevent channel growth
                while self.backend_updates.try_recv().is_ok() {}
                break;
            }
            match update {
                BackendUpdate::Health(h) => self.staged_health = Some(h),
                BackendUpdate::Providers(p) => self.staged_providers = Some(p),
                BackendUpdate::RefreshDone => self.staged_refresh_done = true,
            }
        }

        if !received_any {
            return;
        }

        let should_commit = self.staged_refresh_done
            || self.last_backend_ui_commit.elapsed() >= self.backend_ui_commit_debounce();
        if !should_commit {
            return;
        }

        let mut changed = false;

        if let Some(next_health) = self.staged_health.take() {
            let debounced = self.apply_health_debounce(next_health);
            let is_connected = debounced.connected;
            if self.monitor_view.health.as_ref() != Some(&debounced) {
                self.monitor_view.health = Some(debounced);
                changed = true;
            }
            // Track consecutive poll failures for backoff
            if !is_connected {
                self.consecutive_poll_failures = self.consecutive_poll_failures.saturating_add(1);
            } else {
                self.consecutive_poll_failures = 0;
                // Reset crash count on confirmed healthy connection — a health check
                // with connected=true means the backend is running fine, so any prior
                // "crash" was a legitimate restart (provider add, URL change, etc.).
                self.backend_crash_count = 0;
            }
        }

        if let Some(next_providers) = self.staged_providers.take() {
            if self.monitor_view.providers != next_providers {
                self.monitor_view.providers = next_providers;
                changed = true;
            }
        }

        if self.staged_refresh_done {
            self.staged_refresh_done = false;
            if self.pending_refresh {
                self.pending_refresh = false;
                changed = true;
            }
        }

        self.last_backend_ui_commit = Instant::now();

        if changed {
            // Signal egui that new data arrived; the frame cache in update()
            // will debounce and skip rendering if content hasn't materially changed.
            ctx.request_repaint();
        }
    }

    fn maybe_refresh_backend(&mut self) {
        // Progressive backoff: skip polls after consecutive failures
        if self.consecutive_poll_failures > 0 {
            let backoff_secs =
                (2u64).pow(self.consecutive_poll_failures.min(5).saturating_sub(1) as u32); // 1, 2, 4, 8, 16
            let max_backoff = 60u64;
            let effective_backoff = backoff_secs.min(max_backoff);
            if self.last_refresh.elapsed() < std::time::Duration::from_secs(effective_backoff) {
                return;
            }
        }

        if self.last_refresh.elapsed() >= self.backend_refresh_interval() && !self.pending_refresh {
            self.pending_refresh = true;
            let tx = self.backend_tx.clone();
            let backend = self.backend.clone();
            tokio::spawn(async move {
                // Add timeout to prevent hanging if backend is not responding
                let health =
                    match tokio::time::timeout(std::time::Duration::from_secs(5), backend.health())
                        .await
                    {
                        Ok(h) => h,
                        Err(_) => {
                            log_msg("Warning: Backend health check timed out");
                            HealthStatus {
                                connected: false,
                                healthy: false,
                                uptime: 0,
                                requests_per_minute: 0.0,
                                success_rate: 0.0,
                                avg_latency_ms: 0.0,
                                backend_version: None,
                                backend_build: None,
                            }
                        }
                    };
                if let Err(e) = tx.try_send(BackendUpdate::Health(health)) {
                    log_msg(&format!("WARN: app try_send failed: {:?}", e));
                }

                let providers = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend.provider_status(),
                )
                .await
                {
                    Ok(p) => p,
                    Err(_) => {
                        log_msg("Warning: Backend provider status check timed out");
                        vec![]
                    }
                };
                if let Err(e) = tx.try_send(BackendUpdate::Providers(providers)) {
                    log_msg(&format!("WARN: app try_send failed: {:?}", e));
                }
                if let Err(e) = tx.try_send(BackendUpdate::RefreshDone) {
                    log_msg(&format!("WARN: app try_send failed: {:?}", e));
                }
            });
            self.last_refresh = Instant::now();
        }
    }
}

impl eframe::App for GoOnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _frame_start = std::time::Instant::now();

        self.sync_shared_config_if_needed();
        self.i18n.switch(self.current_lang());

        // Keep prompts data available for chat command/category search even when
        // the Prompts tab is hidden. Also reload on language changes.
        let cur_lang = self.current_lang();
        if !self.prompts_view.loaded {
            self.prompts_view.ensure_loaded(cur_lang);
        } else if self.last_prompts_lang != cur_lang {
            self.prompts_view.reload(cur_lang);
        }
        self.last_prompts_lang = cur_lang;

        // Apply prompt insertion globally so inserts from the Prompts tab are
        // immediately reflected in Chat input and user is routed to Chat.
        if let Some(content) = self.prompts_view.pending_insert.take() {
            if self.chat_view.input.trim().is_empty() {
                self.chat_view.input = content;
            } else {
                self.chat_view.input = format!("{}\n\n{}", self.chat_view.input, content);
            }
            self.active_tab = "chat".to_string();
        }

        // Sync prompts-derived command templates/category collection into Chat.
        if self.last_prompts_command_version != self.prompts_view.command_version {
            self.last_prompts_command_version = self.prompts_view.command_version;
            self.chat_view.prompts_command_templates = self.prompts_view.command_templates.clone();
            self.chat_view.prompt_collection = self.prompts_view.collection.clone();
        }

        // ── Frame rate governor ───────────────────────────────────────────
        // Only apply theme on actual change — ctx.set_style() invalidates
        // egui's text cache and forces full re-layout.
        if self.last_applied_theme != self.config_shared.theme {
            self.last_applied_theme = self.config_shared.theme.clone();
            let theme = crate::theme::Theme::from_name(&self.config_shared.theme);
            theme.apply(ctx);
        }

        // ── Minimal repaint governor ────────────────────────────────
        // DO NOT force continuous repaints — they cause screen flickering
        // on every frame. Each async subsystem (health polling, chat
        // streaming, provider status) already calls ctx.request_repaint()
        // when it has new data. User interactions (mouse, keyboard) are
        // handled by egui's event system automatically.
        //
        // Only schedule a very infrequent wake-up (2s) to keep the GL
        // Minimal wake-up to keep the window system responsive.
        // Increased to 10s to reduce idle repaint frequency (eliminates
        // the micro-jitter caused by periodic widget tree rebuilds).
        if !self.pending_refresh && !self.chat_view.sending {
            ctx.request_repaint_after(std::time::Duration::from_secs(10));
        }

        // Reap zombie child if backend exited
        if let Some(ref mut child) = self.backend_child {
            match child.try_wait() {
                Ok(None) => {} // still running
                Ok(Some(status)) => {
                    eprintln!("go-on backend exited (code: {:?})", status.code());
                    self.backend_child = None;
                    if backend_log_has_addr_in_use() {
                        // Another process is already bound to backend_url; restarting this child
                        // will just thrash and cause visible UI jitter.
                        eprintln!(
                            "Backend exited due to address-in-use; suppressing auto-restart storm"
                        );
                        self.backend_reused_external = true;
                        self.backend_crash_count = 10;
                        self.backend_crash_time = None;
                    } else {
                        self.backend_crash_time = Some(Instant::now());
                    }
                }
                Err(e) => {
                    eprintln!("go-on backend wait error: {}", e);
                    self.backend_child = None;
                    self.backend_crash_time = Some(Instant::now());
                }
            }
        }

        // Setup screen
        if self.show_setup {
            let done = self
                .setup_view
                .show(ctx, &self.i18n, &mut self.config, &self.backend);
            if done {
                self.show_setup = false;
                self.has_providers = has_valid_providers(&self.config);
                save_app_config(&self.config);
                self.sync_shared_config_if_needed();
                // Restart backend so it picks up the new API key from env
                self.restart_backend();
            }
            // Only request repaint if the setup view has something pending.
            // The setup_view.show() method already calls ctx.request_repaint() on user interaction,
            // so we don't need a per-frame repaint here.
            return;
        }

        self.sync_backend_url();

        // Auto-restart backend if it crashed and enough time has passed
        if self.backend_child.is_none() && !self.show_setup {
            if let Some(crash_time) = self.backend_crash_time {
                let backoff_secs = 3u64 * (1u64 << self.backend_crash_count.min(5)); // 3, 6, 12, 24, 48, 96
                if crash_time.elapsed() >= Duration::from_secs(backoff_secs) {
                    if self.backend_crash_count >= 10 {
                        eprintln!(
                            "Backend crashed {} times; giving up auto-restart",
                            self.backend_crash_count
                        );
                        self.backend_crash_time = None;
                    } else {
                        self.backend_crash_time = None;
                        eprintln!(
                            "Auto-restarting backend after crash (count={})...",
                            self.backend_crash_count
                        );
                        self.restart_backend();
                    }
                }
            }
        }

        // Detect backend URL changes and reset chat cache
        let current_hash = {
            let mut hasher = DefaultHasher::new();
            self.config_shared.backend_url.hash(&mut hasher);
            hasher.finish()
        };
        if current_hash != self.last_backend_url_hash {
            self.last_backend_url_hash = current_hash;
            self.chat_view.reset_loaded_state();
        }

        self.poll_backend_updates(ctx);
        self.maybe_refresh_backend();
        self.has_providers = has_valid_providers(self.config_shared.as_ref());

        let tabs = self.active_tabs_precomputed();
        if !tabs.contains(&self.active_tab) {
            self.active_tab = if tabs.iter().any(|t| t == "monitor") {
                "monitor".to_string()
            } else {
                "settings".to_string()
            };
        }
        let is_connected = self
            .monitor_view
            .health
            .as_ref()
            .is_some_and(|h| h.connected);

        // Toolbar
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            egui::Frame::NONE.show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let title_color = ui.style().visuals.text_color();
                    ui.label(
                        egui::RichText::new(self.i18n.t("app.title"))
                            .text_style(egui::TextStyle::Heading)
                            .strong()
                            .color(title_color),
                    );
                    // Keyboard shortcut hints
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new(self.i18n.t("app.shortcutHint"))
                            .size(11.0)
                            .weak(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let status = if is_connected {
                            self.i18n.t("status.connected")
                        } else {
                            self.i18n.t("status.disconnected")
                        };
                        let status_color = if is_connected {
                            egui::Color32::from_rgb(60, 180, 80)
                        } else {
                            egui::Color32::from_rgb(220, 80, 80)
                        };
                        let pid_info = self
                            .backend_child
                            .as_ref()
                            .map(|c| format!("  PID:{}", c.id()))
                            .unwrap_or_default();

                        egui::Frame::new()
                            .fill(status_color.gamma_multiply(0.15))
                            .corner_radius(12.0)
                            .inner_margin(egui::Margin::symmetric(10i8, 4i8))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(format!("{}{}", status, pid_info))
                                        .color(status_color)
                                        .strong(),
                                );
                            });
                    });

                    // Reserve a fixed spinner slot to avoid toolbar width shifts.
                    ui.allocate_ui_with_layout(
                        egui::vec2(20.0, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if self.pending_refresh {
                                ui.add(egui::Label::new(
                                    egui::RichText::new("⟳")
                                        .color(egui::Color32::from_rgb(100, 180, 255))
                                        .size(16.0),
                                ));
                            }
                        },
                    );
                });
            });
        });

        // Global keyboard shortcuts for tab switching
        let mut tab_shortcut: Option<String> = None;
        ctx.input_mut(|i| {
            // Command (macOS) / Ctrl (Windows, Linux) + number for tab switching.
            let tab_keys: [(egui::Key, usize); 10] = [
                (egui::Key::Num1, 0),
                (egui::Key::Num2, 1),
                (egui::Key::Num3, 2),
                (egui::Key::Num4, 3),
                (egui::Key::Num5, 4),
                (egui::Key::Num6, 5),
                (egui::Key::Num7, 6),
                (egui::Key::Num8, 7),
                (egui::Key::Num9, 8),
                (egui::Key::Num0, 9),
            ];
            for (key, idx) in tab_keys {
                let triggered = i.consume_key(egui::Modifiers::CTRL, key)
                    || i.consume_key(egui::Modifiers::COMMAND, key);
                if triggered && idx < tabs.len() {
                    tab_shortcut = Some(tabs[idx].clone());
                }
            }
            // Command+, (macOS) / Ctrl+, (Windows, Linux) for settings
            if (i.consume_key(egui::Modifiers::CTRL, egui::Key::Comma)
                || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma))
                && tabs.iter().any(|t| t == "settings")
            {
                tab_shortcut = Some("settings".to_string());
            }
        });

        // Tab bar — when disconnected, only monitor/providers/settings are accessible
        let allowed_when_offline = [
            "monitor",
            "providers",
            "prompts",
            "risk_decision",
            "settings",
        ];
        let mut new_tab: Option<String> = None;
        let mut blocked_tab: Option<String> = None;
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            egui::Frame::NONE.show(ui, |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        for tab in &tabs {
                            let label = self.tab_label(tab);
                            let is_active = self.active_tab == *tab;
                            let blocked =
                                !is_connected && !allowed_when_offline.contains(&tab.as_str());
                            let resp = ui
                                .add_enabled_ui(!blocked, |ui| {
                                    ui.selectable_label(is_active, label)
                                })
                                .inner;
                            if resp.clicked() {
                                if blocked {
                                    blocked_tab = Some(tab.clone());
                                } else {
                                    new_tab = Some(tab.clone());
                                }
                            }
                        }
                    });
                });
            });
        });
        // Save old tab's UI state before switching tabs
        let previous_tab = self.active_tab.clone();
        if let Some(t) = new_tab {
            self.save_tab_ui_state(&previous_tab);
            self.active_tab = t;
            let tab = self.active_tab.clone();
            self.restore_tab_ui_state(&tab);
        }
        if let Some(t) = tab_shortcut {
            self.save_tab_ui_state(&previous_tab);
            // Apply offline guard for keyboard shortcuts too
            if is_connected || allowed_when_offline.contains(&t.as_str()) {
                self.active_tab = t;
                let tab = self.active_tab.clone();
                self.restore_tab_ui_state(&tab);
            }
        }

        // Show toast when a blocked tab is clicked
        if blocked_tab.is_some() {
            self.blocked_tab_toast_shown = Some(Instant::now());
        }
        // Auto-dismiss toast after 5 seconds
        let toast_visible = self
            .blocked_tab_toast_shown
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        if toast_visible {
            egui::Window::new("⚠")
                .id(egui::Id::new("blocked_tab_toast"))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0])
                .collapsible(false)
                .resizable(false)
                .auto_sized()
                .show(ctx, |ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 160, 50),
                        self.i18n.t("app.backendRequired"),
                    );
                    ui.label(
                        egui::RichText::new(self.i18n.t("app.backendRequiredHint"))
                            .size(13.0)
                            .weak(),
                    );
                    ui.add_space(4.0);
                    if ui.button(self.i18n.t("common.close")).clicked() {
                        self.blocked_tab_toast_shown = None;
                    }
                });
        }

        // ── Main content ────────────────────────────────────────────────
        // Rendered every frame that passes the incremental renderer gate.
        // The WGPU back-buffer retains the previous frame when no changes
        // are needed, so there is no flicker from empty frames.
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::NONE.show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("main_scroll")
                    .show(ui, |ui| {
                        let has_backend = self.has_providers;
                        match self.active_tab.as_str() {
                            "monitor" => {
                                let mon_alerts = self.config.features.monitor_history_alerts;
                                self.monitor_view.show(
                                    ui,
                                    &self.i18n,
                                    has_backend,
                                    &self.backend,
                                    mon_alerts,
                                    self.backend_reused_external,
                                );
                            }
                            "chat" => {
                                let stability = &self.config.ui_stability;
                                let autotune_chain = self.config.features.autotune_chain_injection;
                                self.chat_view.show(
                                    ui,
                                    &self.i18n,
                                    &self.backend,
                                    ctx,
                                    autotune_chain,
                                    ChatUiRuntimeConfig {
                                        repaint_interval_ms: stability.chat_repaint_interval_ms,
                                        stream_chunk_flush_ms: stability.chat_stream_chunk_flush_ms,
                                        max_pending_events_per_frame: stability
                                            .chat_max_pending_events_per_frame,
                                    },
                                );
                                // Keep standalone Risk Decision tab in sync with the latest
                                // fields edited in Chat's risk popup.
                                let draft = self.chat_view.risk_decision_draft();
                                self.risk_decision_view.apply_draft(&draft);
                            }
                            "skills" => {
                                let skills_lifecycle = self.config.features.skills_lifecycle;
                                self.skills_view.show(
                                    ui,
                                    &self.i18n,
                                    &self.backend,
                                    ctx,
                                    skills_lifecycle,
                                );
                                if self.ui_state.skills_show_create != self.skills_view.show_create
                                    || self.ui_state.skills_show_import
                                        != self.skills_view.show_import
                                {
                                    self.ui_state.skills_show_create = self.skills_view.show_create;
                                    self.ui_state.skills_show_import = self.skills_view.show_import;
                                    self.ui_state.save();
                                }
                            }
                            "settings" => {
                                SettingsView::show(ui, &self.i18n, &mut self.config);
                                if self.config.backend_url != self.backend_url_original {
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(4.0);
                                    if ui
                                        .button("🔄 ".to_string() + &self.i18n.t("app.restart"))
                                        .clicked()
                                    {
                                        self.backend_url_original = self.config.backend_url.clone();
                                        self.restart_backend();
                                    }
                                    ui.label(
                                        egui::RichText::new(self.i18n.t("settings.backendUrlHint"))
                                            .weak(),
                                    );
                                }
                            }
                            "workflow" => {
                                let workflow_run_center = self.config.features.workflow_run_center;
                                self.workflow_view.show(
                                    ui,
                                    &self.i18n,
                                    ctx,
                                    &self.backend,
                                    workflow_run_center,
                                );
                                if self.ui_state.workflow_run_status_filter
                                    != self.workflow_view.run_status_filter
                                    || self.ui_state.workflow_selected_run_id
                                        != self.workflow_view.selected_run_id
                                {
                                    self.ui_state.workflow_run_status_filter =
                                        self.workflow_view.run_status_filter.clone();
                                    self.ui_state.workflow_selected_run_id =
                                        self.workflow_view.selected_run_id.clone();
                                    self.ui_state.save();
                                }
                            }
                            "prompts" => {
                                self.prompts_view.show(ui, &self.i18n);
                            }
                            "risk_decision" => {
                                let draft = self.chat_view.risk_decision_draft();
                                self.risk_decision_view.apply_draft(&draft);
                                if let Some(block) = self.risk_decision_view.show(ui, &self.i18n) {
                                    if self.chat_view.input.trim().is_empty() {
                                        self.chat_view.input = block;
                                    } else {
                                        self.chat_view.input =
                                            format!("{}\n\n{}", self.chat_view.input, block);
                                    }
                                    self.active_tab = "chat".to_string();
                                }
                                // Push tab edits back into Chat popup state.
                                let draft = self.risk_decision_view.draft();
                                self.chat_view.apply_risk_decision_draft(&draft);
                            }
                            "autotune" => self.autotune_view.show(ui, &self.i18n),
                            "security" => {
                                self.security_view.show(ui, &self.i18n, &self.backend, ctx)
                            }
                            "config" => {
                                let config_safe_mode = self.config.features.config_safe_mode;
                                self.config_editor_view.show(
                                    ui,
                                    &self.i18n,
                                    &mut self.config,
                                    config_safe_mode,
                                );
                                if self.config_editor_view.applied {
                                    self.config_editor_view.applied = false;
                                    self.chat_view.reset_loaded_state();
                                    self.restart_backend();
                                }
                            }
                            "providers" => {
                                let providers_ops_enabled = self.config.features.providers_ops;
                                let changed = self.providers_view.show(
                                    ui,
                                    &self.i18n,
                                    &mut self.config,
                                    &self.backend,
                                    ctx,
                                    providers_ops_enabled,
                                );
                                if changed {
                                    save_app_config(&self.config);
                                    // Sync shared config BEFORE restart so spawn_backend
                                    // uses the updated provider list, not the stale snapshot.
                                    self.sync_shared_config_if_needed();
                                    self.restart_backend();
                                }
                            }
                            "about" => {
                                self.about_view.show(
                                    ui,
                                    &self.i18n,
                                    self.monitor_view.health.as_ref(),
                                    self.backend_child.as_ref().map(std::process::Child::id),
                                );
                            }
                            _ => {
                                ui.heading(&self.active_tab);
                                ui.label(self.i18n.t("app.unknownTab"));
                            }
                        }
                    });
            });
        });

        // Frame timing diagnostics — rate limited to at most once per second
        let frame_elapsed = _frame_start.elapsed();
        if frame_elapsed.as_millis() > 50 {
            use std::sync::atomic::{AtomicU64, Ordering};
            static LAST_FRAME_LOG: AtomicU64 = AtomicU64::new(0);
            let last = LAST_FRAME_LOG.load(Ordering::Relaxed);
            let current = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if current != last {
                LAST_FRAME_LOG.store(current, Ordering::Relaxed);
                log_msg(&format!(
                    "FRAME_DIAG: [{}] took {}ms",
                    self.active_tab,
                    frame_elapsed.as_millis()
                ));
            }
            // TODO: All EGUI widgets now use double-buffering + partial redraw via view-level caching.
        }
    }
}

impl GoOnApp {
    /// Save the given tab's transient UI state into `self.ui_state`.
    fn save_tab_ui_state(&mut self, tab: &str) {
        match tab {
            "chat" => {
                self.chat_view.save_ui_state(&mut self.ui_state);
            }
            "monitor" => {
                self.ui_state.monitor_metrics_window = self.monitor_view.metrics_window.clone();
                self.ui_state.monitor_auto_refresh_interval =
                    self.monitor_view.auto_refresh_interval;
                self.ui_state.monitor_provider_filter = self.monitor_view.provider_filter.clone();
            }
            "providers" => {
                self.ui_state.providers_selected_provider =
                    self.providers_view.selected_provider.clone();
                self.ui_state.providers_new_model = self.providers_view.new_model.clone();
                self.ui_state.providers_new_label = self.providers_view.new_label.clone();
            }
            "skills" => {
                self.ui_state.skills_show_create = self.skills_view.show_create;
                self.ui_state.skills_show_import = self.skills_view.show_import;
                self.ui_state.skills_selected_skill_name =
                    self.skills_view.selected_skill_name.clone();
                self.ui_state.skills_edit_desc = self.skills_view.edit_desc.clone();
                self.ui_state.skills_edit_prompt = self.skills_view.edit_prompt.clone();
                self.ui_state.skills_edit_schema = self.skills_view.edit_schema.clone();
                self.ui_state.skills_test_input = self.skills_view.test_input.clone();
                self.ui_state.skills_rollback_version = self.skills_view.rollback_version.clone();
                self.ui_state.skills_create_name = self.skills_view.create_name.clone();
                self.ui_state.skills_create_desc = self.skills_view.create_desc.clone();
                self.ui_state.skills_create_prompt = self.skills_view.create_prompt.clone();
                self.ui_state.skills_create_schema = self.skills_view.create_input_schema.clone();
                self.ui_state.skills_import_url = self.skills_view.import_url.clone();
            }
            "workflow" => {
                self.ui_state.workflow_run_status_filter =
                    self.workflow_view.run_status_filter.clone();
                self.ui_state.workflow_selected_run_id = self.workflow_view.selected_run_id.clone();
                self.ui_state.workflow_new_name = self.workflow_view.new_name.clone();
                self.ui_state.workflow_new_command = self.workflow_view.new_command.clone();
            }
            "config" => {
                self.ui_state.config_editor_draft = self.config_editor_view.draft.clone();
                self.ui_state.config_editor_search = self.config_editor_view.search_query.clone();
                self.ui_state.config_editor_snapshots = self.config_editor_view.snapshots.clone();
            }
            _ => {}
        }
    }

    /// Restore the given tab's transient UI state from `self.ui_state`.
    fn restore_tab_ui_state(&mut self, tab_name: &str) {
        match tab_name {
            "chat" => {
                // Only restore mode if saved value is valid — otherwise keep existing default
                let valid_modes = ["ask", "plan", "edit", "safeguard", "full_auto"];
                if !self.ui_state.selected_mode.is_empty()
                    && valid_modes.contains(&self.ui_state.selected_mode.as_str())
                {
                    self.chat_view.selected_mode = self.ui_state.selected_mode.clone();
                }
                self.chat_view.show_token_details = self.ui_state.show_token_details;
                self.chat_view.enable_markdown = self.ui_state.enable_markdown;
                self.chat_view.show_model_picker = self.ui_state.show_model_picker;
                self.chat_view.show_prompts = self.ui_state.show_prompts;
                if let Some(json) = &self.ui_state.model_stats_json {
                    if let Ok(stats) = serde_json::from_str(json) {
                        self.chat_view.model_stats = stats;
                    }
                }
                if self.ui_state.active_session < self.chat_view.sessions.len() {
                    self.chat_view.active_session = self.ui_state.active_session;
                }
                self.chat_view.input = self.ui_state.input_draft.clone();
                self.chat_view.session_search_query = self.ui_state.session_search_query.clone();
                self.chat_view.template_search_query = self.ui_state.template_search_query.clone();
            }
            "monitor" => {
                self.monitor_view.metrics_window = self.ui_state.monitor_metrics_window.clone();
                if self.ui_state.monitor_auto_refresh_interval > 0 {
                    self.monitor_view.auto_refresh_interval =
                        self.ui_state.monitor_auto_refresh_interval;
                }
                self.monitor_view.provider_filter = self.ui_state.monitor_provider_filter.clone();
            }
            "providers" => {
                self.providers_view.selected_provider =
                    self.ui_state.providers_selected_provider.clone();
                if !self.ui_state.providers_new_model.is_empty() {
                    self.providers_view.new_model = self.ui_state.providers_new_model.clone();
                }
                self.providers_view.new_label = self.ui_state.providers_new_label.clone();
            }
            "skills" => {
                self.skills_view.show_create = self.ui_state.skills_show_create;
                self.skills_view.show_import = self.ui_state.skills_show_import;
                if !self.ui_state.skills_selected_skill_name.is_empty() {
                    self.skills_view
                        .load_skill_editor_by_name(&self.ui_state.skills_selected_skill_name);
                }
                self.skills_view.edit_desc = self.ui_state.skills_edit_desc.clone();
                self.skills_view.edit_prompt = self.ui_state.skills_edit_prompt.clone();
                self.skills_view.edit_schema = self.ui_state.skills_edit_schema.clone();
                self.skills_view.test_input = self.ui_state.skills_test_input.clone();
                self.skills_view.rollback_version = self.ui_state.skills_rollback_version.clone();
                self.skills_view.create_name = self.ui_state.skills_create_name.clone();
                self.skills_view.create_desc = self.ui_state.skills_create_desc.clone();
                self.skills_view.create_prompt = self.ui_state.skills_create_prompt.clone();
                self.skills_view.create_input_schema = self.ui_state.skills_create_schema.clone();
                self.skills_view.import_url = self.ui_state.skills_import_url.clone();
            }
            "workflow" => {
                self.workflow_view.run_status_filter =
                    self.ui_state.workflow_run_status_filter.clone();
                self.workflow_view.selected_run_id = self.ui_state.workflow_selected_run_id.clone();
                self.workflow_view.new_name = self.ui_state.workflow_new_name.clone();
                self.workflow_view.new_command = self.ui_state.workflow_new_command.clone();
            }
            "config" => {
                self.config_editor_view.draft = self.ui_state.config_editor_draft.clone();
                self.config_editor_view.search_query = self.ui_state.config_editor_search.clone();
                self.config_editor_view.snapshots = self.ui_state.config_editor_snapshots.clone();
            }
            _ => {}
        }
    }
}

impl Drop for GoOnApp {
    fn drop(&mut self) {
        // Abort any in-flight chat generation tasks.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.chat_view.stop_sending();
        }));

        if let Some(mut child) = self.backend_child.take() {
            eprintln!("Shutting down go-on backend (PID: {})...", child.id());
            // Try graceful shutdown first (SIGTERM on Unix)
            let _ = child.kill();
            // Wait up to 3 seconds for clean exit, then force kill.
            let pid = child.id();
            for _ in 0..30 {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        eprintln!("Backend process {} exited cleanly.", pid);
                        return;
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            // Force kill if still running
            eprintln!("Backend {} did not exit gracefully, force killing...", pid);
            let _ = child.kill();
            let _ = child.wait();
        }
        self.backend_crash_count = 0;
    }
}

impl GoOnApp {
    fn active_tabs_precomputed(&self) -> Vec<String> {
        let mut tabs = Vec::new();
        if self.config_shared.features.monitor {
            tabs.push("monitor".into());
        }
        if self.config_shared.features.chat {
            tabs.push("chat".into());
        }
        if self.config_shared.features.skills {
            tabs.push("skills".into());
        }
        if self.config_shared.features.workflow {
            tabs.push("workflow".into());
        }
        if self.config_shared.features.autotune {
            tabs.push("autotune".into());
        }
        if self.config_shared.features.show_prompts_tab {
            tabs.push("prompts".into());
        }
        if self.config_shared.features.chat && self.config_shared.features.show_risk_decision_tab {
            tabs.push("risk_decision".into());
        }
        if self.config_shared.features.security {
            tabs.push("security".into());
        }
        if self.config_shared.features.config {
            tabs.push("config".into());
        }
        if self.config_shared.features.providers {
            tabs.push("providers".into());
        }
        tabs.push("about".into());
        tabs.push("settings".into());
        tabs
    }

    fn tab_label(&self, tab: &str) -> String {
        match tab {
            "monitor" => self.i18n.t("tab.monitor"),
            "chat" => self.i18n.t("tab.chat"),
            "skills" => self.i18n.t("tab.skills"),
            "workflow" => self.i18n.t("tab.workflow"),
            "autotune" => self.i18n.t("tab.autotune"),
            "prompts" => self.i18n.t("tab.prompts"),
            "risk_decision" => self.i18n.t("tab.riskDecision"),
            "security" => self.i18n.t("tab.security"),
            "config" => self.i18n.t("tab.config"),
            "providers" => self.i18n.t("tab.providers"),
            "about" => self.i18n.t("tab.about"),
            "settings" => self.i18n.t("tab.settings"),
            _ => std::borrow::Cow::Borrowed(tab),
        }
        .to_string()
    }
}
