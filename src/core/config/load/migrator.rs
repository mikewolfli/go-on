use anyhow::Result;
use tracing::{info, warn};

use super::super::schema_version;
use super::super::types::AppConfig;

/// Applies schema version migration to a parsed config.
///
/// Reads the `schema_version` field from the config, validates it against
/// the current schema version, and applies any migration steps if needed.
/// Returns `Ok(())` if migration was successful or unnecessary.
pub(crate) fn migrate_config_schema(cfg: &mut AppConfig, normalized: &str) -> Result<()> {
    let schema_version_str = if normalized.contains("schema_version") {
        cfg.schema_version.clone()
    } else {
        warn!(
            "Config file does not contain a schema_version field; defaulting to \"0.1.0\" for migration"
        );
        "0.1.0".to_string()
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
                    Some(steps) => {
                        if steps.is_empty() {
                            info!(
                                "Config schema version {} is compatible with current {}; no migration needed",
                                parsed_version,
                                schema_version::SchemaVersion::CURRENT
                            );
                        } else {
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
                        }
                        cfg.schema_version = schema_version::SchemaVersion::CURRENT.to_string();
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
