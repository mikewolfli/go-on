//! Config Schema Versioning & Migration — Schema version tracking,
//! forward/backward compatibility, and automatic config migration.
//!
//! Tracks schema versions so that configuration files can be migrated
//! automatically when the application schema evolves. Supports both
//! forward compatibility (new app reads old config) and backward
//! compatibility (old app reads new config with warnings).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// SchemaVersion
// ---------------------------------------------------------------------------

/// A semantic version for the configuration schema.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SchemaVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SchemaVersion {
    pub const CURRENT: Self = Self {
        major: 1,
        minor: 0,
        patch: 0,
    };

    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Check if this version is compatible with another.
    /// Same major = compatible; different major = potentially breaking.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.major == other.major
    }
}

impl std::fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Default for SchemaVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

// ---------------------------------------------------------------------------
// MigrationStep
// ---------------------------------------------------------------------------

/// A single migration step from one config version to another.
#[derive(Debug, Clone)]
pub struct MigrationStep {
    /// Source version.
    pub from_version: SchemaVersion,
    /// Target version.
    pub to_version: SchemaVersion,
    /// Description of what this migration does.
    pub description: String,
    /// Migration function name for logging.
    pub migration_name: String,
}

// ---------------------------------------------------------------------------
// MigrationResult
// ---------------------------------------------------------------------------

/// Result of applying config migrations.
#[derive(Debug, Clone)]
pub struct MigrationResult {
    /// Original version found in config.
    pub original_version: SchemaVersion,
    /// Final version after all migrations.
    pub final_version: SchemaVersion,
    /// How many migration steps were applied.
    pub steps_applied: usize,
    /// Whether any warnings were generated.
    pub has_warnings: bool,
    /// Description of what changed.
    pub changes: Vec<String>,
}

// ---------------------------------------------------------------------------
// SchemaManager
// ---------------------------------------------------------------------------

/// Manages config schema versions, migration paths, and validation.
pub struct SchemaManager {
    /// Registered migration paths keyed by from_version.
    migrations: HashMap<String, Vec<MigrationStep>>,
    /// Current application schema version.
    current_version: SchemaVersion,
    /// Minimum supported schema version.
    min_supported_version: SchemaVersion,
}

impl SchemaManager {
    pub fn new() -> Self {
        let mut manager = Self {
            migrations: HashMap::new(),
            current_version: SchemaVersion::CURRENT,
            min_supported_version: SchemaVersion::new(1, 0, 0),
        };
        manager.register_builtin_migrations();
        manager
    }

    /// Register built-in migration paths.
    fn register_builtin_migrations(&mut self) {
        // v1.0.0: Initial schema — no migration needed
        // Future migrations would be registered here, e.g.:
        // self.register_migration(
        //     SchemaVersion::new(1, 0, 0),
        //     SchemaVersion::new(1, 1, 0),
        //     "Add scheduler concurrence config",
        //     "migrate_1_0_to_1_1",
        // );
    }

    /// Register a migration from one version to another.
    pub fn register_migration(
        &mut self,
        from: SchemaVersion,
        to: SchemaVersion,
        description: &str,
        name: &str,
    ) {
        let step = MigrationStep {
            from_version: from.clone(),
            to_version: to.clone(),
            description: description.to_string(),
            migration_name: name.to_string(),
        };
        self.migrations
            .entry(from.to_string())
            .or_default()
            .push(step);
        info!("Registered config migration: {} -> {}", from, to);
    }

    /// Validate that a config version is supported.
    pub fn validate_version(&self, version: &SchemaVersion) -> Result<(), String> {
        if *version < self.min_supported_version {
            warn!(
                "Config version {} is below minimum supported version {}",
                version, self.min_supported_version
            );
            return Err(format!(
                "Config version {} is too old. Minimum supported: {}",
                version, self.min_supported_version
            ));
        }
        if version.major > self.current_version.major {
            warn!(
                "Config version {} is newer than application version {}",
                version, self.current_version
            );
            // Not a fatal error — allow forward compatibility with warning
        }
        Ok(())
    }

    /// Find migration path from a version to the current version.
    /// Returns None if no path exists.
    pub fn find_migration_path(&self, from: &SchemaVersion) -> Option<Vec<MigrationStep>> {
        let mut path = Vec::new();
        let target = self.current_version.clone();

        // Same version is always a valid no-op migration path.
        if *from == target {
            return Some(path);
        }

        // Direct path: check if there's a single-step migration
        if let Some(steps) = self.migrations.get(&from.to_string()) {
            for step in steps {
                if step.to_version == target {
                    path.push(step.clone());
                    return Some(path);
                }
            }
        }

        // No direct path — in production, would do BFS through migration graph
        if from.major == target.major && (from.minor < target.minor || from.patch < target.patch)
        {
            // Same major version, can migrate incrementally
            info!(
                "Config version {} is within compatible range of {}",
                from, target
            );
            return Some(path); // Empty path = no migration needed (minor/patch)
        }

        None
    }

    /// Get all known schema versions.
    pub fn known_versions(&self) -> Vec<SchemaVersion> {
        let mut versions: Vec<SchemaVersion> = self
            .migrations
            .keys()
            .filter_map(|k| {
                let parts: Vec<&str> = k.trim_start_matches('v').split('.').collect();
                if parts.len() == 3 {
                    Some(SchemaVersion::new(
                        parts[0].parse().ok()?,
                        parts[1].parse().ok()?,
                        parts[2].parse().ok()?,
                    ))
                } else {
                    None
                }
            })
            .collect();
        versions.push(self.current_version.clone());
        versions.sort();
        versions.dedup();
        versions
    }

    /// Get the current application schema version.
    pub fn current_version(&self) -> &SchemaVersion {
        &self.current_version
    }
}

impl Default for SchemaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_version_display() {
        let v = SchemaVersion::new(1, 2, 3);
        assert_eq!(v.to_string(), "v1.2.3");
    }

    #[test]
    fn test_schema_version_compatible() {
        let v1 = SchemaVersion::new(1, 0, 0);
        let v2 = SchemaVersion::new(1, 5, 0);
        assert!(v1.is_compatible_with(&v2));
    }

    #[test]
    fn test_schema_version_incompatible() {
        let v1 = SchemaVersion::new(1, 0, 0);
        let v2 = SchemaVersion::new(2, 0, 0);
        assert!(!v1.is_compatible_with(&v2));
    }

    #[test]
    fn test_validate_version_too_old() {
        let manager = SchemaManager::new();
        let old = SchemaVersion::new(0, 1, 0);
        assert!(manager.validate_version(&old).is_err());
    }

    #[test]
    fn test_validate_version_current() {
        let manager = SchemaManager::new();
        assert!(manager.validate_version(&SchemaVersion::CURRENT).is_ok());
    }

    #[test]
    fn test_find_migration_path_same_version() {
        let manager = SchemaManager::new();
        let path = manager.find_migration_path(&SchemaVersion::CURRENT);
        assert!(path.is_some());
        assert!(path.unwrap().is_empty());
    }

    #[test]
    fn test_known_versions_includes_current() {
        let manager = SchemaManager::new();
        let versions = manager.known_versions();
        assert!(versions.contains(&SchemaVersion::CURRENT));
    }
}
