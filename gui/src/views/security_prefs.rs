use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPrefs {
    pub confirm_dangerous_actions: bool,
    pub redact_api_keys_in_ui: bool,
    pub block_external_urls: bool,
}

impl Default for SecurityPrefs {
    fn default() -> Self {
        Self {
            confirm_dangerous_actions: true,
            redact_api_keys_in_ui: true,
            block_external_urls: false,
        }
    }
}

pub fn state_path() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
        dirs.config_dir().join("security_state.json")
    } else {
        PathBuf::from("security_state.json")
    }
}

pub fn load() -> SecurityPrefs {
    crate::fs_util::load_json_with_backup(&state_path(), "security state")
}

pub fn save(state: &SecurityPrefs) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Failed to create security state dir: {e}");
            return;
        }
    }
    match serde_json::to_string_pretty(state) {
        Ok(content) => {
            if let Err(e) = crate::fs_util::atomic_write(&path, &content) {
                eprintln!("Failed to write security state {}: {e}", path.display());
            }
        }
        Err(e) => eprintln!("Failed to serialize security state: {e}"),
    }
}
