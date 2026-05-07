use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendTomlConfig {
    pub agents: Option<HashMap<String, AgentConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(rename = "type")]
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
}

/// Search for config.toml in standard locations
pub fn find_config() -> Option<PathBuf> {
    let candidates = [
        Path::new("backend/config.toml"),
        Path::new("cmd/config.toml"),
        Path::new("bin/config.toml"),
        Path::new("config.toml"),
        Path::new("../backend/config.toml"),
        Path::new("../config.toml"),
    ];
    for p in &candidates {
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// Parse backend config.toml and extract AI providers
pub fn parse_backend_config(path: &Path) -> Result<BackendTomlConfig, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    toml::from_str(&content).map_err(|e| format!("Failed to parse config: {}", e))
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
        std::fs::create_dir_all(parent).ok();
    }
    if let Ok(content) = serde_json::to_string_pretty(config) {
        std::fs::write(&path, content).ok();
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
