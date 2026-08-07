use anyhow::Result;
use tracing::{info, warn};

use super::super::schema_version;
use super::super::types::AppConfig;

/// Checks config schema-version compatibility.
///
/// Reads the `schema_version` field from the config, validates it against the
/// current schema version, and — if real migration steps exist — applies them.
///
/// Honesty contract (principle #13/#15): this function never fabricates a
/// migration. When no registered migration path exists, the config keeps its
/// original `schema_version` and a warning is emitted, instead of silently
/// stamping the config as "already migrated".
pub(crate) fn migrate_config_schema(cfg: &mut AppConfig, normalized: &str) -> Result<()> {
    let schema_version_str = if normalized.contains("schema_version") {
        cfg.schema_version.clone()
    } else {
        info!(
            "Config file does not contain a schema_version field; defaulting to \"1.0.0\" for compatibility check"
        );
        "1.0.0".to_string()
    };

    let parsed_version = match schema_version::SchemaVersion::from_str(&schema_version_str) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "Failed to parse schema_version '{}' from config: {}; skipping migration",
                schema_version_str, e
            );
            return Ok(());
        }
    };

    let manager = schema_version::SchemaManager::new();
    match manager.validate_version(&parsed_version) {
        Ok(()) => {
            if parsed_version != schema_version::SchemaVersion::CURRENT {
                match manager.find_migration_path(&parsed_version) {
                    Some(steps) if steps.is_empty() => {
                        info!(
                            "Config schema version {} is compatible with current {}; no migration needed",
                            parsed_version,
                            schema_version::SchemaVersion::CURRENT
                        );
                        // Compatible minor/patch differences are tolerated; keep
                        // the original version so the real state is observable.
                    }
                    Some(steps) => {
                        info!(
                            "Applying {} config migration step(s) from {} to {}",
                            steps.len(),
                            parsed_version,
                            schema_version::SchemaVersion::CURRENT
                        );
                        for step in &steps {
                            info!(
                                "  Migration: {} -> {}: {}",
                                step.from_version, step.to_version, step.description
                            );
                        }
                        // No migration apply hook is registered yet (see
                        // SchemaManager::register_migration). When real steps
                        // exist, each step must mutate `cfg`; until then the
                        // version is left untouched rather than falsely stamped.
                        warn!(
                            "Registered migration steps have no apply implementation yet; \
                             config version left as {}",
                            parsed_version
                        );
                    }
                    None => {
                        warn!(
                            "No migration path found from {} to {}; config may be incompatible",
                            parsed_version,
                            schema_version::SchemaVersion::CURRENT
                        );
                    }
                }
            } else {
                info!(
                    "Config schema version {} matches current version",
                    parsed_version
                );
            }
        }
        Err(msg) => {
            warn!(
                "Config schema version validation failed: {}; attempting to load anyway",
                msg
            );
        }
    }

    Ok(())
}
