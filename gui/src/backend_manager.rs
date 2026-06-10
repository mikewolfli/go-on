//! Backend binary management — discovery, spawning, and config generation.
//!
//! Extracted from `app.rs` to keep the main application file focused on the
//! UI lifecycle and eframe integration.

use crate::backend::BackendClient;
use crate::config::AppConfig;
use crate::keyring_util::REDACTED_API_KEY;

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════════
// Backend binary path discovery
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

pub(crate) fn backend_log_path() -> Option<std::path::PathBuf> {
    find_backend_binary().and_then(|path| path.parent().map(|p| p.join("backend.log")))
}

pub(crate) fn backend_log_has_addr_in_use() -> bool {
    backend_log_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|s| s.contains("Address already in use"))
}

pub(crate) fn backend_bind_addr_from_url(url: &str) -> String {
    let without_scheme = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    match without_scheme.find('/') {
        Some(pos) => without_scheme[..pos].to_string(),
        None => without_scheme.to_string(),
    }
}

pub(crate) fn is_addr_listening(addr: &str) -> bool {
    let Ok(candidates) = addr.to_socket_addrs() else {
        return false;
    };
    candidates
        .into_iter()
        .any(|sock| TcpStream::connect_timeout(&sock, Duration::from_millis(150)).is_ok())
}

// ═══════════════════════════════════════════════════════════════════════════
// Backend lifecycle — config generation, spawning, diagnostics
// ═══════════════════════════════════════════════════════════════════════════

/// Print diagnostic info about key sources for debugging.
/// Only active in debug builds to avoid unnecessary keyring calls in production.
#[allow(dead_code)] // F-GAP-49 — reserved for future backend lifecycle re-integration
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
        for name in crate::views::providers::PROVIDER_NAMES {
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

/// Start or restart the backend child process with fresh env vars from keyring.
#[allow(dead_code)] // F-GAP-49 — reserved for future backend lifecycle re-integration
pub(crate) fn spawn_backend(
    config: &AppConfig,
) -> (BackendClient, Option<std::process::Child>, bool) {
    diagnostic_key_report(config);

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

            // Regenerate backend config.toml on every start to keep it
            // in sync with the GUI's provider configuration.
            // The file includes a header marking it as auto-generated.
            let backend_cfg_path = config_dir.join("config.toml");
            generate_backend_config(&backend_cfg_path, config);

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
#[allow(dead_code)] // F-GAP-49 — reserved for future backend lifecycle re-integration
pub(crate) fn generate_backend_config(path: &std::path::Path, config: &AppConfig) {
    /// Clamp a temperature value to the valid range [0.0, 2.0].
    /// F-GAP-55: Reserved for future config validation wiring
    #[allow(dead_code)]
    fn clamp_temperature(v: f32) -> f32 {
        v.clamp(0.0, 2.0)
    }

    /// Clamp top_p to the valid range [0.0, 1.0].
    /// F-GAP-55: Reserved for future config validation wiring
    #[allow(dead_code)]
    fn clamp_top_p(v: f32) -> f32 {
        v.clamp(0.0, 1.0)
    }

    /// Clamp max_tokens to the valid range [1, 1_048_576].
    /// F-GAP-55: Reserved for future config validation wiring
    #[allow(dead_code)]
    fn clamp_max_tokens(v: u32) -> u32 {
        v.clamp(1, 1_048_576)
    }

    /// Clamp a u64 value to the range [min, max].
    /// F-GAP-55: Reserved for future config validation wiring
    #[allow(dead_code)]
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
        eprintln!(
            "WARNING: No providers have valid API keys. Generated config.toml will have no agents."
        );
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
min_query_chars = 80
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
                        Ok(_) => {
                            eprintln!("backend: wrote zed-config.toml to {}", zed_path.display())
                        }
                        Err(e) => eprintln!("backend: failed to write zed-config.toml: {}", e),
                    }
                }
                Err(e) => eprintln!("backend: failed to write zed-config.toml: {}", e),
            }
        }
    }
}
