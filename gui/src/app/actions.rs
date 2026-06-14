//! Backend lifecycle, state sync polling, and application construction.
//!
//! This module handles:
//! - Backend binary discovery and spawning
//! - Backend config.toml generation
//! - Health polling with debounce and progressive backoff
//! - State sync SSE listener polling
//! - Application constructor and restart logic
//! - Tab UI state persistence
//!
//! Free utility functions for backend address resolution and port checking
//! are also defined here so that actions.rs is self-contained for the
//! backend lifecycle operations it drives.

use super::GoOnApp;
use crate::backend::{BackendClient, HealthStatus};
use crate::config::{has_valid_providers, AppConfig};
use crate::config_store::ConfigStore;
use crate::connection::{BackendUpdate, ConnectionManager};
use crate::crash_recovery::CrashRecovery;
use crate::i18n::{I18n, Lang};
use crate::keyring_util::REDACTED_API_KEY;
use crate::state_sync::StateSyncEvent;
use crate::view_registry::ViewRegistry;
#[cfg(debug_assertions)]
use crate::views::providers::PROVIDER_NAMES;
use crate::views::ui_state::GlobalUiState;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════
// Free functions: backend binary discovery, address resolution, port checking
// ═══════════════════════════════════════════════════════════════════════════

/// Find the go-on backend binary path relative to the GUI executable.
pub(crate) fn find_backend_binary() -> Option<std::path::PathBuf> {
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

/// Extract the bind address (host:port) from a backend URL string.
/// Strips the scheme and path components.
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

/// Check whether a TCP address is already listening (150ms connect timeout).
fn is_addr_listening(addr: &str) -> bool {
    let Ok(candidates) = addr.to_socket_addrs() else {
        return false;
    };
    candidates
        .into_iter()
        .any(|sock| TcpStream::connect_timeout(&sock, Duration::from_millis(150)).is_ok())
}

// ═══════════════════════════════════════════════════════════════════════════
// GoOnApp impl — tab state persistence, backend lifecycle, polling
// ═══════════════════════════════════════════════════════════════════════════

impl GoOnApp {
    // ── Tab UI state persistence ─────────────────────────────────────────

    /// Save the given tab's transient UI state into `self.ui_state`.
    pub(crate) fn save_tab_ui_state(&mut self, tab: &str) {
        match tab {
            "chat" => {
                self.views.chat_view.save_ui_state(&mut self.ui_state);
            }
            "monitor" => {
                self.ui_state.monitor_metrics_window =
                    self.views.monitor_view.metrics_window.clone();
                self.ui_state.monitor_auto_refresh_interval =
                    self.views.monitor_view.auto_refresh_interval;
                self.ui_state.monitor_provider_filter =
                    self.views.monitor_view.provider_filter.clone();
            }
            "providers" => {
                self.ui_state.providers_selected_provider =
                    self.views.providers_view.selected_provider.clone();
                self.ui_state.providers_new_model = self.views.providers_view.new_model.clone();
                self.ui_state.providers_new_label = self.views.providers_view.new_label.clone();
            }
            "skills" => {
                self.ui_state.skills_show_create = self.views.skills_view.show_create;
                self.ui_state.skills_show_import = self.views.skills_view.show_import;
                self.ui_state.skills_selected_skill_name =
                    self.views.skills_view.selected_skill_name.clone();
                self.ui_state.skills_edit_desc = self.views.skills_view.edit_desc.clone();
                self.ui_state.skills_edit_prompt = self.views.skills_view.edit_prompt.clone();
                self.ui_state.skills_edit_schema = self.views.skills_view.edit_schema.clone();
                self.ui_state.skills_test_input = self.views.skills_view.test_input.clone();
                self.ui_state.skills_rollback_version =
                    self.views.skills_view.rollback_version.clone();
                self.ui_state.skills_create_name = self.views.skills_view.create_name.clone();
                self.ui_state.skills_create_desc = self.views.skills_view.create_desc.clone();
                self.ui_state.skills_create_prompt = self.views.skills_view.create_prompt.clone();
                self.ui_state.skills_create_schema =
                    self.views.skills_view.create_input_schema.clone();
                self.ui_state.skills_import_url = self.views.skills_view.import_url.clone();
            }
            "workflow" => {
                self.ui_state.workflow_run_status_filter =
                    self.views.workflow_view.run_status_filter.clone();
                self.ui_state.workflow_selected_run_id =
                    self.views.workflow_view.selected_run_id.clone();
                self.ui_state.workflow_new_name = self.views.workflow_view.new_name.clone();
                self.ui_state.workflow_new_command = self.views.workflow_view.new_command.clone();
            }
            "config" => {
                self.ui_state.config_editor_draft = self.views.config_editor_view.draft.clone();
                self.ui_state.config_editor_search =
                    self.views.config_editor_view.search_query.clone();
                self.ui_state.config_editor_snapshots =
                    self.views.config_editor_view.snapshots.clone();
            }
            _ => {}
        }
    }

    /// Restore the given tab's transient UI state from `self.ui_state`.
    pub(crate) fn restore_tab_ui_state(&mut self, tab_name: &str) {
        match tab_name {
            "chat" => {
                // Only restore mode if saved value is valid — otherwise keep existing default
                let valid_modes = ["ask", "plan", "edit", "safeguard", "full_auto"];
                if !self.ui_state.selected_mode.is_empty()
                    && valid_modes.contains(&self.ui_state.selected_mode.as_str())
                {
                    self.views.chat_view.selected_mode = self.ui_state.selected_mode.clone();
                }
                self.views.chat_view.show_token_details = self.ui_state.show_token_details;
                self.views.chat_view.enable_markdown = self.ui_state.enable_markdown;
                self.views.chat_view.show_model_picker = self.ui_state.show_model_picker;
                self.views.chat_view.show_prompts = self.ui_state.show_prompts;
                if let Some(json) = &self.ui_state.model_stats_json {
                    if let Ok(stats) = serde_json::from_str(json) {
                        self.views.chat_view.model_stats = stats;
                    }
                }
                if self.ui_state.active_session < self.views.chat_view.sessions.len() {
                    self.views.chat_view.active_session = self.ui_state.active_session;
                }
                self.views.chat_view.input = self.ui_state.input_draft.clone();
                self.views.chat_view.session_search_query =
                    self.ui_state.session_search_query.clone();
                self.views.chat_view.template_search_query =
                    self.ui_state.template_search_query.clone();
            }
            "monitor" => {
                self.views.monitor_view.metrics_window =
                    self.ui_state.monitor_metrics_window.clone();
                if self.ui_state.monitor_auto_refresh_interval > 0 {
                    self.views.monitor_view.auto_refresh_interval =
                        self.ui_state.monitor_auto_refresh_interval;
                }
                self.views.monitor_view.provider_filter =
                    self.ui_state.monitor_provider_filter.clone();
            }
            "providers" => {
                self.views.providers_view.selected_provider =
                    self.ui_state.providers_selected_provider.clone();
                if !self.ui_state.providers_new_model.is_empty() {
                    self.views.providers_view.new_model = self.ui_state.providers_new_model.clone();
                }
                self.views.providers_view.new_label = self.ui_state.providers_new_label.clone();
            }
            "skills" => {
                self.views.skills_view.show_create = self.ui_state.skills_show_create;
                self.views.skills_view.show_import = self.ui_state.skills_show_import;
                if !self.ui_state.skills_selected_skill_name.is_empty() {
                    self.views
                        .skills_view
                        .load_skill_editor_by_name(&self.ui_state.skills_selected_skill_name);
                }
                self.views.skills_view.edit_desc = self.ui_state.skills_edit_desc.clone();
                self.views.skills_view.edit_prompt = self.ui_state.skills_edit_prompt.clone();
                self.views.skills_view.edit_schema = self.ui_state.skills_edit_schema.clone();
                self.views.skills_view.test_input = self.ui_state.skills_test_input.clone();
                self.views.skills_view.rollback_version =
                    self.ui_state.skills_rollback_version.clone();
                self.views.skills_view.create_name = self.ui_state.skills_create_name.clone();
                self.views.skills_view.create_desc = self.ui_state.skills_create_desc.clone();
                self.views.skills_view.create_prompt = self.ui_state.skills_create_prompt.clone();
                self.views.skills_view.create_input_schema =
                    self.ui_state.skills_create_schema.clone();
                self.views.skills_view.import_url = self.ui_state.skills_import_url.clone();
            }
            "workflow" => {
                self.views.workflow_view.run_status_filter =
                    self.ui_state.workflow_run_status_filter.clone();
                self.views.workflow_view.selected_run_id =
                    self.ui_state.workflow_selected_run_id.clone();
                self.views.workflow_view.new_name = self.ui_state.workflow_new_name.clone();
                self.views.workflow_view.new_command = self.ui_state.workflow_new_command.clone();
            }
            "config" => {
                self.views.config_editor_view.draft = self.ui_state.config_editor_draft.clone();
                self.views.config_editor_view.search_query =
                    self.ui_state.config_editor_search.clone();
                self.views.config_editor_view.snapshots =
                    self.ui_state.config_editor_snapshots.clone();
            }
            _ => {}
        }
    }

    // ── UI stability configuration helpers ───────────────────────────────

    pub(crate) fn backend_refresh_interval(&self) -> Duration {
        Duration::from_secs(
            self.config_store
                .shared()
                .ui_stability
                .backend_refresh_interval_secs
                .clamp(1, 60),
        )
    }

    pub(crate) fn backend_ui_commit_debounce(&self) -> Duration {
        Duration::from_millis(
            self.config_store
                .shared()
                .ui_stability
                .backend_ui_commit_debounce_ms
                .clamp(16, 1000),
        )
    }

    pub(crate) fn health_disconnect_debounce_count(&self) -> u8 {
        self.config_store
            .shared()
            .ui_stability
            .health_disconnect_debounce_count
            .clamp(1, 8)
    }

    // ── Diagnostic key report (debug-only) ───────────────────────────────

    /// Print diagnostic info about key sources for debugging.
    /// Only active in debug builds to avoid unnecessary keyring calls in production.
    pub(crate) fn diagnostic_key_report(config: &AppConfig) {
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
            for name in PROVIDER_NAMES {
                let lower = name.to_lowercase();
                if seen.insert(lower.clone()) {
                    all_names.push(lower);
                }
            }
            for name in &all_names {
                let config_has_key = config
                    .providers
                    .iter()
                    .any(|p| p.name.to_lowercase() == *name && !p.api_key.is_empty());
                let keyring_has_key = crate::keyring_util::get_api_key(name)
                    .map(|k| !k.is_empty())
                    .unwrap_or(false);
                eprintln!(
                    "  {}: config={}, keyring={}",
                    name,
                    if config_has_key {
                        "(present)"
                    } else {
                        "(empty)"
                    },
                    if keyring_has_key {
                        "(present)"
                    } else {
                        "(not in keyring)"
                    }
                );
            }
            eprintln!("=== END DIAGNOSTIC ===");
        }
    }

    // ── Backend spawning and config generation ───────────────────────────

    /// Start or restart the backend child process with fresh env vars from keyring.
    pub(crate) fn spawn_backend(
        config: &AppConfig,
    ) -> (BackendClient, Option<std::process::Child>, bool) {
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
                            .or_else(|_| std::env::var("USERPROFILE")) // Windows
                            .unwrap_or_else(|_| {
                                // Last resort: use current executable directory
                                std::env::current_exe()
                                    .ok()
                                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                                    .to_string_lossy()
                                    .to_string()
                            });
                        std::borrow::Cow::Owned(std::path::PathBuf::from(home))
                    }
                };
                let mut cmd = std::process::Command::new(&path);
                cmd.current_dir(&config_dir)
                    .arg("--protocol-mode")
                    .arg(&config.protocol_mode)
                    .arg("--low-memory")
                    .stdout(std::process::Stdio::null());

                // API keys are NOT injected into the backend process environment.
                // The generated config.toml uses `keyring://go-on/{name}_api_key` URIs
                // and the backend resolves these via system keyring (libsecret, Keychain,
                // Credential Manager). This ensures secrets stay in the secure keyring
                // rather than leaking into the process environment (visible via /proc/PID/environ).
                //
                // For headless/server deployments, operators can still set env vars directly
                // (e.g., DEEPSEEK_API_KEY=sk-xxx) which the backend's load_secret_value()
                // uses as fallback when keyring:// resolution fails.

                // Sync language between GUI and backend
                cmd.env("LANG", &config.language);

                // Only generate a default config.toml if one does NOT already exist.
                // User-manual configs take precedence.
                let backend_cfg_path = config_dir.join("config.toml");
                if !backend_cfg_path.exists() {
                    Self::generate_backend_config(&backend_cfg_path, config);
                }

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
    pub(crate) fn generate_backend_config(path: &std::path::Path, config: &AppConfig) {
        /// Clamp a temperature value to the valid range [0.0, 2.0].
        #[allow(dead_code)] // F-GAP-48 — reserved action features
        fn clamp_temperature(v: f32) -> f32 {
            v.clamp(0.0, 2.0)
        }

        /// Clamp top_p to the valid range [0.0, 1.0].
        #[allow(dead_code)] // F-GAP-48 — reserved action features
        fn clamp_top_p(v: f32) -> f32 {
            v.clamp(0.0, 1.0)
        }

        /// Clamp max_tokens to the valid range [1, 1_048_576].
        #[allow(dead_code)] // F-GAP-48 — reserved action features
        fn clamp_max_tokens(v: u32) -> u32 {
            v.clamp(1, 1_048_576)
        }

        /// Clamp a u64 value to the range [min, max].
        #[allow(dead_code)] // F-GAP-48 — reserved action features
        fn clamp_u64(v: u64, min: u64, max: u64) -> u64 {
            v.clamp(min, max)
        }

        // == Validation helpers available above ==

        // Provider metadata is sourced from the authoritative `built_in_provider_specs()`
        // in `gui/src/views/providers/catalog.rs`. This is a sync fallback used when
        // `generate_backend_config()` is called before the backend is running. The backend's
        // `/provider/catalog` endpoint is the canonical source at runtime.
        use crate::views::providers::catalog::built_in_provider_specs;

        // ===== Section: TOML Generation =====
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
                let spec = built_in_provider_specs(&name);
                let agent_type = spec.agent_type;
                let default_url = spec.default_url;
                let default_model = spec.default_model;
                let supports_system = spec.supports_system;

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
                let api_key_env = format!("keyring://go-on/{}_api_key", name);

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

        // Atomic write: write to temp file, then rename
        match tempfile::NamedTempFile::new_in(path.parent().unwrap_or(std::path::Path::new("."))) {
            Ok(mut tmp) => {
                use std::io::Write;
                match (|| -> Result<(), std::io::Error> {
                    tmp.write_all(toml.as_bytes())?;
                    tmp.flush()?;
                    tmp.persist(path)?;
                    Ok(())
                })() {
                    Ok(_) => eprintln!("backend: wrote config.toml to {}", path.display()),
                    Err(e) => eprintln!("backend: failed to write config.toml: {}", e),
                }
            }
            Err(e) => eprintln!("backend: failed to write config.toml: {}", e),
        }

        // ===== Section: Zed Config =====
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
top_k = 2

{agent_section}"#,
                    agent_section = agent_section,
                );
                // Atomic write: write to temp file, then rename
                match tempfile::NamedTempFile::new_in(
                    zed_path.parent().unwrap_or(std::path::Path::new(".")),
                ) {
                    Ok(mut tmp) => {
                        use std::io::Write;
                        match (|| -> Result<(), std::io::Error> {
                            tmp.write_all(zed_toml.as_bytes())?;
                            tmp.flush()?;
                            tmp.persist(zed_path)?;
                            Ok(())
                        })() {
                            Ok(_) => eprintln!(
                                "backend: wrote zed-config.toml to {}",
                                zed_path.display()
                            ),
                            Err(e) => eprintln!("backend: failed to write zed-config.toml: {}", e),
                        }
                    }
                    Err(e) => eprintln!("backend: failed to write zed-config.toml: {}", e),
                }
            }
        }
    }

    // ── Backend restart lifecycle ────────────────────────────────────────

    /// Kill the current backend child and schedule a restart after a brief cooldown.
    /// Called after adding/updating API keys so the new keys take effect immediately.
    /// Uses a non-blocking cooldown (via request_repaint_after) instead of
    /// thread::sleep(300ms) on the UI thread to avoid freezing the GUI.
    pub(crate) fn restart_backend(&mut self, ctx: &egui::Context) {
        // Increment crash counter unconditionally — this is called either from
        // the crash-auto-restart path (where backend_child is already None) or
        // from the manual restart path (provider add, URL change, etc.).
        // Counting on both paths ensures the give-up gate (count >= 10) works.
        self.crash.backend_crash_count = self.crash.backend_crash_count.saturating_add(1);

        // Kill old process
        if let Some(mut child) = self.connection.backend_child.take() {
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
        // Schedule non-blocking cooldown so the old process can release the port
        // before we spawn the new one (prevents EADDRINUSE).
        self.crash.restart_cooldown_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(300));
        ctx.request_repaint_after(std::time::Duration::from_millis(300));

        // Reset state for the new backend (will be spawned once cooldown elapses)
        self.connection.pending_refresh = false;
        self.connection.last_refresh = Instant::now() - std::time::Duration::from_secs(10);
        self.connection.staged_health = None;
        self.connection.staged_providers = None;
        self.connection.staged_refresh_done = false;
        self.connection.health_disconnect_streak = 0;
        // Clear stale health/providers so monitor shows correct state
        self.views.monitor_view.health = None;
        self.views.monitor_view.providers = Vec::new();
        // Also reset chat cache so it re-fetches models from new backend
        self.views.chat_view.reset_loaded_state();
        // Reset providers loaded state so models are re-fetched
        self.views.providers_view.reset_loaded_state();
        eprintln!("Backend restart scheduled (cooldown 300ms)...");
    }

    /// Spawn the new backend process after the restart cooldown has elapsed.
    /// Called from update() when restart_cooldown_until is set and expired.
    /// Also triggers async protocol version discovery to determine the chat endpoint.
    pub(crate) fn finish_restart_backend(&mut self) {
        let (backend, child, reused_external) =
            Self::spawn_backend(self.config_store.shared().as_ref());
        // Kick off async protocol version discovery
        let discovery_backend = backend.clone();
        tokio::spawn(async move {
            discovery_backend.discover_protocol_version().await;
        });
        self.connection.backend = backend;
        self.connection.backend_child = child;
        self.connection.backend_reused_external = reused_external;
        self.crash.restart_cooldown_until = None;
        eprintln!("Backend restarted after cooldown");
    }

    // ── Constructor ──────────────────────────────────────────────────────

    /// Detect the localized window title based on the saved config language.
    /// Called once at startup before the I18n instance is created.
    /// NOTE: Currently unused — the window title is set via egui context directly.
    /// Retained for reference in case programmatic title setting is re-enabled.
    #[allow(dead_code)] // F-GAP-48: Reserved for future window title detection feature
    pub fn detect_initial_window_title(config: &AppConfig) -> String {
        if config.language == "zh-CN" {
            "Go-On 图形界面".to_string()
        } else if config.language == "zh-TW" {
            "Go-On 圖形界面".to_string()
        } else {
            "Go-On GUI".to_string()
        }
    }

    /// Construct a new `GoOnApp` instance.
    ///
    /// This is the primary constructor. It:
    /// 1. Detects the system language (if not explicitly configured)
    /// 2. Spawns or reuses an external backend process
    /// 3. Loads persisted UI state
    /// 4. Starts the cross-client SSE state sync listener
    /// 5. Kicks off async protocol version discovery
    /// 6. Pre-loads prompts data for chat command expansion
    pub fn new(config: AppConfig) -> Self {
        // Auto-detect: if user hasn't explicitly set a language, try system locale
        let lang = if config.language.is_empty() || config.language == "en" {
            super::detect_system_language()
        } else {
            match config.language.as_str() {
                "zh-CN" => Lang::ZhCn,
                "zh-TW" => Lang::ZhTw,
                _ => Lang::En,
            }
        };
        let providers_valid = has_valid_providers(&config);
        let backend_url = config.backend_url.clone();

        // Start backend with env vars from keyring
        let (backend, backend_child, backend_reused_external) = Self::spawn_backend(&config);

        let ui_state = GlobalUiState::load();

        // ── Start cross-client state sync SSE listener ────────────────
        let (state_sync_tx, state_sync_rx) = std::sync::mpsc::channel();
        let sync_url = config.backend_url.clone();
        crate::state_sync::start_state_sync_listener(&sync_url, state_sync_tx);

        let mut app = Self {
            config_store: ConfigStore::new(config),
            connection: ConnectionManager::new(
                backend,
                backend_child,
                backend_reused_external,
                backend_url,
            ),
            crash: CrashRecovery::new(),
            views: ViewRegistry::new(),
            i18n: I18n::new(lang),
            show_setup: !providers_valid,
            // Internal tab IDs must stay stable (English keys); labels are localized in UI.
            active_tab: "monitor".to_string(),
            has_providers: providers_valid,
            last_applied_theme: String::new(),
            blocked_tab_toast_shown: None,
            last_prompts_command_version: 0,
            last_prompts_lang: lang,
            ui_state,
            render_cache: super::CachedRender::new(),
            state_sync_rx: Some(state_sync_rx),
            frame_count: 0,
        };

        // Kick off async protocol version discovery to determine the chat endpoint.
        // This runs in the background and updates the shared chat_endpoint.
        {
            let discovery_backend = app.connection.backend.clone();
            tokio::spawn(async move {
                discovery_backend.discover_protocol_version().await;
            });
        }

        // Pre-load prompts for chat `/` command expansion and category browser,
        // regardless of whether the Prompts tab itself is visible.
        app.views.prompts_view.ensure_loaded(lang);

        app
    }

    // ── Health debouncing ────────────────────────────────────────────────

    /// Apply a debounce filter to a health status update.
    ///
    /// When the current state is connected and the incoming update shows
    /// disconnected, the disconnect is buffered for `health_disconnect_debounce_count`
    /// consecutive samples before being committed. This prevents transient
    /// blips (e.g., backend GC pause, brief network hiccup) from causing UI jitter.
    pub(crate) fn apply_health_debounce(&mut self, mut next: HealthStatus) -> HealthStatus {
        let was_connected = self
            .views
            .monitor_view
            .health
            .as_ref()
            .is_some_and(|h| h.connected);

        if next.connected {
            self.connection.health_disconnect_streak = 0;
            return next;
        }

        if was_connected {
            self.connection.health_disconnect_streak =
                self.connection.health_disconnect_streak.saturating_add(1);
            if self.connection.health_disconnect_streak < self.health_disconnect_debounce_count() {
                if let Some(ref prev) = self.views.monitor_view.health {
                    next = prev.clone();
                }
            }
        }

        next
    }

    // ── Language helpers ─────────────────────────────────────────────────

    /// Get the current `Lang` from the shared config's language code.
    pub(crate) fn current_lang(&self) -> Lang {
        match self.config_store.current_lang_code() {
            "zh-CN" => Lang::ZhCn,
            "zh-TW" => Lang::ZhTw,
            _ => Lang::En,
        }
    }

    // ── Backend URL sync ─────────────────────────────────────────────────

    /// Sync the backend URL on `BackendClient` if the config has changed.
    pub(crate) fn sync_backend_url(&mut self) {
        let config_url = self
            .config_store
            .shared()
            .backend_url
            .trim_end_matches('/')
            .to_string();
        if self.connection.backend.base_url() != config_url {
            self.connection.backend.set_base_url(&config_url);
        }
    }

    // ── State sync SSE polling ───────────────────────────────────────────

    /// Poll cross-client state sync events and dispatch to UI.
    pub(crate) fn poll_state_sync_events(&mut self, ctx: &egui::Context) {
        let Some(ref rx) = self.state_sync_rx else {
            return;
        };
        let mut requested_refresh = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                StateSyncEvent::ConfigReloaded { .. } => {
                    eprintln!("[state-sync] Config reloaded; refreshing backend data");
                    requested_refresh = true;
                }
                StateSyncEvent::ModelsChanged { .. } => {
                    eprintln!("[state-sync] Models changed; refreshing providers");
                    requested_refresh = true;
                }
                StateSyncEvent::AgentsChanged { added, removed } => {
                    if !added.is_empty() {
                        eprintln!("[state-sync] Agents added: {:?}", added);
                    }
                    if !removed.is_empty() {
                        eprintln!("[state-sync] Agents removed: {:?}", removed);
                    }
                    requested_refresh = true;
                }
                StateSyncEvent::BackendRestarting { reason, .. } => {
                    eprintln!("[state-sync] Backend restarting: {}", reason);
                    self.connection.consecutive_poll_failures = 10;
                    self.connection.last_refresh = std::time::Instant::now()
                        - self.backend_refresh_interval()
                        - std::time::Duration::from_secs(1);
                }
                StateSyncEvent::Heartbeat { .. } => {}
            }
        }
        if requested_refresh {
            self.connection.pending_refresh = false;
            self.connection.last_refresh = std::time::Instant::now()
                - self.backend_refresh_interval()
                - std::time::Duration::from_secs(1);
            ctx.request_repaint();
        }
    }

    // ── Backend update polling (health + providers) ──────────────────────

    /// Poll the async backend update channel and commit staged updates.
    ///
    /// Use the `backend_ui_commit_debounce` to batch updates and reduce UI jitter.
    /// Also applies health debouncing to filter transient disconnect blips.
    pub(crate) fn poll_backend_updates(&mut self, ctx: &egui::Context) {
        let mut received_any = false;
        let mut processed = 0;
        while let Ok(update) = self.connection.backend_updates.try_recv() {
            received_any = true;
            processed += 1;
            if processed > 128 {
                eprintln!(
                    "poll_backend_updates: discarding {} queued updates (processing limit)",
                    processed
                );
                // Drain remaining to prevent channel growth.
                // Process remaining updates (up to 128) instead of silently discarding them.
                let mut drained = 0;
                while drained < 128 {
                    match self.connection.backend_updates.try_recv() {
                        Ok(update) => {
                            match update {
                                BackendUpdate::Health(h) => self.connection.staged_health = Some(h),
                                BackendUpdate::Providers(p) => {
                                    self.connection.staged_providers = Some(p)
                                }
                                BackendUpdate::RefreshDone => {
                                    self.connection.staged_refresh_done = true
                                }
                            }
                            drained += 1;
                        }
                        Err(_) => break,
                    }
                }
                break;
            }
            match update {
                BackendUpdate::Health(h) => self.connection.staged_health = Some(h),
                BackendUpdate::Providers(p) => self.connection.staged_providers = Some(p),
                BackendUpdate::RefreshDone => self.connection.staged_refresh_done = true,
            }
        }

        if !received_any {
            return;
        }

        let should_commit = self.connection.staged_refresh_done
            || self.connection.last_backend_ui_commit.elapsed()
                >= self.backend_ui_commit_debounce();
        if !should_commit {
            return;
        }

        let mut changed = false;

        if let Some(next_health) = self.connection.staged_health.take() {
            let debounced = self.apply_health_debounce(next_health);
            let is_connected = debounced.connected;
            if self.views.monitor_view.health.as_ref() != Some(&debounced) {
                self.views.monitor_view.health = Some(debounced);
                changed = true;
            }
            // Track consecutive poll failures for backoff
            if !is_connected {
                self.connection.consecutive_poll_failures =
                    self.connection.consecutive_poll_failures.saturating_add(1);
            } else {
                self.connection.consecutive_poll_failures = 0;
                // Reset crash count on confirmed healthy connection — a health check
                // with connected=true means the backend is running fine, so any prior
                // "crash" was a legitimate restart (provider add, URL change, etc.).
                self.crash.backend_crash_count = 0;
            }
        }

        if let Some(next_providers) = self.connection.staged_providers.take() {
            if self.views.monitor_view.providers != next_providers {
                self.views.monitor_view.providers = next_providers;
                changed = true;
            }
        }

        if self.connection.staged_refresh_done {
            self.connection.staged_refresh_done = false;
            if self.connection.pending_refresh {
                self.connection.pending_refresh = false;
                changed = true;
            }
        }

        self.connection.last_backend_ui_commit = Instant::now();

        if changed {
            // Signal egui that new data arrived; the frame cache in update()
            // will debounce and skip rendering if content hasn't materially changed.
            ctx.request_repaint();
        }
    }

    // ── Progressive backoff refresh ───────────────────────────────────────

    /// Periodically poll the backend for health and provider status, with
    /// progressive backoff after consecutive failures.
    ///
    /// Backoff sequence: 1s, 2s, 4s, 8s, 16s (capped at 60s max).
    /// Once connected, resumes the normal `backend_refresh_interval`.
    pub(crate) fn maybe_refresh_backend(&mut self) {
        // Progressive backoff: skip polls after consecutive failures
        if self.connection.consecutive_poll_failures > 0 {
            let backoff_secs = (2u64).pow(
                self.connection
                    .consecutive_poll_failures
                    .min(5)
                    .saturating_sub(1) as u32,
            ); // 1, 2, 4, 8, 16
            let max_backoff = 60u64;
            let effective_backoff = backoff_secs.min(max_backoff);
            if self.connection.last_refresh.elapsed()
                < std::time::Duration::from_secs(effective_backoff)
            {
                return;
            }
        }

        if self.connection.last_refresh.elapsed() >= self.backend_refresh_interval()
            && !self.connection.pending_refresh
        {
            self.connection.pending_refresh = true;
            let tx = self.connection.backend_tx.clone();
            let backend = self.connection.backend.clone();
            tokio::spawn(async move {
                // Add timeout to prevent hanging if backend is not responding
                let health =
                    match tokio::time::timeout(std::time::Duration::from_secs(5), backend.health())
                        .await
                    {
                        Ok(h) => h,
                        Err(_) => {
                            super::log_msg("Warning: Backend health check timed out");
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
                    super::log_msg(&format!("WARN: app try_send failed: {:?}", e));
                }

                let providers = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend.provider_status(),
                )
                .await
                {
                    Ok(p) => p,
                    Err(_) => {
                        super::log_msg("Warning: Backend provider status check timed out");
                        vec![]
                    }
                };
                if let Err(e) = tx.try_send(BackendUpdate::Providers(providers)) {
                    super::log_msg(&format!("WARN: app try_send failed: {:?}", e));
                }
                if let Err(e) = tx.try_send(BackendUpdate::RefreshDone) {
                    super::log_msg(&format!("WARN: app try_send failed: {:?}", e));
                }
            });
            self.connection.last_refresh = Instant::now();
        }
    }
}
