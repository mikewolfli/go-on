use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub backend_url: String,
    pub language: String,
    pub theme: String,
    pub features: FeatureToggles,
    pub enterprise: EnterpriseConfig,
    pub providers: Vec<ProviderConfig>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub api_key: String,
    pub model: String,
    pub validated: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            backend_url: "http://127.0.0.1:8090".to_string(),
            language: "en".to_string(),
            theme: "简约".to_string(),
            features: FeatureToggles::default(),
            enterprise: EnterpriseConfig::default(),
            providers: Vec::new(),
        }
    }
}

/// Load GUI app config from JSON file and auto-migrate keyring providers
pub fn load_app_config() -> AppConfig {
    let path = app_config_path();
    let mut config = if let Ok(content) = std::fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        AppConfig::default()
    };

    // Auto-migrate: detect providers in keyring but not in config
    let mut changed = false;
    for provider_name in [
        "deepseek",
        "openai",
        "anthropic",
        "qwen",
        "gemini",
        "groq",
        "mistral",
    ] {
        if config.providers.iter().any(|p| p.name == provider_name) {
            continue;
        }
        if let Some(key) = crate::keyring_util::get_api_key(provider_name) {
            eprintln!(
                "Auto-migrating '{}' from keyring to gui_config.json",
                provider_name
            );
            config.providers.push(ProviderConfig {
                name: provider_name.to_string(),
                api_key: key,
                model: "auto".to_string(),
                validated: true,
            });
            changed = true;
        }
    }

    if changed {
        save_app_config(&config);
    }

    config
}

/// Save GUI app config to JSON file
pub fn save_app_config(config: &AppConfig) {
    let path = app_config_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "Failed to create config directory {}: {e}",
                parent.display()
            );
            return;
        }
    }
    match serde_json::to_string_pretty(config) {
        Ok(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                eprintln!("Failed to write config to {}: {e}", path.display());
            }
        }
        Err(e) => {
            eprintln!("Failed to serialize config: {e}");
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

/// Check if any AI provider is configured, validated, or exists in keyring
pub fn has_valid_providers(config: &AppConfig) -> bool {
    // Check local config first
    if config
        .providers
        .iter()
        .any(|p| p.validated && !p.api_key.is_empty())
    {
        return true;
    }
    // Also check if any known provider has a key in the system keyring
    // This handles the case where key was set via CLI (--secret set) or backend
    let known_providers = [
        "deepseek",
        "openai",
        "anthropic",
        "qwen",
        "gemini",
        "groq",
        "mistral",
        "copilot",
    ];
    for name in &known_providers {
        if let Some(key) = crate::keyring_util::get_api_key(name) {
            if !key.is_empty() {
                return true;
            }
        }
    }
    false
}
