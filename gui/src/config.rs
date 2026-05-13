use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub backend_url: String,
    pub language: String,
    pub theme: String,
    #[serde(default)]
    pub ui_stability: UiStabilityConfig,
    pub features: FeatureToggles,
    pub enterprise: EnterpriseConfig,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiStabilityConfig {
    pub backend_refresh_interval_secs: u64,
    pub backend_ui_commit_debounce_ms: u64,
    pub health_disconnect_debounce_count: u8,
    pub chat_stream_chunk_flush_ms: u64,
    pub chat_repaint_interval_ms: u64,
    pub chat_max_pending_events_per_frame: usize,
}

impl Default for UiStabilityConfig {
    fn default() -> Self {
        Self {
            backend_refresh_interval_secs: 5,
            backend_ui_commit_debounce_ms: 120,
            health_disconnect_debounce_count: 2,
            chat_stream_chunk_flush_ms: 33,
            chat_repaint_interval_ms: 33,
            chat_max_pending_events_per_frame: 256,
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
                    backend_url: "http://127.0.0.1:8090".to_string(),
                },
                EnvironmentPreset {
                    name: "prod".to_string(),
                    backend_url: "http://127.0.0.1:8090".to_string(),
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
            providers_ops: false,
            monitor_history_alerts: false,
            config_safe_mode: false,
            setup_enterprise: false,
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
    /// API key stored directly in config (fallback when keyring unavailable).
    /// Can be empty if key is only in system keyring.
    pub api_key: String,
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
            ui_stability: UiStabilityConfig::default(),
            features: FeatureToggles::default(),
            enterprise: EnterpriseConfig::default(),
            providers: Vec::new(),
        }
    }
}

/// Load GUI app config from JSON file.
///
/// Strategy (dual storage):
///   - api_key is stored in BOTH config file AND system keyring.
///   - On load: migrate key from config → keyring if keyring is empty (fills keyring).
///   - On load: migrate key from keyring → config if config is empty (fills config).
///   - This ensures keys are never lost regardless of platform quirks.
pub fn load_app_config() -> AppConfig {
    let path = app_config_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();

    // Detect corrupted config — if file exists but parse fails, warn user
    let file_exists = path.exists();
    if file_exists && content.trim().is_empty() {
        eprintln!(
            "WARNING: Config file exists at {} but is empty, using defaults",
            path.display()
        );
    }

    let raw: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
    let mut config: AppConfig = match serde_json::from_str(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "ERROR: Failed to parse config file at {}: {e}",
                path.display()
            );
            eprintln!("Attempting recovery from backup...");
            let bak_path = path.with_extension("json.bak");
            match std::fs::read_to_string(&bak_path) {
                Ok(bak) => match serde_json::from_str(&bak) {
                    Ok(cfg) => {
                        eprintln!("Recovered config from backup.");
                        // Restore backup to main path
                        let _ = std::fs::write(&path, &bak);
                        cfg
                    }
                    Err(_) => {
                        eprintln!("Backup also corrupted. Starting with default config.");
                        AppConfig::default()
                    }
                },
                Err(_) => {
                    eprintln!("No backup found. Starting with default config.");
                    AppConfig::default()
                }
            }
        }
    };

    if file_exists
        && !content.trim().is_empty()
        && config.providers.is_empty()
        && raw.get("providers").is_none()
    {
        eprintln!(
            "WARNING: Failed to parse config file at {}, using defaults",
            path.display()
        );
    }

    let mut changed = false;

    // Step 1: If old JSON has provider data but deserialize gave empty list,
    // rebuild from raw JSON (compatibility with intermediate format that dropped api_key field).
    if config.providers.is_empty() {
        if let Some(old_providers) = raw.get("providers").and_then(|p| p.as_array()) {
            for old_p in old_providers {
                let name = match old_p.get("name").and_then(|n| n.as_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let api_key = old_p
                    .get("api_key")
                    .and_then(|k| k.as_str())
                    .unwrap_or("")
                    .to_string();
                let model = old_p
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("auto")
                    .to_string();
                let validated = old_p
                    .get("validated")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let label = old_p
                    .get("label")
                    .and_then(|l| l.as_str())
                    .unwrap_or("")
                    .to_string();
                config.providers.push(ProviderConfig {
                    name,
                    api_key,
                    model,
                    validated,
                    label,
                });
                changed = true;
            }
        }
    }

    // Step 1b: Deduplicate providers — keep last entry for each name
    let mut seen = std::collections::HashSet::new();
    let mut deduped = Vec::new();
    for provider in config.providers.drain(..).rev() {
        if seen.insert(provider.name.clone()) {
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
        if !config_key.is_empty() && config_key != "********" && keyring_key.is_none() {
            eprintln!(
                "load_config: keyring missing '{}', copying from config",
                provider_name
            );
            if let Err(e) = crate::keyring_util::store_api_key(provider_name, &config_key) {
                eprintln!(
                    "keyring: failed to store key for '{}': {}",
                    provider_name, e
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
                    eprintln!(
                        "load_config: config missing '{}', copying from keyring",
                        provider_name
                    );
                } else {
                    config.providers.push(ProviderConfig {
                        name: provider_name.to_string(),
                        api_key: kk.clone(),
                        model: "auto".to_string(),
                        validated: true,
                        label: String::new(),
                    });
                    changed = true;
                    eprintln!(
                        "load_config: added '{}' to config from keyring",
                        provider_name
                    );
                }
            }
        }
    }

    if changed {
        save_app_config(&config);
    }

    config
}

/// Save GUI app config to JSON file. Returns true on success, false on failure.
pub fn save_app_config(config: &AppConfig) -> bool {
    let path = app_config_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "Failed to create config directory {}: {e}",
                parent.display()
            );
            return false;
        }
    }
    match serde_json::to_string_pretty(config) {
        Ok(content) => match crate::fs_util::save_with_backup(&path, &content) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("Failed to write config to {}: {e}", path.display());
                false
            }
        },
        Err(e) => {
            eprintln!("Failed to serialize config: {e}");
            false
        }
    }
}

fn app_config_path() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
        dirs.config_dir().join("gui_config.json")
    } else {
        PathBuf::from("gui_config.json")
    }
}

/// Cached result of has_valid_providers, refreshed at most once per second.
static PROVIDERS_CACHE: std::sync::Mutex<Option<(std::time::Instant, bool)>> =
    std::sync::Mutex::new(None);

/// Check if any AI provider has a key available (in config or keyring).
/// Results are cached for 1 second to avoid per-frame keyring access.
pub fn has_valid_providers(config: &AppConfig) -> bool {
    // Check cache first (1 second TTL)
    if let Ok(cache) = PROVIDERS_CACHE.lock() {
        if let Some((timestamp, result)) = *cache {
            if timestamp.elapsed() < std::time::Duration::from_secs(1) {
                return result;
            }
        }
    }

    // Compute the actual result
    let result = {
        // PRIMARY: check keyring for all configured providers + the canonical PROVIDER_NAMES list
        for p in &config.providers {
            if crate::keyring_util::has_api_key(&p.name.to_lowercase()) {
                return set_and_return(true);
            }
        }

        for name in crate::views::providers::PROVIDER_NAMES {
            if crate::keyring_util::has_api_key(name) {
                return set_and_return(true);
            }
        }

        // FALLBACK: if keyring didn't yield any keys (e.g. macOS blocked),
        // check config.api_key as a fallback
        for p in &config.providers {
            if !p.api_key.is_empty() && p.api_key != "********" {
                return set_and_return(true);
            }
        }

        false
    };

    // Update cache before returning
    if let Ok(mut cache) = PROVIDERS_CACHE.lock() {
        *cache = Some((std::time::Instant::now(), result));
    }
    result
}

/// Helper to update the cache and return a value from has_valid_providers.
fn set_and_return(val: bool) -> bool {
    if let Ok(mut cache) = PROVIDERS_CACHE.lock() {
        *cache = Some((std::time::Instant::now(), val));
    }
    val
}
