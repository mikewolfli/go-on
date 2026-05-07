use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub backend_url: String,
    pub language: String,
    pub theme: String,
    pub features: FeatureToggles,
    pub providers: Vec<ProviderConfig>,
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
            providers: Vec::new(),
        }
    }
}

/// Load GUI app config from JSON file
pub fn load_app_config() -> AppConfig {
    let path = app_config_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        AppConfig::default()
    }
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

/// Check if any AI provider is configured and validated
pub fn has_valid_providers(config: &AppConfig) -> bool {
    config
        .providers
        .iter()
        .any(|p| p.validated && !p.api_key.is_empty())
}
