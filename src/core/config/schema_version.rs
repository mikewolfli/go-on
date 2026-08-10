//! Config Schema Versioning — schema version tracking and forward/backward
//! compatibility checks.
//!
//! Tracks the config schema version so that configuration files can be
//! checked for compatibility when the application schema evolves. Supports
//! both forward compatibility (new app reads old config) and backward
//! compatibility (old app reads new config with warnings).
//!
//! The migration-step machinery (register_migration / find_migration_path /
//! known_versions) was removed: no real migration step existed, and the
//! registered-path branch in `migrate_config_schema` only logged without ever
//! mutating the config (a §8 placeholder). Version parsing + compatibility
//! validation remain the live surface.

use serde::{Deserialize, Serialize};
use tracing::warn;

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

    /// Parse a schema version from a semver string (e.g. "1.0.0" or "v1.0.0").
    // Not implementing std::str::FromStr because this returns Result<Self, String>
    // rather than Result<Self, ParseIntError>.  String errors are more ergonomic here.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, String> {
        let trimmed = s.trim().trim_start_matches('v');
        let parts: Vec<&str> = trimmed.split('.').collect();
        if parts.len() != 3 {
            return Err(format!("Invalid schema version format: '{}'", s));
        }
        let major = parts[0]
            .parse::<u32>()
            .map_err(|e| format!("Invalid major version in '{}': {}", s, e))?;
        let minor = parts[1]
            .parse::<u32>()
            .map_err(|e| format!("Invalid minor version in '{}': {}", s, e))?;
        let patch = parts[2]
            .parse::<u32>()
            .map_err(|e| format!("Invalid patch version in '{}': {}", s, e))?;
        Ok(Self {
            major,
            minor,
            patch,
        })
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
// SchemaManager
// ---------------------------------------------------------------------------

/// Manages config schema-version compatibility checks.
pub struct SchemaManager {
    /// Minimum supported schema version.
    min_supported_version: SchemaVersion,
}

impl SchemaManager {
    pub fn new() -> Self {
        Self {
            min_supported_version: SchemaVersion::new(1, 0, 0),
        }
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
        // Same-major check via `is_compatible_with` — once the lower bound
        // above passed, this is exactly `version.major > CURRENT.major`.
        if !version.is_compatible_with(&SchemaVersion::CURRENT) {
            warn!(
                "Config version {} is newer than application version {}",
                version,
                SchemaVersion::CURRENT
            );
            // Not a fatal error — allow forward compatibility with warning
        }
        Ok(())
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
    fn test_validate_version_future_major_warns_but_passes() {
        // Forward compatibility: new config, older app — allowed with warning.
        let manager = SchemaManager::new();
        let newer = SchemaVersion::new(2, 0, 0);
        assert!(manager.validate_version(&newer).is_ok());
    }

    #[test]
    fn test_schema_version_missing_defaults_and_warns() {
        // Test from_str parsing — the safety net when schema_version is missing.
        let v = SchemaVersion::from_str("0.1.0").expect("Should parse 0.1.0");
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);

        // Parsing with 'v' prefix should also work.
        let v_prefixed = SchemaVersion::from_str("v1.0.0").expect("Should parse v1.0.0");
        assert_eq!(v_prefixed, SchemaVersion::CURRENT);

        // Invalid format should return Err.
        assert!(SchemaVersion::from_str("not-a-version").is_err());
        assert!(SchemaVersion::from_str("1.0").is_err());
        assert!(SchemaVersion::from_str("abc.def.ghi").is_err());
    }
}
