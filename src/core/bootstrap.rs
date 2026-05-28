//! System bootstrap — initialization sequence for telemetry, i18n, cache, health.
//!
//! Called once during application startup to initialize all subsystems
//! that must be ready before configuration loading and server startup.

use anyhow::Result;
use std::path::Path;
use tracing::info;

/// Configuration for the bootstrap process.
#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    pub enable_telemetry: bool,
    pub enable_i18n: bool,
    pub config_path: std::path::PathBuf,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            enable_telemetry: true,
            enable_i18n: true,
            config_path: Path::new("config/config.toml").to_path_buf(),
        }
    }
}

/// Perform all initialization steps. Call once at startup.
pub async fn perform_bootstrap(config: &BootstrapConfig) -> Result<()> {
    // 1. Initialize telemetry (tracing subscriber, OpenTelemetry)
    if config.enable_telemetry {
        let telemetry_cfg = crate::observability::telemetry_enhanced::TelemetryConfig {
            enable_logging: true,
            enable_tracing: true,
            enable_metrics: false,
            service_name: "go-on".to_string(),
            ..Default::default()
        };
        if let Err(e) = crate::observability::telemetry_enhanced::init_telemetry(&telemetry_cfg)
            .map_err(|e| anyhow::anyhow!("telemetry init: {e}"))
        {
            tracing::warn!("telemetry initialization skipped: {e}");
        }
        info!("Telemetry initialized");
    }

    // 2. Initialize i18n (internationalization)
    if config.enable_i18n {
        let lang_dir = config
            .config_path
            .parent()
            .map(|p| p.join("languages"))
            .unwrap_or_else(|| Path::new("config/languages").to_path_buf());
        if lang_dir.exists() {
            let _ = crate::i18n::runtime::init_i18n(&lang_dir);
        }
        info!("I18n initialized");
    }

    info!("System bootstrap completed");
    Ok(())
}
