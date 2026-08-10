use anyhow::Result;
use tracing::{info, warn};

use super::super::schema_version;
use super::super::types::AppConfig;

/// Checks config schema-version compatibility.
///
/// Reads the `schema_version` field from the config, validates it against the
/// current schema version, and logs the outcome. There are no registered
/// migration steps (the schema is at v1.0.0), so this never rewrites the
/// config: the original version is left untouched and observable.
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
                "Failed to parse schema_version '{}' from config: {}; skipping compatibility check",
                schema_version_str, e
            );
            return Ok(());
        }
    };

    if parsed_version == schema_version::SchemaVersion::CURRENT {
        info!(
            "Config schema version {} matches current version",
            parsed_version
        );
        return Ok(());
    }

    let manager = schema_version::SchemaManager::new();
    match manager.validate_version(&parsed_version) {
        Ok(()) => {
            info!(
                "Config schema version {} is compatible with current {}; no migration needed",
                parsed_version,
                schema_version::SchemaVersion::CURRENT
            );
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
