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

    // 2. i18n initialization and local-skill discovery are independent, so
    //    they run concurrently to cut startup latency. The skill walk does
    //    synchronous std::fs I/O, so it runs on a blocking thread instead of
    //    stalling the async runtime.
    let lang_dir = if config.enable_i18n {
        Some(
            config
                .config_path
                .parent()
                .map(|p| p.join("languages"))
                .unwrap_or_else(|| Path::new("config/languages").to_path_buf()),
        )
    } else {
        None
    };

    let (_i18n_result, skill_result) = tokio::join!(
        async {
            if let Some(lang_dir) = lang_dir.as_deref() {
                // I18nManager::new creates the languages dir when missing, so
                // a fresh install (no <config-dir>/languages) still initializes
                // the global I18N instead of silently leaving t()/tf() on
                // raw keys.
                if let Err(e) = crate::i18n::runtime::init_i18n(lang_dir) {
                    tracing::warn!(target: "go_on::core::bootstrap", "i18n initialization failed: {e:#}");
                } else {
                    info!("I18n initialized");
                }
                // Wire the language hot-reload watcher (i18n::watcher) so edits
                // to the on-disk language files (en-US.json / zh-CN.json /
                // zh-TW.json) are picked up at runtime without a restart.
                // Best-effort: failures are logged, not fatal.
                let watcher_started = crate::i18n::watcher::start_watcher(
                    lang_dir,
                    std::time::Duration::from_secs(5),
                );
                if let Ok(true) = watcher_started {
                    info!("I18n hot-reload watcher started");
                } else if let Err(e) = watcher_started {
                    tracing::warn!("I18n hot-reload watcher failed to start: {e}");
                }
            }
        },
        async {
            tokio::task::spawn_blocking(move || {
                let mut skill_registry = crate::orchestration::skill::SkillRegistry::default();
                let discovery = skill_registry.discover_and_register_local_skills(None);
                (skill_registry, discovery)
            })
            .await
        },
    );
    // i18n_result is unit (errors already logged inside the async block).

    // 3. Consume the discovery outcome and finish skill registration.
    let mut skill_registry = match skill_result {
        Ok((registry, discovery)) => {
            match discovery {
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
            registry
        }
        Err(e) => {
            tracing::warn!(
                target: "go_on::core::bootstrap",
                "SKILL discovery task failed: {e}"
            );
            crate::orchestration::skill::SkillRegistry::default()
        }
    };

    // 4. Register built-in skills that ship with go-on (e.g. create-skill).
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
