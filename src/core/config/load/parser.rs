use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use crate::orchestration::roles::install_role_registry;

use super::super::defaults;
use super::super::types::AppConfig;
use super::migrator;

/// Configuration warning severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigWarningSeverity {
    /// Informational warning
    Info,
    /// Warning that may affect functionality
    Warn,
    /// Critical issue that will prevent proper operation
    Critical,
}

/// Configuration warning structure
#[derive(Debug, Clone, Serialize)]
pub struct ConfigWarning {
    /// Warning code
    pub code: String,
    /// Warning severity
    pub severity: ConfigWarningSeverity,
    /// Warning message
    pub message: String,
}

/// Configuration health report
#[derive(Debug, Clone, Serialize)]
pub struct ConfigHealthReport {
    /// Health score (0-100)
    pub score: u32,
    /// Total number of warnings
    pub total: usize,
    /// Number of informational warnings
    pub info_count: usize,
    /// Number of warnings
    pub warn_count: usize,
    /// Number of critical warnings
    pub critical_count: usize,
    /// Recommended profile based on current warning/risk posture
    pub profile_recommendation: String,
    /// Actionable recommendations for improving configuration quality
    pub recommendations: Vec<String>,
    /// List of warnings
    pub warnings: Vec<ConfigWarning>,
}

impl ConfigHealthReport {
    /// Get all warning messages
    ///
    /// # Returns
    /// * `Vec<String>` - List of warning messages
    pub fn warning_messages(&self) -> Vec<String> {
        self.warnings
            .iter()
            .map(|item| item.message.clone())
            .collect()
    }
}

impl AppConfig {
    /// Load configuration from file
    ///
    /// # Arguments
    /// * `path` - Path to configuration file
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if loaded successfully, or an error if something goes wrong
    #[must_use]
    #[allow(clippy::double_must_use)]
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).with_context(|| {
            crate::i18n::runtime::tf(
                "error.config_read_failed",
                &[("error", &path.display().to_string())],
            )
        })?;

        let normalized = if content.trim().is_empty() {
            let bootstrap = defaults::default_non_ai_config_toml();
            // Verify the bootstrap defaults parse correctly in memory before writing to disk
            let _parsed: AppConfig = toml::from_str(&bootstrap).map_err(|e| {
                anyhow::anyhow!(
                    "{}: {}",
                    crate::i18n::runtime::tf(
                        "error.config_parse_failed",
                        &[("error", &path.display().to_string())],
                    ),
                    e,
                )
            })?;
            fs::write(path, &bootstrap).with_context(|| {
                format!(
                    "failed to write bootstrap defaults to blank config: {}",
                    path.display()
                )
            })?;
            info!(
                "blank config detected; wrote non-AI bootstrap defaults to {}",
                path.display()
            );
            bootstrap
        } else {
            content
        };

        let mut cfg: AppConfig = toml::from_str(&normalized).map_err(|e| {
            anyhow::anyhow!(
                "{}: {}",
                crate::i18n::runtime::tf(
                    "error.config_parse_failed",
                    &[("error", &path.display().to_string())],
                ),
                e,
            )
        })?;
        defaults::normalize_nested_phase_option_extra(&mut cfg);

        // Validate and migrate schema version BEFORE applying auto-rules
        // so that auto-rules don't reference stale phase names after migration.
        migrator::migrate_config_schema(&mut cfg, &normalized)?;

        // Apply auto-rules AFTER migration to ensure phase structure is final.
        defaults::apply_auto_rules(path, &mut cfg);

        // Sync `[protocol].mode` → `runtime.protocol_mode` so that
        // the protocol.mode TOML key is available without re-reading
        // the raw file (previously re-read in server.rs).
        if let Some(ref protocol) = cfg.protocol {
            if let Some(ref mode) = protocol.mode {
                let rt = cfg.runtime.get_or_insert_with(Default::default);
                rt.protocol_mode.get_or_insert_with(|| mode.clone());
            }
        }

        if !cfg.role_registry().is_empty() {
            install_role_registry(cfg.role_registry().clone());
        }

        Ok(cfg)
    }
}
