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
    pub enable_i18n: bool,
    pub config_path: std::path::PathBuf,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
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
    // 1. Telemetry is initialized earlier in main/mod.rs run() so startup
    //    logs are captured; OTLP export is wired after config load. No
    //    telemetry step here (the previous enable_telemetry branch was an
    //    unreachable duplicate of that init — see log-20260809-3).

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
        // Wire the language hot-reload watcher (i18n::watcher) so edits to the
        // on-disk language files (en-US.json / zh-CN.json / zh-TW.json) are
        // picked up at runtime without a restart. Best-effort: failures are
        // logged, not fatal.
        let watcher_started =
            crate::i18n::watcher::start_watcher(&lang_dir, std::time::Duration::from_secs(5));
        if let Ok(true) = watcher_started {
            info!("I18n hot-reload watcher started");
        } else if let Err(e) = watcher_started {
            tracing::warn!("I18n hot-reload watcher failed to start: {e}");
        }
    }

    // 3. Orchestration provider trait is available for architecture boundary verification.
    //    DefaultOrchestrationProvider was a stub (always returned 0 skills) and has been removed.
    tracing::debug!(
        target: "go_on::core::bootstrap",
        "OrchestrationProvider trait available"
    );

    // 4. (removed) intermediate-file dir init — the `.goon/intermediates/`
    //    feature was dormant: create_task_intermediate_dir ran per request but
    //    nothing consumed or cleaned the directories (log-20260730-18).

    // 5. Initialize agent skills system — discover local SKILL.md files
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

    // 6. Register built-in skills that ship with go-on (e.g. create-skill).
    //    Done after local discovery so that a locally discovered skill with the
    //    same name takes precedence (built-in registration skips existing names).
    if let Err(e) = skill_registry.register_builtin_skills() {
        tracing::warn!(
            target: "go_on::core::bootstrap",
            "Built-in skill registration failed: {e}"
        );
    }

    info!("System bootstrap completed");
    Ok(skill_registry)
}
