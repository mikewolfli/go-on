//! System bootstrap — initialization sequence for telemetry, i18n, cache, health.
//!
//! Called once during application startup to initialize all subsystems
//! that must be ready before configuration loading and server startup.

use anyhow::Result;
use std::path::Path;
use tracing::info;

use crate::orchestration::skill::SkillRegistry;

/// Configuration for the bootstrap process.
///
/// Used by [`perform_bootstrap`] during application startup and constructed
/// in `main/mod.rs`. Reserved for future expansion of init-time parameters.
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
///
/// Returns a `SkillRegistry` populated with locally discovered skills
/// from `~/.agents/skills/` so the caller can pass it to the server.
pub async fn perform_bootstrap(config: &BootstrapConfig) -> Result<SkillRegistry> {
    // 1. Initialize telemetry (tracing subscriber, OpenTelemetry)
    if config.enable_telemetry {
        let telemetry_cfg = crate::observability::telemetry_enhanced::TelemetryConfig {
            enable_logging: true,
            enable_tracing: true,
            enable_metrics: true,
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

    // 3. Orchestration provider trait is available for architecture boundary verification.
    //    DefaultOrchestrationProvider was a stub (always returned 0 skills) and has been removed.
    tracing::debug!(
        target: "go_on::core::bootstrap",
        "OrchestrationProvider trait available"
    );

    // 4. Initialize agent skills system — discover local SKILL.md files
    //    and set up the default prompt skill agent.
    //    The registry is returned so the server can use these discovered skills.
    let mut skill_registry = crate::orchestration::skill::SkillRegistry::default();
    match skill_registry.discover_and_register_local_skills(None) {
        Ok(summary) => {
            tracing::debug!(
                target: "go_on::core::bootstrap",
                registered = summary.registered,
                skipped = summary.skipped,
                errors = summary.errors.len(),
                "Local skills discovered"
            );
        }
        Err(e) => {
            tracing::warn!(
                target: "go_on::core::bootstrap",
                "SKILL discovery skipped: {e}"
            );
        }
    }

    info!("System bootstrap completed");
    Ok(skill_registry)
}
