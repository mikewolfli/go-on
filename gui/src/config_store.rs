use crate::config::AppConfig;
use std::sync::Arc;

/// Manages application configuration loading, saving, and shared access.
/// Provides fingerprint-based change detection to avoid unnecessary syncs.
pub struct ConfigStore {
    /// Mutable application configuration
    pub config: AppConfig,
    /// Immutable snapshot shared across threads for use in async tasks
    pub config_shared: Arc<AppConfig>,
    /// Fingerprint of the last synced config; used to detect changes
    pub config_shared_fingerprint: u64,
}

impl ConfigStore {
    pub fn new(config: AppConfig) -> Self {
        let config_shared = Arc::new(config.clone());
        let config_shared_fingerprint = Self::config_fingerprint(&config);
        Self {
            config,
            config_shared,
            config_shared_fingerprint,
        }
    }

    /// Compute a fingerprint of config fields that affect rendering or backend behavior.
    /// Returns 0 if the config is empty, otherwise a hash of key fields.
    pub fn config_fingerprint(config: &AppConfig) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
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
        config.features.show_prompts_tab.hash(&mut hasher);
        config.features.show_risk_decision_tab.hash(&mut hasher);
        for provider in &config.providers {
            provider.name.hash(&mut hasher);
            provider.model.hash(&mut hasher);
            provider.validated.hash(&mut hasher);
            provider.api_key.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Sync the shared config snapshot if the mutable config has changed.
    pub fn sync_shared_if_needed(&mut self) {
        let fingerprint = Self::config_fingerprint(&self.config);
        if fingerprint != self.config_shared_fingerprint {
            self.config_shared = Arc::new(self.config.clone());
            self.config_shared_fingerprint = fingerprint;
        }
    }

    /// Get the current language code from config.
    pub fn current_lang_code(&self) -> &str {
        &self.config_shared.language
    }

    /// Get a reference to the shared config.
    pub fn shared(&self) -> &Arc<AppConfig> {
        &self.config_shared
    }
}
