use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persistent global UI state shared across all views.
/// Saved to `chat_ui_state.json` in the config directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalUiState {
    // ── Chat view ──────────────────────────────────────────────
    pub selected_mode: String,
    pub show_token_details: bool,
    pub enable_markdown: bool,
    pub show_model_picker: bool,
    pub show_prompts: bool,
    pub show_mode_row: bool,
    pub show_extra_buttons: bool,
    pub model_stats_json: Option<String>,
    /// Active session index
    pub active_session: usize,
    /// Input box draft content
    pub input_draft: String,
    /// Session search query
    pub session_search_query: String,
    /// Template editor search query
    pub template_search_query: String,

    // ── Monitor view ───────────────────────────────────────────
    /// Selected metrics time window
    pub monitor_metrics_window: String,
    /// Auto refresh interval in seconds
    pub monitor_auto_refresh_interval: u64,
    /// Provider filter text
    pub monitor_provider_filter: String,

    // ── Providers view ─────────────────────────────────────────
    /// Last selected provider in dropdown
    pub providers_selected_provider: String,
    /// Default model for new providers
    pub providers_new_model: String,
    /// Default label for new providers
    pub providers_new_label: String,

    // ── Skills view ────────────────────────────────────────────
    pub skills_show_create: bool,
    pub skills_show_import: bool,
    pub skills_selected_skill_name: String,
    pub skills_edit_desc: String,
    pub skills_edit_prompt: String,
    pub skills_edit_schema: String,
    pub skills_test_input: String,
    pub skills_rollback_version: String,
    /// Create dialog fields
    pub skills_create_name: String,
    pub skills_create_desc: String,
    pub skills_create_prompt: String,
    pub skills_create_schema: String,
    pub skills_import_url: String,

    // ── Workflow view ──────────────────────────────────────────
    pub workflow_run_status_filter: String,
    pub workflow_selected_run_id: String,
    /// Workflow step editor fields
    pub workflow_new_name: String,
    pub workflow_new_command: String,

    // ── Config editor view ─────────────────────────────────────
    pub config_editor_draft: String,
    pub config_editor_search: String,
    pub config_editor_snapshots: Vec<String>,

    // ── Setup view ─────────────────────────────────────────────
    pub setup_selected_provider: String,
    pub setup_selected_model: String,
}

impl Default for GlobalUiState {
    fn default() -> Self {
        Self {
            // Show the mode row (mode + model selectors) by default
            show_mode_row: true,
            show_extra_buttons: true,
            selected_mode: String::new(),
            show_token_details: false,
            enable_markdown: true,
            show_model_picker: false,
            show_prompts: false,
            model_stats_json: None,
            active_session: 0,
            input_draft: String::new(),
            session_search_query: String::new(),
            template_search_query: String::new(),
            monitor_metrics_window: String::new(),
            monitor_auto_refresh_interval: 0,
            monitor_provider_filter: String::new(),
            providers_selected_provider: String::new(),
            providers_new_model: String::new(),
            providers_new_label: String::new(),
            skills_show_create: false,
            skills_show_import: false,
            skills_selected_skill_name: String::new(),
            skills_edit_desc: String::new(),
            skills_edit_prompt: String::new(),
            skills_edit_schema: String::new(),
            skills_test_input: String::new(),
            skills_rollback_version: String::new(),
            skills_create_name: String::new(),
            skills_create_desc: String::new(),
            skills_create_prompt: String::new(),
            skills_create_schema: String::new(),
            skills_import_url: String::new(),
            workflow_run_status_filter: String::new(),
            workflow_selected_run_id: String::new(),
            workflow_new_name: String::new(),
            workflow_new_command: String::new(),
            config_editor_draft: String::new(),
            config_editor_search: String::new(),
            config_editor_snapshots: Vec::new(),
            setup_selected_provider: String::new(),
            setup_selected_model: String::new(),
        }
    }
}

impl GlobalUiState {
    pub fn path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
            dirs.config_dir().join("chat_ui_state.json")
        } else {
            PathBuf::from("chat_ui_state.json")
        }
    }

    pub fn load() -> Self {
        crate::fs_util::load_json_with_backup(&Self::path(), "chat UI state")
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = crate::fs_util::atomic_write(&path, &content);
        }
    }
}
