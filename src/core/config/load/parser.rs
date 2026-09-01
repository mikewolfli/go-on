use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::orchestration::roles::install_role_registry;

use super::super::defaults;
use super::super::types::AppConfig;
use super::migrator;
use super::patch::{self, LayeredLoad};

/// Process-wide config parse cache keyed by (path, mtime).
///
/// Display/status endpoints repeatedly parse the same TOML file (release
/// readiness alone called `AppConfig::load` up to 6 times per request, each
/// doing ~21 file syscalls + TOML parse + a global role-registry write). The
/// file mtime changes only when the file is actually rewritten (setup wizard,
/// `config.reload`), so a cached parse is always fresh for read-only loads.
/// Startup (`main/server.rs`) and explicit reload paths bypass this cache by
/// calling the uncached `load` when they need to force a re-parse.
///
/// # Known limitation (single slot)
///
/// The cache holds exactly ONE (path, mtime, config) entry. A process that
/// loads a second config path (e.g. the Hub daemon or tests switching
/// `-c` paths) invalidates the previous slot on every successful parse;
/// correctness is unaffected (mtime comparison always wins), only the hit
/// rate for multi-config workloads is reduced. Keep the single slot unless a
/// hot multi-path workload appears — a HashMap cache would need eviction.
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
        Ok(Self::load_inner_with_layers(path, None)?.config)
    }

    /// Layered config load (M1.2): the same funnel as [`Self::load`], but
    /// returns the merged config plus per-top-level-key sources and accepts an
    /// inline CLI patch layer for this invocation only (used by `go-on config
    /// dump --patch`). The mtime cache is bypassed on purpose — every dump is
    /// a fresh, patch-specific parse.
    pub fn load_layered(path: &Path, cli_patch: Option<&str>) -> Result<LayeredLoad> {
        Self::load_inner_with_layers(path, cli_patch)
    }

    fn load_inner_with_layers(path: &Path, cli_patch: Option<&str>) -> Result<LayeredLoad> {
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

        // M1.2 opt-in layered merge (builtin defaults → project file → user
        // config → CLI patch). OFF by default: when `layered_merge` is
        // absent/false and no CLI patch is given, nothing below runs and the
        // load is byte-identical to the historical single-file path.
        let mut warnings = Vec::new();
        let mut sources = Vec::new();
        let mut merged_view: Option<Value> = None;
        let layered_view: Option<String> = if cfg.layered_merge || cli_patch.is_some() {
            let mut layers = vec![patch::LayerSource {
                layer: "project",
                path: Some(path.display().to_string()),
                toml: normalized.clone(),
            }];
            if cfg.layered_merge {
                match patch::read_user_layer() {
                    Ok(Some(user_layer)) => layers.push(user_layer),
                    Ok(None) => {}
                    Err(err) => warnings.push(err),
                }
            }
            if let Some(cli) = cli_patch {
                layers.push(patch::LayerSource {
                    layer: "cli",
                    path: None,
                    toml: cli.to_string(),
                });
            }

            let layered = patch::merge_layers(patch::builtin_layer(), &layers);
            merged_view = Some(layered.merged);
            cfg = layered.config;
            sources = layered.sources;
            warnings.extend(layered.warnings);

            // Legacy-key sync + migration below run against the merged layer
            // view so keys set in any layer participate; when that view cannot
            // be re-serialized to TOML (exotic values), fall back to the
            // project file content.
            match patch::explicit_layers_toml(&layers) {
                Some(view) => Some(view),
                None => {
                    warnings.push(
                        "merged config could not be re-serialized to TOML for legacy-key sync; \
                         using the project file only"
                            .to_string(),
                    );
                    None
                }
            }
        } else {
            None
        };
        let toml_view = layered_view.unwrap_or(normalized);

        sync_legacy_flat_keys(&mut cfg, &toml_view);
        defaults::normalize_nested_phase_option_extra(&mut cfg);

        // Validate and migrate schema version BEFORE applying auto-rules
        // so that auto-rules don't reference stale phase names after migration.
        migrator::migrate_config_schema(&mut cfg, &toml_view)?;

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

        Ok(LayeredLoad {
            merged: merged_view.unwrap_or_else(|| {
                // No layering ran: fall back to the plain effective config
                // (serde defaults materialized, nulls stripped) so `config
                // dump` still prints something meaningful.
                let mut view = serde_json::to_value(&cfg).unwrap_or(Value::Null);
                patch::strip_nulls(&mut view);
                view
            }),
            config: cfg,
            sources,
            warnings,
        })
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

    // Keys written inside `[security]` / `[feature]` sections: `#[serde(flatten)]`
    // only absorbs top-level keys into the structs, so values placed inside
    // these sections parse as an unknown `security`/`feature` table and are
    // silently dropped — e.g. `[security] entry_auth_enabled = true` would
    // leave entry auth off. Treat such keys as legacy sources and read their
    // raw TOML values below, so the sections actually take effect instead of
    // being ignored.
    let mut section_values: std::collections::HashMap<&str, &toml::Value> =
        std::collections::HashMap::new();
    for section in ["security", "feature"] {
        if let Some(table) = top.get(section).and_then(|v| v.as_table()) {
            for (key, value) in table {
                section_values.insert(key.as_str(), value);
            }
        }
    }
    let section_keys: HashSet<&str> = section_values.keys().copied().collect();

    // All legacy top-level keys that mirror a [runtime] field, as one table:
    // the key list and its sync rules live together (previously the
    // `LEGACY_KEYS` presence list and the 29 per-key macro invocations were
    // maintained separately and could drift). Each rule assigns
    // `runtime.<field>` from the raw TOML value via serde round-trip — the
    // same typed conversion the per-key macro used.
    macro_rules! legacy_rules {
        ($(($key:literal, $field:ident)),* $(,)?) => {
            &[
                $(LegacyKeyRule {
                    key: $key,
                    apply: |r: &mut crate::config::RuntimeConfig, v: &toml::Value| {
                        let json = serde_json::to_value(v).unwrap_or_default();
                        if let Ok(typed) = serde_json::from_value(json) {
                            r.$field = typed;
                        }
                    },
                }),*
            ]
        };
    }
    static LEGACY_SYNC_RULES: &[LegacyKeyRule] = legacy_rules![
        // SecurityConfig -> RuntimeConfig
        ("entry_auth_enabled", entry_auth_enabled),
        ("entry_auth_api_key_env", entry_auth_api_key_env),
        ("entry_rate_limit_rpm", entry_rate_limit_rpm),
        ("entry_rate_limit_burst", entry_rate_limit_burst),
        ("user_auth_enabled", user_auth_enabled),
        ("user_auth_token_secret", user_auth_token_secret),
        ("user_auth_token_secret_env", user_auth_token_secret_env),
        ("user_auth_token_ttl_seconds", user_auth_token_ttl_seconds),
        ("request_signing_enabled", request_signing_enabled),
        ("request_signing_public_key", request_signing_public_key),
        ("request_signing_hmac_secret", request_signing_hmac_secret),
        ("mtls_enabled", mtls_enabled),
        ("mtls_ca_cert_path", mtls_ca_cert_path),
        ("mtls_server_cert_path", mtls_server_cert_path),
        ("mtls_server_key_path", mtls_server_key_path),
        ("mtls_require_client_cert", mtls_require_client_cert),
        ("mtls_allowed_cns", mtls_allowed_cns),
        // FeatureConfig -> RuntimeConfig
        ("governance_enabled", governance_enabled),
        ("governance_policy_mode", governance_policy_mode),
        ("skills_enabled", skills_enabled),
        ("skills_import_enabled", skills_import_enabled),
        ("skills_allowed_sources", skills_allowed_sources),
        ("skills_require_sha256", skills_require_sha256),
        ("skills_allow_floating_ref", skills_allow_floating_ref),
        ("skills_cache_dir", skills_cache_dir),
        ("enable_dag_execution", enable_dag_execution),
        ("enable_agent_reroute", enable_agent_reroute),
        (
            "enable_metacognitive_feedback",
            enable_metacognitive_feedback
        ),
        ("enable_delphi_debate", enable_delphi_debate),
    ];

    // When no legacy key is present (top-level or inside [security]/[feature]
    // sections), leave cfg.runtime untouched (None stays None) so the config
    // shape is preserved for None-vs-Some callers.
    if !LEGACY_SYNC_RULES
        .iter()
        .any(|rule| top.contains_key(rule.key) || section_keys.contains(rule.key))
    {
        return;
    }

    let runtime = cfg.runtime.get_or_insert_with(Default::default);

    for rule in LEGACY_SYNC_RULES {
        // Top-level legacy layout wins; otherwise take the raw TOML value from
        // the [security]/[feature] section (the flattened structs never
        // received it). An explicit [runtime] key always wins over both.
        if (top.contains_key(rule.key) || section_keys.contains(rule.key))
            && !runtime_explicit.contains(rule.key)
        {
            if let Some(value) = top
                .get(rule.key)
                .or_else(|| section_values.get(rule.key).copied())
            {
                (rule.apply)(runtime, value);
            }
        }
    }
}

/// One legacy key → `[runtime]` field sync rule (see [`sync_legacy_flat_keys`]).
struct LegacyKeyRule {
    key: &'static str,
    /// Assign the runtime field from the raw TOML value (serde round-trip,
    /// target type inferred from the field).
    apply: fn(&mut crate::config::RuntimeConfig, &toml::Value),
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
    fn section_written_legacy_keys_backfill_runtime() {
        let path = write_temp_config(
            r#"
            schema_version = "1.0.0"

            default_phase = "coding"

            [agents.copilot]
            agent_type = "copilot"

            [flow]
            name = "flow"
            phases = ["coding"]

            [phases.coding]
            agents = ["copilot"]

            [security]
            entry_auth_enabled = true
            user_auth_enabled = true

            [feature]
            governance_enabled = false
            skills_enabled = false
            "#,
        );

        let cfg = AppConfig::load_uncached(&path).expect("config should parse");
        let runtime = cfg.runtime.as_ref().expect("runtime should exist");

        // Keys written inside [security]/[feature] sections are absorbed into
        // the flattened structs and must be synced into runtime.* instead of
        // being silently ignored (the section is parseable yet would have had
        // no effect otherwise).
        assert!(runtime.entry_auth_enabled);
        assert!(runtime.user_auth_enabled);
        assert!(!runtime.governance_enabled);
        assert!(!runtime.skills_enabled);
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

    #[test]
    fn layered_load_applies_cli_patch_and_tracks_sources() {
        let path = write_temp_config(
            r#"
            layered_merge = true
            default_phase = "planning"
            [cache]
            enabled = false
            "#,
        );

        // Point the user-layer resolution at an empty temp dir so this test
        // is hermetic (a real ~/.config/go-on/config.toml would otherwise
        // participate once `layered_merge` is on).
        let _guard = crate::config::patch::USER_CONFIG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let empty_user_dir = tempfile::tempdir().expect("tempdir should be created");
        std::env::set_var("GO_ON_CONFIG_DIR", empty_user_dir.path());

        let loaded = AppConfig::load_layered(&path, Some("[cache]\nenabled = true"))
            .expect("layered load should succeed");

        std::env::remove_var("GO_ON_CONFIG_DIR");

        // The cli patch layer wins over the project file value.
        assert!(
            loaded
                .config
                .cache
                .as_ref()
                .expect("cache should exist")
                .enabled
        );
        assert_eq!(loaded.config.provider.default_phase, "planning");
        let cache_source = loaded
            .sources
            .iter()
            .find(|s| s.key == "cache")
            .expect("cache source should be tracked");
        assert_eq!(cache_source.layer, "cli");
        let phase_source = loaded
            .sources
            .iter()
            .find(|s| s.key == "default_phase")
            .expect("default_phase source should be tracked");
        assert_eq!(phase_source.layer, "project");
        assert!(
            loaded.warnings.is_empty(),
            "no warnings expected: {:?}",
            loaded.warnings
        );
    }

    #[test]
    fn runtime_load_path_applies_user_layer_when_knob_enabled() {
        // The server/exec/hot-reload path (`AppConfig::load`) must also get
        // the layered merge when the knob is on — not just `load_layered`.
        let path = write_temp_config(
            r#"
            layered_merge = true
            [cache]
            enabled = false
            "#,
        );

        let _guard = crate::config::patch::USER_CONFIG_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let user_dir = tempfile::tempdir().expect("tempdir should be created");
        let user_cfg = user_dir.path().join("config.toml");
        std::fs::write(&user_cfg, "[cache]\nenabled = true")
            .expect("user config should be written");
        std::env::set_var("GO_ON_CONFIG_DIR", user_dir.path());

        let cfg = AppConfig::load_uncached(&path).expect("load should succeed");

        std::env::remove_var("GO_ON_CONFIG_DIR");

        assert!(cfg.cache.as_ref().expect("cache should exist").enabled);
    }

    #[test]
    fn layered_load_without_knob_is_single_file_behavior() {
        // No `layered_merge` knob and no CLI patch: byte-identical to the
        // historical load — no sources, no user/cli layers.
        let path = write_temp_config(
            r#"
            default_phase = "planning"
            [cache]
            enabled = false
            "#,
        );

        let loaded = AppConfig::load_layered(&path, None).expect("load should succeed");
        assert!(loaded.sources.is_empty());
        assert!(
            !loaded
                .config
                .cache
                .as_ref()
                .expect("cache should exist")
                .enabled
        );
        assert_eq!(loaded.config.provider.default_phase, "planning");

        // And the plain load agrees exactly.
        let plain = AppConfig::load_uncached(&path).expect("load should succeed");
        assert_eq!(
            plain.provider.default_phase,
            loaded.config.provider.default_phase
        );
        assert_eq!(
            plain.cache.as_ref().expect("cache should exist").enabled,
            loaded
                .config
                .cache
                .as_ref()
                .expect("cache should exist")
                .enabled
        );
    }
}
