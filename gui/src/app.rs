use crate::backend::BackendClient;
use crate::config::{has_valid_providers, save_app_config, AppConfig};

/// Write a line to go-on-gui.log in the temp directory.
/// Only active in debug builds to avoid blocking the UI thread.
#[allow(dead_code)]
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
use crate::views::chat::ChatUiRuntimeConfig;
use crate::views::{
    about::AboutView, autotune::AutoTuneView, chat::ChatView, config_editor::ConfigEditorView,
    monitor::MonitorView, providers::ProvidersView, security::SecurityView, settings::SettingsView,
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
    pub providers_view: ProvidersView,
    pub about_view: AboutView,
    pub show_setup: bool,
    pub active_tab: String,
    pub has_providers: bool,
    backend_updates: mpsc::Receiver<BackendUpdate>,
    backend_tx: mpsc::Sender<BackendUpdate>,
    pending_refresh: bool,
    last_refresh: Instant,
    /// Managed backend child process
    backend_child: Option<std::process::Child>,
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
    /// Count of consecutive backend crashes for rate limiting
    backend_crash_count: u8,
    /// Consecutive backend health poll failures for progressive backoff
    consecutive_poll_failures: u8,
}

/// Detect system locale from environment variables.
/// Checks LC_ALL, LC_MESSAGES, LANG on Unix; GetUserDefaultLocaleName on Windows.
fn detect_system_language() -> Lang {
    // Check env vars (cross-platform: Linux, macOS, Windows)
    for var in &["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(val) = std::env::var_os(var) {
            let s = val.to_string_lossy().to_lowercase();
            if s.contains("zh_cn")
                || s.contains("zh-cn")
                || s.contains("zh_cn.")
                || s.contains("chinese")
            {
                return Lang::ZhCn;
            }
            if s.contains("zh_tw")
                || s.contains("zh-tw")
                || s.contains("zh_tw.")
                || s.contains("taiwan")
            {
                return Lang::ZhTw;
            }
        }
    }
    // Fallback: try common locale env vars set by desktop environments
    for var in &["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(val) = std::env::var_os(var) {
            let s = val.to_string_lossy().to_lowercase();
            if s.contains("zh") {
                if s.contains("hant") || s.contains("tw") || s.contains("hk") {
                    return Lang::ZhTw;
                }
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
    fn diagnostic_key_report(config: &AppConfig) {
        eprintln!("=== KEY DIAGNOSTIC ===");
        // Collect all provider names: from config.providers AND the canonical PROVIDER_NAMES list.
        let mut all_names: Vec<String> = config
            .providers
            .iter()
            .map(|p| p.name.to_lowercase())
            .collect();
        for name in crate::views::providers::PROVIDER_NAMES {
            let lower = name.to_lowercase();
            if !all_names.contains(&lower) {
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

    /// Start or restart the backend child process with fresh env vars from keyring.
    fn spawn_backend(config: &AppConfig) -> (BackendClient, Option<std::process::Child>) {
        Self::diagnostic_key_report(config);
        match find_backend_binary() {
            Some(path) => {
                let config_dir = path.parent().unwrap_or_else(|| {
                    // When backend binary has no parent path (e.g. "/go-on"),
                    // use the user's home directory as a writable fallback.
                    let home = std::env::var("HOME")
                        .or_else(|_| std::env::var("USERPROFILE"))
                        .unwrap_or_else(|_| ".".to_string());
                    // Leak the PathBuf to get a &'static Path (this is a spawned process)
                    Box::leak(Box::new(std::path::PathBuf::from(home))).as_path()
                });
                let mut cmd = std::process::Command::new(&path);
                cmd.current_dir(config_dir)
                    .arg("--protocol-mode")
                    .arg("acp_http")
                    .stdout(std::process::Stdio::null());

                // Inject API keys for ALL configured providers into backend process environment.
                // Priority: keyring > config file > inherited env.
                for provider in &config.providers {
                    let provider_lower = provider.name.to_lowercase();
                    let derived_var = format!("{}_API_KEY", provider_lower.to_uppercase());
                    let env_var = match provider_lower.as_str() {
                        "copilot" => "GITHUB_COPILOT_TOKEN",
                        "replicate" => "REPLICATE_API_TOKEN",
                        _ => &derived_var,
                    };

                    // Try keyring first
                    let mut key = crate::keyring_util::get_api_key(&provider_lower);

                    // Log when keyring fails (useful for debugging macOS keychain issues)
                    if key.is_none() {
                        eprintln!(
                            "backend: keyring returned no key for '{}', falling back to config",
                            provider_lower
                        );
                    }

                    // Fallback: config file api_key (only clone if needed)
                    if key.is_none()
                        && !provider.api_key.is_empty()
                        && provider.api_key != "********"
                    {
                        key = Some(provider.api_key.clone());
                    }

                    // Fallback: inherited env var
                    if key.is_none() {
                        key = std::env::var(env_var).ok().filter(|v| !v.is_empty());
                    }

                    if let Some(k) = key {
                        let preview = if k.len() > 4 {
                            format!("{}...", &k[..4])
                        } else {
                            "[short]".to_string()
                        };
                        eprintln!("backend: set {}={}", env_var, preview);
                        cmd.env(env_var, k);
                    } else {
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
                        (BackendClient::new(&config.backend_url), Some(child))
                    }
                    Err(e) => {
                        eprintln!("warning: failed to start backend: {}", e);
                        (BackendClient::new(&config.backend_url), None)
                    }
                }
            }
            None => {
                eprintln!("warning: go-on backend binary not found");
                (BackendClient::new(&config.backend_url), None)
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
        let provider_lines: Vec<String> = config
            .providers
            .iter()
            .filter(|p| {
                // Priority: keyring first, then config as fallback
                crate::keyring_util::has_api_key(&p.name.to_lowercase())
                    || (!p.api_key.is_empty() && p.api_key != "********")
            })
            .map(|p| {
                let name = p.name.to_lowercase();
                let model = if p.model.is_empty() || p.model == "auto" {
                    match name.as_str() {
                        "deepseek" => "deepseek-chat",
                        "openai" => "gpt-4o",
                        "anthropic" => "claude-sonnet-4-20250514",
                        "gemini" => "gemini-2.5-flash-preview-04-17",
                        "qwen" => "qwen-max-2025-01-25",
                        _ => "auto",
                    }
                } else {
                    &p.model
                };
                let keyring_ref = format!("keyring://go-on/{}_api_key", name);
                format!(
                    r#"[agents.{}]
type = "{}"
api_key_env = "{}"
model = "{}"
supports_system = true
"#,
                    name, name, keyring_ref, model
                )
            })
            .collect();

        let agent_section = if provider_lines.is_empty() {
            String::new()
        } else {
            let agents_toml = provider_lines.join("\n");
            let agent_names: Vec<String> = config
                .providers
                .iter()
                .filter(|p| {
                    // Priority: keyring first, then config as fallback
                    crate::keyring_util::has_api_key(&p.name.to_lowercase())
                        || (!p.api_key.is_empty() && p.api_key != "********")
                })
                .map(|p| p.name.to_lowercase())
                .collect();
            let agents_list = if agent_names.is_empty() {
                "[]".to_string()
            } else {
                let quoted: Vec<String> =
                    agent_names.iter().map(|n| format!("\"{}\"", n)).collect();
                format!("[{}]", quoted.join(", "))
            };
            format!(
                r#"{agents_toml}

[flow]
name = "go-on-gui"
phases = ["coding"]

[phases.coding]
description = "Coding phase with configured providers"
agents = {agents_list}
fallback = true

[phases.coding.options]
request_timeout_seconds = 300
review_timeout_seconds = 60
cache_enabled = true
vector_enabled = true
phase_max_inflight = 24
global_max_inflight = 128
"#
            )
        };

        // Bind address must match GUI's backend_url
        let bind_addr = config
            .backend_url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/');

        let toml = format!(
            r#"# Auto-generated by go-on-gui — do not edit manually.
# Provider settings are managed from the GUI's Providers/Settings page.

default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "acp_http"

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
acp_http_bind_addr = "{bind_addr}"

[autotune]
enabled = false
evaluate_interval = 20
state_path = "acp_autotune_state.json"

{agent_section}"#
        );

        match std::fs::write(path, &toml) {
            Ok(_) => eprintln!("backend: wrote config.toml to {}", path.display()),
            Err(e) => eprintln!("backend: failed to write config.toml: {}", e),
        }
    }

    /// Kill the current backend child and start a new one with fresh env vars.
    /// Called after adding/updating API keys so the new keys take effect immediately.
    fn restart_backend(&mut self) {
        self.backend_crash_count = self.backend_crash_count.saturating_add(1);
        // Kill old process
        if let Some(mut child) = self.backend_child.take() {
            eprintln!("Restarting backend (old PID: {})...", child.id());
            let _ = child.kill();
            // Don't block UI thread waiting for backend to exit.
            // Spawn a background thread to reap the zombie.
            let pid = child.id();
            std::thread::spawn(move || {
                let _ = child.wait();
                eprintln!("go-on backend (PID: {}) fully stopped", pid);
            });
        }
        // Start new
        let (backend, child) = Self::spawn_backend(self.config_shared.as_ref());
        self.backend = backend;
        self.backend_child = child;
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

        let (backend_tx, backend_updates) = mpsc::channel();

        // Start backend with env vars from keyring
        let (backend, backend_child) = Self::spawn_backend(&config);

        // Compute hash before moving config
        let initial_url_hash = {
            let mut hasher = DefaultHasher::new();
            config.backend_url.hash(&mut hasher);
            hasher.finish()
        };
        let initial_url = config.backend_url.clone();

        Self {
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
        }
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
                let _ = tx.send(BackendUpdate::Health(health));

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
                let _ = tx.send(BackendUpdate::Providers(providers));
                let _ = tx.send(BackendUpdate::RefreshDone);
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
        // Only apply theme when it actually changes — calling ctx.set_style() every
        // frame invalidates egui's text cache and forces a full re-layout each frame.
        if self.last_applied_theme != self.config_shared.theme {
            self.last_applied_theme = self.config_shared.theme.clone();
            let theme = crate::theme::Theme::from_name(&self.config_shared.theme);
            theme.apply(ctx);
        }
        // No periodic repaint — egui redraws on user interaction automatically.
        // Async callbacks (health poll, streaming) call ctx.request_repaint() when data arrives.

        // Reap zombie child if backend exited
        if let Some(ref mut child) = self.backend_child {
            match child.try_wait() {
                Ok(None) => {} // still running
                Ok(Some(status)) => {
                    eprintln!("go-on backend exited (code: {:?})", status.code());
                    self.backend_child = None;
                    self.backend_crash_time = Some(Instant::now());
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
            let done = self.setup_view.show(ctx, &self.i18n, &mut self.config);
            if done {
                self.show_setup = false;
                self.has_providers = has_valid_providers(&self.config);
                save_app_config(&self.config);
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
            ui.horizontal_wrapped(|ui| {
                let title_color = if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(236, 241, 255)
                } else {
                    egui::Color32::from_rgb(19, 53, 110)
                };
                ui.label(
                    egui::RichText::new(self.i18n.t("app.title"))
                        .text_style(egui::TextStyle::Name("Title".into()))
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
                        egui::Color32::from_rgb(20, 120, 70)
                    } else {
                        egui::Color32::from_rgb(198, 60, 60)
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
        let allowed_when_offline = ["monitor", "providers", "settings"];
        let mut new_tab: Option<String> = None;
        let mut blocked_tab: Option<String> = None;
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                for tab in &tabs {
                    let label = self.tab_label(tab);
                    let is_active = self.active_tab == *tab;
                    let blocked = !is_connected && !allowed_when_offline.contains(&tab.as_str());
                    let resp = ui
                        .add_enabled_ui(!blocked, |ui| ui.selectable_label(is_active, label))
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
        if let Some(t) = new_tab {
            self.active_tab = t;
        }
        if let Some(t) = tab_shortcut {
            // Apply offline guard for keyboard shortcuts too
            if is_connected || allowed_when_offline.contains(&t.as_str()) {
                self.active_tab = t;
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

        // Main content
        egui::CentralPanel::default().show(ctx, |ui| {
            let has_backend = self.has_providers;
            let monitor_history_alerts_enabled = self.config.features.monitor_history_alerts;
            let skills_lifecycle_enabled = self.config.features.skills_lifecycle;
            let workflow_run_center_enabled = self.config.features.workflow_run_center;
            let autotune_chain_enabled = self.config.features.autotune_chain_injection;
            let config_safe_mode_enabled = self.config.features.config_safe_mode;
            let providers_ops_enabled = self.config.features.providers_ops;
            match self.active_tab.as_str() {
                "monitor" => self.monitor_view.show(
                    ui,
                    &self.i18n,
                    has_backend,
                    &self.backend,
                    monitor_history_alerts_enabled,
                ),
                "chat" => {
                    let stability = &self.config.ui_stability;
                    self.chat_view.show(
                        ui,
                        &self.i18n,
                        &self.backend,
                        ctx,
                        autotune_chain_enabled,
                        ChatUiRuntimeConfig {
                            repaint_interval_ms: stability.chat_repaint_interval_ms,
                            stream_chunk_flush_ms: stability.chat_stream_chunk_flush_ms,
                            max_pending_events_per_frame: stability
                                .chat_max_pending_events_per_frame,
                        },
                    );
                }
                "skills" => self.skills_view.show(
                    ui,
                    &self.i18n,
                    &self.backend,
                    ctx,
                    skills_lifecycle_enabled,
                ),
                "settings" => {
                    SettingsView::show(ui, &self.i18n, &mut self.config);
                    // Show restart button if backend URL changed
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
                            egui::RichText::new(self.i18n.t("settings.backendUrlHint")).weak(),
                        );
                    }
                }
                "workflow" => self.workflow_view.show(
                    ui,
                    &self.i18n,
                    ctx,
                    &self.backend,
                    workflow_run_center_enabled,
                ),
                "autotune" => self.autotune_view.show(ui, &self.i18n),
                "security" => self.security_view.show(ui, &self.i18n, &self.backend, ctx),
                "config" => {
                    self.config_editor_view.show(
                        ui,
                        &self.i18n,
                        &mut self.config,
                        config_safe_mode_enabled,
                    );
                    // Config changes may affect backend connectivity — reset chat cache only on apply
                    if self.config_editor_view.applied {
                        self.config_editor_view.applied = false;
                        self.chat_view.reset_loaded_state();
                        // Track original URL so Settings tab can detect and show restart button
                        if self.config.backend_url != self.backend_url_original {
                            self.backend_url_original = self.config.backend_url.clone();
                        }
                    }
                }
                "providers" => {
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
                        // Providers page may have added/updated API keys in keyring.
                        // Restart backend so it picks up the new keys.
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

        // Frame timing diagnostics — write to log file
        let frame_elapsed = _frame_start.elapsed();
        if frame_elapsed.as_millis() > 50 {
            log_msg(&format!(
                "FRAME_DIAG: [{}] took {}ms",
                self.active_tab,
                frame_elapsed.as_millis()
            ));
        }
    }
}

impl Drop for GoOnApp {
    fn drop(&mut self) {
        // Abort any in-flight chat generation tasks
        self.chat_view.stop_sending();

        if let Some(mut child) = self.backend_child.take() {
            eprintln!("Shutting down go-on backend (PID: {})...", child.id());
            let _ = child.kill();
            // Don't block shutdown on backend process exit
            std::thread::spawn(move || {
                let _ = child.wait();
            });
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
