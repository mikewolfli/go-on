use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::orchestration::roles::install_role_registry;

use super::super::defaults;
use super::super::types::AppConfig;
use super::migrator;

/// Process-wide config parse cache keyed by (path, mtime).
///
/// Display/status endpoints repeatedly parse the same TOML file (release
/// readiness alone called `AppConfig::load` up to 6 times per request, each
/// doing ~21 file syscalls + TOML parse + a global role-registry write). The
/// file mtime changes only when the file is actually rewritten (setup wizard,
/// `config.reload`), so a cached parse is always fresh for read-only loads.
/// Startup (`main/server.rs`) and explicit reload paths bypass this cache by
/// calling the uncached `load` when they need to force a re-parse.
type ConfigCacheEntry = (PathBuf, std::time::SystemTime, Arc<AppConfig>);
static CONFIG_CACHE: OnceLock<Mutex<Option<ConfigCacheEntry>>> = OnceLock::new();

fn config_cache() -> &'static Mutex<Option<ConfigCacheEntry>> {
    CONFIG_CACHE.get_or_init(|| Mutex::new(None))
}

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
    /// Load configuration from file, with a process-wide mtime cache for
    /// read-only loads.
    ///
    /// # Arguments
    /// * `path` - Path to configuration file
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if loaded successfully, or an error if something goes wrong
    #[must_use]
    #[allow(clippy::double_must_use)]
    pub fn load(path: &Path) -> Result<Self> {
        // Fast path: return the cached parse when the file has not changed.
        let mtime = fs::metadata(path).and_then(|meta| meta.modified()).ok();
        if let Ok(cache) = config_cache().lock() {
            if let Some((cached_path, cached_mtime, cached_cfg)) = cache.as_ref() {
                if cached_path == path && mtime.is_some() && *cached_mtime == mtime.unwrap() {
                    // Arc::clone is cheap; return a deep-cloned config so callers
                    // may mutate it (auto-rules etc. never re-run on cache hits).
                    return Ok((**cached_cfg).clone());
                }
            }
        }

        let cfg = Self::load_inner(path)?;
        Self::fill_config_cache(path, mtime, &cfg);
        Ok(cfg)
    }

    /// Uncached config load — always parses the file from disk (never reads
    /// the mtime cache), so callers observe fresh file content. Used by
    /// startup and explicit reload paths that must force a re-parse.
    ///
    /// On success the fresh parse also refreshes the mtime cache, so later
    /// read-only `AppConfig::load` calls reuse it (e.g. the onboarding reload
    /// in `main/mod.rs` reuses the startup parse when the file did not
    /// change). Failed parses never touch the cache.
    pub fn load_uncached(path: &Path) -> Result<Self> {
        let mtime = fs::metadata(path).and_then(|meta| meta.modified()).ok();
        let cfg = Self::load_inner(path)?;
        Self::fill_config_cache(path, mtime, &cfg);
        Ok(cfg)
    }

    /// Store a successfully parsed config in the mtime cache, keyed by the file's
    /// current mtime. Shared by `load` and `load_uncached` so every successful
    /// parse keeps the cache fresh for subsequent read-only loads. When the mtime
    /// is unavailable (e.g. the file was just created), nothing is cached.
    fn fill_config_cache(path: &Path, mtime: Option<std::time::SystemTime>, cfg: &AppConfig) {
        if let (Ok(mut cache), Some(mtime)) = (config_cache().lock(), mtime) {
            *cache = Some((path.to_path_buf(), mtime, Arc::new(cfg.clone())));
        }
    }

    fn load_inner(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).with_context(|| {
            crate::i18n::runtime::tf(
                "error.config_read_failed",
                &[("error", &path.display().to_string())],
            )
        })?;

        let normalized = if content.trim().is_empty() {
            // Single shared helper (defaults::ensure_bootstrap_config): writes
            // the non-AI bootstrap defaults after verifying them in memory.
            defaults::ensure_bootstrap_config(path)?;
            defaults::default_non_ai_config_toml()
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
        sync_legacy_flat_keys(&mut cfg, &normalized);
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

/// Backfill `[runtime]` fields from the legacy flattened top-level keys
/// (`SecurityConfig` / `FeatureConfig`).
///
/// Production reads `runtime.*` for these settings, but config files written
/// for the pre-A7 layout place them at the top level, where `#[serde(flatten)]`
/// routes them into `security` / `feature`. Without this sync those settings
/// were silently dropped. A key that is explicitly set inside `[runtime]`
/// wins over the legacy top-level key.
fn sync_legacy_flat_keys(cfg: &mut AppConfig, normalized: &str) {
    // Parse the raw TOML once to learn which keys are explicitly present at
    // the top level and which are explicitly set inside [runtime].
    let Ok(root) = normalized.parse::<toml::Table>() else {
        return;
    };
    let top = &root;
    let runtime_explicit: HashSet<&str> = top
        .get("runtime")
        .and_then(|v| v.as_table())
        .map(|table| table.keys().map(String::as_str).collect())
        .unwrap_or_default();

    // All legacy top-level keys that mirror a [runtime] field.
    const LEGACY_KEYS: &[&str] = &[
        "entry_auth_enabled",
        "entry_auth_api_key_env",
        "entry_rate_limit_rpm",
        "entry_rate_limit_burst",
        "user_auth_enabled",
        "user_auth_token_secret",
        "user_auth_token_secret_env",
        "user_auth_token_ttl_seconds",
        "request_signing_enabled",
        "request_signing_public_key",
        "request_signing_hmac_secret",
        "mtls_enabled",
        "mtls_ca_cert_path",
        "mtls_server_cert_path",
        "mtls_server_key_path",
        "mtls_require_client_cert",
        "mtls_allowed_cns",
        "governance_enabled",
        "governance_policy_mode",
        "skills_enabled",
        "skills_import_enabled",
        "skills_allowed_sources",
        "skills_require_sha256",
        "skills_allow_floating_ref",
        "skills_cache_dir",
        "enable_dag_execution",
        "enable_agent_reroute",
        "enable_metacognitive_feedback",
        "enable_delphi_debate",
    ];

    // When no legacy key is present, leave cfg.runtime untouched (None stays
    // None) so the config shape is preserved for None-vs-Some callers.
    if !LEGACY_KEYS.iter().any(|key| top.contains_key(*key)) {
        return;
    }

    let runtime = cfg.runtime.get_or_insert_with(Default::default);

    macro_rules! sync_from_legacy {
        ($key:literal, $src:expr, $field:ident) => {
            if top.contains_key($key) && !runtime_explicit.contains($key) {
                runtime.$field = $src.$field.clone();
            }
        };
    }

    // SecurityConfig -> RuntimeConfig
    sync_from_legacy!("entry_auth_enabled", cfg.security, entry_auth_enabled);
    sync_from_legacy!(
        "entry_auth_api_key_env",
        cfg.security,
        entry_auth_api_key_env
    );
    sync_from_legacy!("entry_rate_limit_rpm", cfg.security, entry_rate_limit_rpm);
    sync_from_legacy!(
        "entry_rate_limit_burst",
        cfg.security,
        entry_rate_limit_burst
    );
    sync_from_legacy!("user_auth_enabled", cfg.security, user_auth_enabled);
    sync_from_legacy!(
        "user_auth_token_secret",
        cfg.security,
        user_auth_token_secret
    );
    sync_from_legacy!(
        "user_auth_token_secret_env",
        cfg.security,
        user_auth_token_secret_env
    );
    sync_from_legacy!(
        "user_auth_token_ttl_seconds",
        cfg.security,
        user_auth_token_ttl_seconds
    );
    sync_from_legacy!(
        "request_signing_enabled",
        cfg.security,
        request_signing_enabled
    );
    sync_from_legacy!(
        "request_signing_public_key",
        cfg.security,
        request_signing_public_key
    );
    sync_from_legacy!(
        "request_signing_hmac_secret",
        cfg.security,
        request_signing_hmac_secret
    );
    sync_from_legacy!("mtls_enabled", cfg.security, mtls_enabled);
    sync_from_legacy!("mtls_ca_cert_path", cfg.security, mtls_ca_cert_path);
    sync_from_legacy!("mtls_server_cert_path", cfg.security, mtls_server_cert_path);
    sync_from_legacy!("mtls_server_key_path", cfg.security, mtls_server_key_path);
    sync_from_legacy!(
        "mtls_require_client_cert",
        cfg.security,
        mtls_require_client_cert
    );
    sync_from_legacy!("mtls_allowed_cns", cfg.security, mtls_allowed_cns);

    // FeatureConfig -> RuntimeConfig
    sync_from_legacy!("governance_enabled", cfg.feature, governance_enabled);
    sync_from_legacy!(
        "governance_policy_mode",
        cfg.feature,
        governance_policy_mode
    );
    sync_from_legacy!("skills_enabled", cfg.feature, skills_enabled);
    sync_from_legacy!("skills_import_enabled", cfg.feature, skills_import_enabled);
    sync_from_legacy!(
        "skills_allowed_sources",
        cfg.feature,
        skills_allowed_sources
    );
    sync_from_legacy!("skills_require_sha256", cfg.feature, skills_require_sha256);
    sync_from_legacy!(
        "skills_allow_floating_ref",
        cfg.feature,
        skills_allow_floating_ref
    );
    sync_from_legacy!("skills_cache_dir", cfg.feature, skills_cache_dir);
    sync_from_legacy!("enable_dag_execution", cfg.feature, enable_dag_execution);
    sync_from_legacy!("enable_agent_reroute", cfg.feature, enable_agent_reroute);
    sync_from_legacy!(
        "enable_metacognitive_feedback",
        cfg.feature,
        enable_metacognitive_feedback
    );
    sync_from_legacy!("enable_delphi_debate", cfg.feature, enable_delphi_debate);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_config(content: &str) -> std::path::PathBuf {
        let mut file = tempfile::NamedTempFile::new().expect("tempfile should be created");
        std::io::Write::write_all(&mut file, content.as_bytes())
            .expect("config content should be written");
        let (_, path) = file.keep().expect("temp config file should be retained");
        path
    }

    #[test]
    fn legacy_top_level_keys_backfill_runtime() {
        let path = write_temp_config(
            r#"
            schema_version = "1.0.0"
            entry_auth_enabled = true
            user_auth_enabled = true
            request_signing_enabled = true
            mtls_enabled = true
            governance_enabled = false
            skills_enabled = false
            enable_dag_execution = true
            entry_rate_limit_rpm = 999
            mtls_allowed_cns = "client-a"

            default_phase = "coding"

            [agents.copilot]
            agent_type = "copilot"

            [flow]
            name = "flow"
            phases = ["coding"]

            [phases.coding]
            agents = ["copilot"]
            "#,
        );

        let cfg = AppConfig::load_uncached(&path).expect("config should parse");
        let runtime = cfg.runtime.as_ref().expect("runtime should exist");

        // Old-format top-level keys must take effect via runtime.*.
        assert!(runtime.entry_auth_enabled);
        assert!(runtime.user_auth_enabled);
        assert!(runtime.request_signing_enabled);
        assert!(runtime.mtls_enabled);
        assert!(!runtime.governance_enabled);
        assert!(!runtime.skills_enabled);
        assert!(runtime.enable_dag_execution);
        assert_eq!(runtime.entry_rate_limit_rpm, 999);
        assert_eq!(runtime.mtls_allowed_cns, "client-a");
    }

    #[test]
    fn runtime_explicit_keys_take_precedence_over_legacy() {
        let path = write_temp_config(
            r#"
            schema_version = "1.0.0"
            entry_auth_enabled = true

            default_phase = "coding"

            [agents.copilot]
            agent_type = "copilot"

            [flow]
            name = "flow"
            phases = ["coding"]

            [phases.coding]
            agents = ["copilot"]

            [runtime]
            entry_auth_enabled = false
            "#,
        );

        let cfg = AppConfig::load_uncached(&path).expect("config should parse");
        let runtime = cfg.runtime.as_ref().expect("runtime should exist");
        // The explicit [runtime] value wins over the legacy top-level key.
        assert!(!runtime.entry_auth_enabled);
    }

    #[test]
    fn absent_legacy_keys_leave_runtime_defaults_untouched() {
        let path = write_temp_config(
            r#"
            schema_version = "1.0.0"
            entry_auth_enabled = true

            default_phase = "coding"

            [agents.copilot]
            agent_type = "copilot"

            [flow]
            name = "flow"
            phases = ["coding"]

            [phases.coding]
            agents = ["copilot"]

            [runtime]
            maintenance_interval_seconds = 120
            "#,
        );

        let cfg = AppConfig::load_uncached(&path).expect("config should parse");
        let runtime = cfg.runtime.as_ref().expect("runtime should exist");
        // The present legacy key is synced...
        assert!(runtime.entry_auth_enabled);
        // ...while absent legacy keys keep their runtime defaults untouched
        // (no corruption from the flattened security/feature defaults).
        assert!(!runtime.user_auth_enabled);
        assert_eq!(
            runtime.user_auth_token_secret,
            crate::core::config::defaults::default_runtime_user_auth_token_secret()
        );
        assert_eq!(runtime.maintenance_interval_seconds, 120);
    }
}
