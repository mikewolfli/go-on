use crate::keyring_util::REDACTED_API_KEY;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub backend_url: String,
    pub language: String,
    pub theme: String,
    /// Font scale factor for accessibility. 1.0 = default size.
    #[serde(default = "default_font_scale")]
    pub font_scale: f64,
    /// Protocol mode for backend connection.
    /// Allowed: "adaptive", "acp_http", "mcp_http", "acp_stdio", "mcp_stdio"
    #[serde(default = "default_protocol_mode")]
    pub protocol_mode: String,
    #[serde(default)]
    pub ui_stability: UiStabilityConfig,
    pub features: FeatureToggles,
    pub enterprise: EnterpriseConfig,
    pub providers: Vec<ProviderConfig>,
}

fn default_protocol_mode() -> String {
    "adaptive".to_string()
}

fn default_font_scale() -> f64 {
    1.0
}

fn default_stream_token_flush_ms() -> u64 {
    16
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiStabilityConfig {
    pub backend_refresh_interval_secs: u64,
    pub backend_ui_commit_debounce_ms: u64,
    pub health_disconnect_debounce_count: u8,
    pub chat_stream_chunk_flush_ms: u64,
    pub chat_repaint_interval_ms: u64,
    pub chat_max_pending_events_per_frame: usize,
    /// Minimum interval (ms) between token batch flushes to the UI.
    /// Controls frame rate — e.g. 16ms → ~60fps.
    #[serde(default = "default_stream_token_flush_ms")]
    pub stream_token_flush_ms: u64,
}

impl UiStabilityConfig {
    /// Clamp all numeric fields to sensible ranges to prevent
    /// misconfigured values from causing UI hangs or excessive repaints.
    fn clamp_to_sensible_ranges(&mut self) {
        self.backend_refresh_interval_secs = self.backend_refresh_interval_secs.clamp(1, 300);
        self.backend_ui_commit_debounce_ms = self.backend_ui_commit_debounce_ms.clamp(16, 5000);
        self.health_disconnect_debounce_count = self.health_disconnect_debounce_count.clamp(0, 10);
        self.chat_stream_chunk_flush_ms = self.chat_stream_chunk_flush_ms.clamp(1, 500);
        self.chat_repaint_interval_ms = self.chat_repaint_interval_ms.clamp(1, 1000);
        self.chat_max_pending_events_per_frame =
            self.chat_max_pending_events_per_frame.clamp(1, 4096);
        self.stream_token_flush_ms = self.stream_token_flush_ms.clamp(1, 500);
    }
}

impl Default for UiStabilityConfig {
    fn default() -> Self {
        Self {
            backend_refresh_interval_secs: 5,
            backend_ui_commit_debounce_ms: 120,
            health_disconnect_debounce_count: 2,
            chat_stream_chunk_flush_ms: 8,
            chat_repaint_interval_ms: 16,
            chat_max_pending_events_per_frame: 256,
            stream_token_flush_ms: 16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterpriseConfig {
    pub active_environment: String,
    pub environments: Vec<EnvironmentPreset>,
    pub secret_source: String,
    pub import_path: String,
    pub export_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentPreset {
    pub name: String,
    pub backend_url: String,
}

impl Default for EnterpriseConfig {
    fn default() -> Self {
        Self {
            active_environment: "dev".to_string(),
            environments: vec![
                EnvironmentPreset {
                    name: "dev".to_string(),
                    backend_url: "http://127.0.0.1:8090".to_string(),
                },
                EnvironmentPreset {
                    name: "stage".to_string(),
                    backend_url: "http://127.0.0.1:19090".to_string(),
                },
                EnvironmentPreset {
                    name: "prod".to_string(),
                    backend_url: "http://127.0.0.1:29090".to_string(),
                },
            ],
            secret_source: "keyring".to_string(),
            import_path: "gui_config.import.json".to_string(),
            export_path: "gui_config.export.json".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureToggles {
    pub monitor: bool,
    pub chat: bool,
    pub skills: bool,
    pub workflow: bool,
    pub autotune: bool,
    pub security: bool,
    pub config: bool,
    pub providers: bool,
    pub workflow_run_center: bool,
    pub autotune_chain_injection: bool,
    pub skills_lifecycle: bool,
    pub providers_ops: bool,
    pub monitor_history_alerts: bool,
    pub config_safe_mode: bool,
    pub setup_enterprise: bool,
    /// Whether to show the prompt templates tab
    pub show_prompts_tab: bool,
    /// Whether to show the Risk Decision tab
    #[serde(default = "default_true")]
    pub show_risk_decision_tab: bool,
}

impl Default for FeatureToggles {
    fn default() -> Self {
        Self {
            monitor: true,
            chat: true,
            skills: true,
            workflow: true,
            autotune: true,
            security: true,
            config: true,
            providers: true,
            workflow_run_center: false,
            autotune_chain_injection: false,
            skills_lifecycle: false,
            providers_ops: true,
            monitor_history_alerts: false,
            config_safe_mode: false,
            setup_enterprise: false,
            show_prompts_tab: true,
            show_risk_decision_tab: true,
        }
    }
}

/// Provider configuration stored in GUI config file.
///
/// Strategy:
///   - System keyring is tried FIRST (no macOS prompt issue on modern keyring crate).
///   - api_key is ALSO stored in config as fallback (so key is never lost on any platform).
///   - At backend startup, keyring is checked first; if empty, config's api_key is used.
///
/// Multiple entries with the same `name` are allowed when they have different `label` values.
/// The agent is identified as `{name}_{label}` in the backend config, allowing the same
/// provider (e.g. "openai") to serve multiple models through distinct agent entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    /// API key — stored securely in system keyring, never serialized to config JSON.
    /// When deserializing from old configs, this field is loaded for migration
    /// to keyring but is cleared before saving.
    #[serde(skip)]
    pub api_key: String,
    /// Secret key — same as api_key; only stored in keyring, never in config JSON.
    #[serde(skip)]
    pub secret_key: String,
    pub model: String,
    pub validated: bool,
    /// Optional unique label to distinguish multiple entries of the same provider.
    /// When set, the backend agent name becomes `{name}_{label}`.
    /// When empty, falls back to just `{name}` (legacy behavior).
    #[serde(default)]
    pub label: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            backend_url: "http://127.0.0.1:8090".to_string(),
            language: "en".to_string(),
            theme: "简约".to_string(),
            font_scale: 1.0,
            protocol_mode: "adaptive".to_string(),
            ui_stability: UiStabilityConfig::default(),
            features: FeatureToggles::default(),
            enterprise: EnterpriseConfig::default(),
            providers: Vec::new(),
        }
    }
}

/// Load GUI app config, preferring TOML format with automatic JSON→TOML migration.
///
/// This is the **primary** loading path that delegates to `load_from_toml()`.
/// TOML is the canonical config format (`gui_config.toml`). If a JSON config
/// (`gui_config.json`) exists but no TOML config exists, it is automatically
/// migrated to TOML on load. The JSON file is preserved as a backup.
///
/// ## Post-Load Processing
///
/// 1. **Provider deduplication**: After loading, providers are deduplicated by
///    (name, label), keeping the last occurrence of each pair.
/// 2. **Keyring synchronisation**: API keys and secret keys are synced
///    bidirectionally between the config file and the OS keyring.
///    - If a key exists in config but not in keyring → write to keyring.
///    - If a key exists in keyring but not in config → copy into config.
/// 3. **Environment variable override**: `GO_ON_BACKEND_URL` can override
///    `backend_url` at load time if set.
///
/// ## Security
///
/// - API keys and secret keys are NEVER written to disk as plaintext.
///   The `save_app_config` function strips them before serialization,
///   storing them exclusively in the OS keyring.
/// - This ensures keys are never lost regardless of platform quirks.
pub fn load_app_config() -> AppConfig {
    // Delegate to load_from_toml() which handles TOML preference + JSON migration
    let mut config = load_from_toml();

    // Clamp UI stability config to sensible ranges after deserialization
    config.ui_stability.clamp_to_sensible_ranges();

    let mut changed = false;

    // Step 1: Deduplicate providers — keep last entry for each (name, label) pair.
    // Multiple entries with the same `name` are allowed when they have different `label` values.
    let mut seen = std::collections::HashSet::new();
    let mut deduped = Vec::new();
    for provider in config.providers.drain(..).rev() {
        let dedup_key = (provider.name.clone(), provider.label.clone());
        if seen.insert(dedup_key) {
            deduped.push(provider);
        }
    }
    deduped.reverse();
    config.providers = deduped;

    // Step 2: Sync keys between config and keyring (bidirectional)
    // Collect all provider names: from config.providers AND the canonical PROVIDER_NAMES list.
    let mut all_provider_names: HashSet<String> = HashSet::new();
    for p in &config.providers {
        all_provider_names.insert(p.name.clone());
    }
    for name in crate::views::providers::PROVIDER_NAMES {
        all_provider_names.insert(name.to_string());
    }
    for provider_name in &all_provider_names {
        // Find matching provider in config (or any, if name matches)
        let config_key = config
            .providers
            .iter()
            .find(|p| p.name == *provider_name)
            .map(|p| p.api_key.clone())
            .unwrap_or_default();
        let keyring_key = crate::keyring_util::get_api_key(provider_name);

        // If config has key but keyring doesn't → write to keyring
        if !config_key.is_empty() && config_key != REDACTED_API_KEY && keyring_key.is_none() {
            tracing::info!(
                "load_config: keyring missing '{}', copying from config",
                provider_name
            );
            if let Err(e) = crate::keyring_util::store_api_key(provider_name, &config_key) {
                tracing::warn!(
                    "keyring: failed to store key for '{}': {}",
                    provider_name,
                    e
                );
            }
        }

        // If keyring has key but config doesn't → write to config
        if let Some(kk) = &keyring_key {
            if !kk.is_empty() && config_key.is_empty() {
                if let Some(p) = config
                    .providers
                    .iter_mut()
                    .find(|p| p.name == *provider_name)
                {
                    p.api_key = kk.clone();
                    changed = true;
                    tracing::info!(
                        "load_config: config missing '{}', copying from keyring",
                        provider_name
                    );
                } else {
                    config.providers.push(ProviderConfig {
                        name: provider_name.to_string(),
                        api_key: kk.clone(),
                        secret_key: String::new(),
                        model: "auto".to_string(),
                        validated: true,
                        label: String::new(),
                    });
                    changed = true;
                    tracing::info!(
                        "load_config: added '{}' to config from keyring",
                        provider_name
                    );
                }
            }
        }

        // ── Secret key sync (parallel to api_key sync above) ────────────────
        let config_secret = config
            .providers
            .iter()
            .find(|p| p.name == *provider_name)
            .map(|p| p.secret_key.clone())
            .unwrap_or_default();
        let keyring_secret = crate::keyring_util::get_secret_key(provider_name);

        // If config has secret but keyring doesn't → write to keyring
        if !config_secret.is_empty()
            && config_secret != REDACTED_API_KEY
            && keyring_secret.is_none()
        {
            tracing::info!(
                "load_config: keyring missing secret_key for '{}', copying from config",
                provider_name
            );
            if let Err(e) = crate::keyring_util::store_secret_key(provider_name, &config_secret) {
                tracing::warn!(
                    "keyring: failed to store secret_key for '{}': {}",
                    provider_name,
                    e
                );
            }
        }

        // If keyring has secret but config doesn't → write to config
        if let Some(ks) = &keyring_secret {
            if !ks.is_empty() && config_secret.is_empty() {
                if let Some(p) = config
                    .providers
                    .iter_mut()
                    .find(|p| p.name == *provider_name)
                {
                    p.secret_key = ks.clone();
                    changed = true;
                    tracing::info!(
                        "load_config: config missing secret_key for '{}', copying from keyring",
                        provider_name
                    );
                } else {
                    config.providers.push(ProviderConfig {
                        name: provider_name.to_string(),
                        api_key: String::new(),
                        secret_key: ks.clone(),
                        model: "auto".to_string(),
                        validated: true,
                        label: String::new(),
                    });
                    changed = true;
                    tracing::info!(
                        "load_config: added '{}' to config from keyring (with secret_key)",
                        provider_name
                    );
                }
            }
        }
    }

    if changed {
        save_to_toml(&config);
    }

    // Flush any pending macOS Keychain ACL updates in a single batch,
    // so the user only gets prompted for their keychain password once.
    crate::keyring_util::flush_pending_acl_updates();

    // Allow env var override of backend URL
    if let Ok(env_url) = std::env::var("GO_ON_BACKEND_URL") {
        let env_url = env_url.trim().to_string();
        if !env_url.is_empty() {
            config.backend_url = env_url;
        }
    }

    config
}

/// Save GUI app config to TOML format. Delegates to `save_to_toml()`.
/// Returns true on success, false on failure.
///
/// API keys and secret keys are NEVER written to the config file.
/// They are stored exclusively in the system keyring.
/// Before serialization, the keys are cleared from the in-memory config
/// (the original AppConfig is NOT modified — a clone is used).
/// Debounce guard — prevents flushing config to disk more than once per
/// DEBOUNCE_MS window when multiple UI events trigger saves in quick succession.
/// Uses AtomicU64 (nanosecond timestamp) instead of Mutex for lock-free reads.
static CONFIG_SAVE_DEBOUNCE_NS: AtomicU64 = AtomicU64::new(0);
const CONFIG_DEBOUNCE_MS: u64 = 100;

pub fn save_app_config(config: &AppConfig) -> bool {
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let last_ns = CONFIG_SAVE_DEBOUNCE_NS.load(Ordering::Relaxed);
    if last_ns != 0 && (now_ns - last_ns) < CONFIG_DEBOUNCE_MS * 1_000_000 {
        return true; // skip — too soon since last save
    }
    CONFIG_SAVE_DEBOUNCE_NS.store(now_ns, Ordering::Relaxed);
    save_to_toml(config)
}

fn app_config_path() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
        dirs.config_dir().join("gui_config.json")
    } else {
        PathBuf::from("gui_config.json")
    }
}

/// Path for TOML-format config file (new format).
fn app_config_toml_path() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
        dirs.config_dir().join("gui_config.toml")
    } else {
        PathBuf::from("gui_config.toml")
    }
}

/// Save GUI app config to TOML format. Returns true on success, false on failure.
///
/// API keys and secret keys are NEVER written to the config file.
/// They are stored exclusively in the system keyring.
pub fn save_to_toml(config: &AppConfig) -> bool {
    let path = app_config_toml_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                "Failed to create config directory {}: {e}",
                parent.display()
            );
            return false;
        }
    }
    // Clone and redact API keys before serialization to prevent
    // plaintext secrets from being written to disk.
    let mut config_for_save = config.clone();
    for provider in &mut config_for_save.providers {
        provider.api_key.clear();
        provider.secret_key.clear();
    }
    match toml::to_string_pretty(&config_for_save) {
        Ok(content) => match std::fs::write(&path, &content) {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("Failed to write TOML config to {}: {e}", path.display());
                false
            }
        },
        Err(e) => {
            tracing::warn!("Failed to serialize config to TOML: {e}");
            false
        }
    }
}

/// Load GUI app config from TOML format (with JSON fallback for backward compatibility).
///
/// Prefers `gui_config.toml` over `gui_config.json`. When loading from the old JSON
/// format, a deprecation warning is logged and the config is automatically migrated
/// to TOML on save.
pub fn load_from_toml() -> AppConfig {
    let toml_path = app_config_toml_path();
    let json_path = app_config_path();

    // Try TOML first
    if toml_path.exists() {
        return load_app_config_inner(Some(&toml_path));
    }

    // Fall back to JSON with deprecation warning
    if json_path.exists() {
        tracing::warn!(
            "DEPRECATION WARNING: Loading config from JSON format ({}). ",
            json_path.display()
        );
        tracing::warn!(
            "The JSON format is deprecated. Config will be migrated to TOML ({}).",
            toml_path.display()
        );
        let config = load_app_config_inner(Some(&json_path));
        // Migrate to TOML on load
        save_to_toml(&config);
        return config;
    }

    // No config file exists at all — return defaults
    AppConfig::default()
}

/// Inner load: read AppConfig from an optional path. If None, reads from JSON path.
fn load_app_config_inner(path: Option<&PathBuf>) -> AppConfig {
    let path = match path {
        Some(p) => p.clone(),
        None => app_config_path(),
    };

    if !path.exists() {
        return AppConfig::default();
    }

    let content = std::fs::read_to_string(&path).unwrap_or_default();
    if content.trim().is_empty() {
        tracing::info!(
            "Config file exists at {} but is empty, using defaults",
            path.display()
        );
        return AppConfig::default();
    }

    // Try TOML first (if .toml extension), then JSON
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let config: Option<AppConfig> = if ext == "toml" {
        match toml::from_str(&content) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                tracing::warn!(
                    "Failed to parse TOML config file at {}: {e}",
                    path.display()
                );
                None
            }
        }
    } else {
        match serde_json::from_str(&content) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                tracing::warn!(
                    "Failed to parse JSON config file at {}: {e}",
                    path.display()
                );
                None
            }
        }
    };

    match config {
        Some(mut cfg) => {
            cfg.ui_stability.clamp_to_sensible_ranges();
            cfg
        }
        None => {
            tracing::info!("Attempting recovery from backup...");
            let bak_path = if ext == "toml" {
                path.with_extension("toml.bak")
            } else {
                path.with_extension("json.bak")
            };
            if let Ok(bak) = std::fs::read_to_string(&bak_path) {
                let recovered: Option<AppConfig> = if ext == "toml" {
                    toml::from_str(&bak).ok()
                } else {
                    serde_json::from_str(&bak).ok()
                };
                if let Some(cfg) = recovered {
                    tracing::info!("Recovered config from backup.");
                    let _ = std::fs::write(&path, &bak);
                    return cfg;
                } else {
                    tracing::warn!("Backup also corrupted. Starting with default config.");
                }
            } else {
                tracing::info!("No backup found. Starting with default config.");
            }
            AppConfig::default()
        }
    }
}

/// Cache for has_valid_providers: millisecond-epoch timestamp + result, atomically.
/// Avoids any Mutex contention on the UI thread.
static PROVIDERS_CACHE_TS: AtomicU64 = AtomicU64::new(0);
static PROVIDERS_CACHE_RESULT: AtomicBool = AtomicBool::new(false);

/// Check if any AI provider has a key available (in config or keyring).
/// Results are cached for 1 second to avoid per-frame keyring access.
/// Lock-free: uses atomic timestamp comparison.
pub fn has_valid_providers(config: &AppConfig) -> bool {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    // Check cache first (1 second TTL) — lock-free read
    let cached_ts = PROVIDERS_CACHE_TS.load(Ordering::Acquire);
    if cached_ts > 0 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        if now_ms.saturating_sub(cached_ts) < 1000 {
            return PROVIDERS_CACHE_RESULT.load(Ordering::Acquire);
        }
    }

    // Compute fresh result
    let result = compute_valid_providers(config);

    // Update atomics
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    PROVIDERS_CACHE_RESULT.store(result, Ordering::Release);
    PROVIDERS_CACHE_TS.store(now_ms, Ordering::Release);

    result
}

/// Core check — computes whether any provider has a valid key.
fn compute_valid_providers(config: &AppConfig) -> bool {
    // PRIMARY: check keyring for all configured providers + the canonical PROVIDER_NAMES list
    for p in &config.providers {
        if crate::keyring_util::has_api_key(&p.name.to_lowercase()) {
            return true;
        }
    }

    for name in crate::views::providers::PROVIDER_NAMES {
        if crate::keyring_util::has_api_key(name) {
            return true;
        }
    }

    // FALLBACK: if keyring didn't yield any keys (e.g. macOS blocked),
    // check config.api_key as a fallback
    for p in &config.providers {
        if !p.api_key.is_empty() && p.api_key != REDACTED_API_KEY {
            return true;
        }
    }

    // SECONDARY FALLBACK: if the config has provider entries (the user has
    // configured providers), trust that the keys are in the keyring or will
    // be resolved at runtime. This handles cases where the keyring crate's
    // lookup doesn't match entries stored by a different mechanism or version.
    if !config.providers.is_empty() {
        return true;
    }

    false
}
