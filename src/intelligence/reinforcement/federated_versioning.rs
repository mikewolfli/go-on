//! GAP-B52-10: Federated Model Versioning
//!
//! Provides a versioning scheme for federated model weights, compatibility
//! checks between versions, and migration functions to convert weights
//! from one version to another.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::intelligence::reinforcement::federated::ModelWeights;

// ── ModelVersion ───────────────────────────────────────────────────────────

/// Semantic version for a federated model schema.
///
/// Follows semver semantics for model compatibility:
/// - `major`: breaking changes (incompatible with previous versions)
/// - `minor`: backward-compatible additions (new parameters added)
/// - `patch`: backward-compatible fixes (parameter semantics unchanged)
/// - `schema_hash`: SHA-256 hash of the canonical schema definition
///   (used for exact schema matching across nodes)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelVersion {
    /// Breaking-change version. Models with different major versions are
    /// incompatible and require migration.
    pub major: u32,
    /// Backward-compatible addition version.
    pub minor: u32,
    /// Backward-compatible fix version.
    pub patch: u32,
    /// SHA-256 hash of the canonical schema definition.
    /// Nodes with identical schema_hash can exchange weights directly.
    pub schema_hash: String,
}

impl ModelVersion {
    /// Create a new model version.
    pub fn new(major: u32, minor: u32, patch: u32, schema_hash: impl Into<String>) -> Self {
        Self {
            major,
            minor,
            patch,
            schema_hash: schema_hash.into(),
        }
    }

    /// Create a version with a computed schema hash from a list of
    /// canonical parameter names.
    ///
    /// This is useful for deriving a schema hash from the actual set of
    /// keys used by a model.
    pub fn with_schema(
        major: u32,
        minor: u32,
        patch: u32,
        q_table_keys: &[String],
        policy_param_keys: &[String],
    ) -> Self {
        let schema_hash = compute_schema_hash(q_table_keys, policy_param_keys);
        Self {
            major,
            minor,
            patch,
            schema_hash,
        }
    }

    /// Check whether this version is wire-compatible with another version.
    ///
    /// Compatibility rules:
    /// - Same major version → compatible (minor/patch differences are OK).
    /// - Different major versions → incompatible (migration required).
    /// - Same schema_hash → always compatible regardless of semver numbers.
    ///
    /// # Arguments
    ///
    /// * `other` - The other version to check against.
    ///
    /// # Returns
    ///
    /// `true` if the two versions are compatible for direct weight exchange.
    pub fn is_compatible_with(&self, other: &ModelVersion) -> bool {
        // Exact schema match is always compatible.
        if self.schema_hash == other.schema_hash {
            return true;
        }

        // Same major version means compatible (minor/patch differences OK).
        self.major == other.major
    }

    /// Check if this version is strictly equal to another.
    pub fn is_exact_match(&self, other: &ModelVersion) -> bool {
        self.major == other.major
            && self.minor == other.minor
            && self.patch == other.patch
            && self.schema_hash == other.schema_hash
    }

    /// Return a string representation like `"1.4.2 (sha256:abc...)"`.
    pub fn format_version_string(&self) -> String {
        let short_hash = if self.schema_hash.len() > 8 {
            &self.schema_hash[..8]
        } else {
            &self.schema_hash
        };
        format!(
            "{}.{}.{} ({})",
            self.major, self.minor, self.patch, short_hash
        )
    }
}

impl std::fmt::Display for ModelVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format_version_string())
    }
}

// ── Compute schema hash ────────────────────────────────────────────────────

/// Compute a SHA-256 schema hash from canonical lists of parameter keys.
///
/// The hash is computed over the concatenation of all Q-table keys followed
/// by all policy param keys, each prefixed with its length.
fn compute_schema_hash(q_table_keys: &[String], policy_param_keys: &[String]) -> String {
    let mut hasher = Sha256::new();

    // Hash Q-table keys.
    for key in q_table_keys.iter() {
        hasher.update(format!("q:{}:{}\n", key.len(), key));
    }

    // Hash policy param keys.
    for key in policy_param_keys.iter() {
        hasher.update(format!("p:{}:{}\n", key.len(), key));
    }

    let result = hasher.finalize();
    hex::encode(&result)
}

// ── Version grouping ───────────────────────────────────────────────────────

/// Group a list of model versions by their major version number.
///
/// Returns a map from major version to the list of versions in that group.
pub fn group_versions_by_major(versions: &[ModelVersion]) -> HashMap<u32, Vec<ModelVersion>> {
    let mut groups: HashMap<u32, Vec<ModelVersion>> = HashMap::new();
    for v in versions {
        groups.entry(v.major).or_default().push(v.clone());
    }
    groups
}

/// Group a list of model versions by their schema hash.
///
/// Versions with the same schema hash share an identical parameter set
/// and can exchange weights without migration.
pub fn group_versions_by_schema(versions: &[ModelVersion]) -> HashMap<String, Vec<ModelVersion>> {
    let mut groups: HashMap<String, Vec<ModelVersion>> = HashMap::new();
    for v in versions {
        groups
            .entry(v.schema_hash.clone())
            .or_default()
            .push(v.clone());
    }
    groups
}

/// Find the latest (highest) version in a list, ordering by
/// `major >> minor >> patch`.
pub fn latest_version(versions: &[ModelVersion]) -> Option<ModelVersion> {
    versions
        .iter()
        .max_by_key(|v| (v.major, v.minor, v.patch))
        .cloned()
}

// ── Migration registry ─────────────────────────────────────────────────────

/// A migration function that converts weights from one version to another.
///
/// The function takes source weights and returns converted weights in the
/// target version's schema.
pub type MigrationFn = fn(&ModelWeights) -> Result<ModelWeights>;

/// A pair of (from_version, to_version) identifying a migration path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MigrationPath {
    pub from_major: u32,
    pub to_major: u32,
}

impl MigrationPath {
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            from_major: from,
            to_major: to,
        }
    }
}

/// A registry of migrations between major model versions.
///
/// Migrations are one-way transformations from a source major version to
/// a target major version. The registry finds the shortest migration path
/// using BFS.
#[derive(Debug, Clone)]
pub struct MigrationRegistry {
    /// Registered migration functions, keyed by (from_major, to_major).
    migrations: HashMap<MigrationPath, MigrationFn>,
}

impl MigrationRegistry {
    /// Create an empty migration registry.
    pub fn new() -> Self {
        Self {
            migrations: HashMap::new(),
        }
    }

    /// Register a migration function from one major version to another.
    ///
    /// # Arguments
    ///
    /// * `from` - Source major version.
    /// * `to` - Target major version.
    /// * `migrate` - Function that transforms weights.
    pub fn register(&mut self, from: u32, to: u32, migrate: MigrationFn) {
        let path = MigrationPath::new(from, to);
        self.migrations.insert(path, migrate);
        debug!("MigrationRegistry: registered migration {} -> {}", from, to);
    }

    /// Check if a direct migration path exists.
    pub fn has_direct_migration(&self, from: u32, to: u32) -> bool {
        self.migrations.contains_key(&MigrationPath::new(from, to))
    }

    /// Find the shortest migration path from `from` to `to` using BFS.
    ///
    /// Returns a list of intermediate major versions to traverse,
    /// including `from` and `to`. Returns `None` if no path exists.
    pub fn find_path(&self, from: u32, to: u32) -> Option<Vec<u32>> {
        if from == to {
            return Some(vec![from]);
        }

        // Build adjacency list.
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
        for path in self.migrations.keys() {
            adj.entry(path.from_major).or_default().push(path.to_major);
        }

        // BFS.
        use std::collections::{HashMap as Hm, VecDeque};
        let mut queue: VecDeque<u32> = VecDeque::new();
        let mut parent: Hm<u32, u32> = Hm::new();

        queue.push_back(from);
        parent.insert(from, from);

        while let Some(current) = queue.pop_front() {
            if current == to {
                // Reconstruct path.
                let mut path = Vec::new();
                let mut node = to;
                while node != from {
                    path.push(node);
                    node = parent[&node];
                }
                path.push(from);
                path.reverse();
                return Some(path);
            }

            if let Some(neighbors) = adj.get(&current) {
                for &next in neighbors {
                    if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(next) {
                        e.insert(current);
                        queue.push_back(next);
                    }
                }
            }
        }

        None
    }

    /// Migrate weights from one version to another, possibly through
    /// intermediate steps.
    ///
    /// # Arguments
    ///
    /// * `weights` - The source weights.
    /// * `from` - Source `ModelVersion`.
    /// * `to` - Target `ModelVersion`.
    ///
    /// # Errors
    ///
    /// Returns an error if no migration path exists, or if any
    /// intermediate migration fails.
    pub fn migrate(
        &self,
        weights: &ModelWeights,
        from: &ModelVersion,
        to: &ModelVersion,
    ) -> Result<ModelWeights> {
        // If versions are compatible or equal, return as-is.
        if from.is_compatible_with(to) {
            return Ok(weights.clone());
        }

        let path = self
            .find_path(from.major, to.major)
            .with_context(|| format!("no migration path from v{} to v{}", from.major, to.major))?;

        let mut current = weights.clone();

        // Walk through intermediate steps.
        for window in path.windows(2) {
            let src_major = window[0];
            let dst_major = window[1];
            let migration_path = MigrationPath::new(src_major, dst_major);

            let migration_fn = self.migrations.get(&migration_path).with_context(|| {
                format!(
                    "migration function not found for {} -> {}",
                    src_major, dst_major
                )
            })?;

            current = migration_fn(&current)?;

            // Update the version on the weights.
            current.version = dst_major as u64;

            debug!(
                "migrate: applied {} -> {} (weights now at version {})",
                src_major, dst_major, current.version
            );
        }

        Ok(current)
    }
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── migrate_weights convenience function ────────────────────────────────────

/// Convenience function to migrate weights from one version to another
/// using the default migration registry.
///
/// This function creates a new `MigrationRegistry`, registers the standard
/// built-in migrations, and runs the migration chain.
///
/// Use `MigrationRegistry::migrate()` directly if you have a persistent
/// registry with custom migrations.
///
/// # Arguments
///
/// * `weights` - The source weights to migrate.
/// * `from` - Source `ModelVersion`.
/// * `to` - Target `ModelVersion`.
///
/// # Returns
///
/// The migrated `ModelWeights` in the target version's schema.
pub fn migrate_weights(
    weights: &ModelWeights,
    from: &ModelVersion,
    to: &ModelVersion,
) -> Result<ModelWeights> {
    let mut registry = MigrationRegistry::new();
    register_builtin_migrations(&mut registry);
    registry.migrate(weights, from, to)
}

// ── Built-in migrations ────────────────────────────────────────────────────

/// Register the built-in set of migration functions.
///
/// Currently empty — users should register their own migrations for
/// their specific model schemas. This function serves as an extension
/// point for future built-in migrations.
pub fn register_builtin_migrations(_registry: &mut MigrationRegistry) {
    // Built-in migrations go here as the project evolves.
    //
    // Example:
    // ```ignore
    // registry.register(1, 2, migrate_v1_to_v2);
    // registry.register(2, 3, migrate_v2_to_v3);
    // ```
    //
    // Where each migration function transforms ModelWeights from the
    // source schema to the target schema.
}

// ── Default version constants ──────────────────────────────────────────────

/// The initial model version (1.0.0 with a placeholder schema hash).
/// Nodes should replace the schema hash with one computed from their
/// actual parameter schema.
pub const VERSION_INITIAL: ModelVersion = ModelVersion {
    major: 1,
    minor: 0,
    patch: 0,
    schema_hash: String::new(),
};

// ── Helper: hex encoding (re-export from sha2 crate pattern) ─────────────────

mod hex {
    /// Encode bytes to a lowercase hex string.
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_version(major: u32, minor: u32, patch: u32, hash: &str) -> ModelVersion {
        ModelVersion::new(major, minor, patch, hash)
    }

    #[test]
    fn test_model_version_new() {
        let v = ModelVersion::new(1, 2, 3, "abcd1234");
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
        assert_eq!(v.schema_hash, "abcd1234");
    }

    #[test]
    fn test_model_version_with_schema() {
        let q_keys = vec!["s1_a1".to_string(), "s1_a2".to_string()];
        let p_keys = vec!["lr".to_string()];
        let v = ModelVersion::with_schema(1, 0, 0, &q_keys, &p_keys);
        assert_eq!(v.major, 1);
        assert!(!v.schema_hash.is_empty());
    }

    #[test]
    fn test_is_compatible_same_major() {
        let a = make_version(1, 0, 0, "hash_a");
        let b = make_version(1, 5, 2, "hash_b");
        assert!(a.is_compatible_with(&b));
        assert!(b.is_compatible_with(&a));
    }

    #[test]
    fn test_is_compatible_different_major() {
        let a = make_version(1, 0, 0, "hash_a");
        let b = make_version(2, 0, 0, "hash_b");
        assert!(!a.is_compatible_with(&b));
        assert!(!b.is_compatible_with(&a));
    }

    #[test]
    fn test_is_compatible_same_schema_hash() {
        let a = make_version(1, 0, 0, "same_hash");
        let b = make_version(2, 0, 0, "same_hash");
        // Same schema hash overrides major version incompatibility.
        assert!(a.is_compatible_with(&b));
        assert!(b.is_compatible_with(&a));
    }

    #[test]
    fn test_is_exact_match() {
        let a = make_version(1, 2, 3, "hash_x");
        let b = make_version(1, 2, 3, "hash_x");
        assert!(a.is_exact_match(&b));

        let c = make_version(1, 2, 4, "hash_x");
        assert!(!a.is_exact_match(&c));
    }

    #[test]
    fn test_compute_schema_hash_deterministic() {
        let q1 = vec!["a".to_string(), "b".to_string()];
        let p1 = vec!["x".to_string()];

        let h1 = compute_schema_hash(&q1, &p1);
        let h2 = compute_schema_hash(&q1, &p1);
        assert_eq!(h1, h2, "schema hash must be deterministic");
    }

    #[test]
    fn test_compute_schema_hash_different_keys() {
        let q1 = vec!["a".to_string(), "b".to_string()];
        let p1 = vec!["x".to_string()];

        let q2 = vec!["a".to_string(), "c".to_string()];
        let p2 = vec!["x".to_string()];

        let h1 = compute_schema_hash(&q1, &p1);
        let h2 = compute_schema_hash(&q2, &p2);
        assert_ne!(h1, h2, "different keys should produce different hashes");
    }

    #[test]
    fn test_group_versions_by_major() {
        let versions = vec![
            make_version(1, 0, 0, "h1"),
            make_version(1, 2, 0, "h2"),
            make_version(2, 0, 0, "h3"),
            make_version(2, 1, 0, "h4"),
        ];
        let groups = group_versions_by_major(&versions);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[&1].len(), 2);
        assert_eq!(groups[&2].len(), 2);
    }

    #[test]
    fn test_group_versions_by_schema() {
        let versions = vec![
            make_version(1, 0, 0, "hash_a"),
            make_version(1, 1, 0, "hash_b"),
            make_version(2, 0, 0, "hash_a"),
        ];
        let groups = group_versions_by_schema(&versions);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups["hash_a"].len(), 2);
        assert_eq!(groups["hash_b"].len(), 1);
    }

    #[test]
    fn test_latest_version() {
        let versions = vec![
            make_version(1, 0, 0, "h1"),
            make_version(1, 5, 0, "h2"),
            make_version(2, 0, 0, "h3"),
        ];
        let latest = latest_version(&versions).unwrap();
        assert_eq!(latest.major, 2);
        assert_eq!(latest.minor, 0);
    }

    #[test]
    fn test_migration_registry_register_and_find() {
        let mut registry = MigrationRegistry::new();

        fn mock_migrate(w: &ModelWeights) -> Result<ModelWeights> {
            Ok(w.clone())
        }

        registry.register(1, 2, mock_migrate);
        registry.register(2, 3, mock_migrate);

        assert!(registry.has_direct_migration(1, 2));
        assert!(!registry.has_direct_migration(1, 3));

        let path = registry.find_path(1, 3);
        assert!(path.is_some());
        assert_eq!(path.unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_migration_registry_find_path_no_path() {
        let mut registry = MigrationRegistry::new();

        fn mock_migrate(w: &ModelWeights) -> Result<ModelWeights> {
            Ok(w.clone())
        }

        registry.register(1, 2, mock_migrate);
        registry.register(3, 4, mock_migrate);

        let path = registry.find_path(1, 4);
        assert!(path.is_none());
    }

    // ── any migration tests after this ──
    #[test]
    fn test_migration_requires_path() {
        let registry = MigrationRegistry::new();
        let weights = ModelWeights {
            q_table_snapshot: HashMap::new(),
            policy_params: HashMap::new(),
            version: 1,
        };

        let from = make_version(1, 0, 0, "hash_1");
        let to = make_version(2, 0, 0, "hash_2");

        let result = registry.migrate(&weights, &from, &to);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no migration path"));
    }

    #[test]
    fn test_migration_chain() {
        let mut registry = MigrationRegistry::new();

        fn v1_to_v2(w: &ModelWeights) -> Result<ModelWeights> {
            let mut out = w.clone();
            out.q_table_snapshot.insert("v2_param".into(), 0.5);
            out.version = 2;
            Ok(out)
        }

        fn v2_to_v3(w: &ModelWeights) -> Result<ModelWeights> {
            let mut out = w.clone();
            out.policy_params.insert("v3_param".into(), 0.1);
            out.version = 3;
            Ok(out)
        }

        registry.register(1, 2, v1_to_v2);
        registry.register(2, 3, v2_to_v3);

        let weights = ModelWeights {
            q_table_snapshot: {
                let mut m = HashMap::new();
                m.insert("initial".into(), 1.0);
                m
            },
            policy_params: HashMap::new(),
            version: 1,
        };

        let from = make_version(1, 0, 0, "hash_1");
        let to = make_version(3, 0, 0, "hash_3");

        let result = registry.migrate(&weights, &from, &to).unwrap();

        assert_eq!(result.version, 3);
        assert_eq!(result.q_table_snapshot.get("initial"), Some(&1.0));
        assert_eq!(result.q_table_snapshot.get("v2_param"), Some(&0.5));
        assert_eq!(result.policy_params.get("v3_param"), Some(&0.1));
    }

    #[test]
    fn test_migrate_weights_convenience() {
        let weights = ModelWeights {
            q_table_snapshot: {
                let mut m = HashMap::new();
                m.insert("k".into(), 42.0);
                m
            },
            policy_params: HashMap::new(),
            version: 1,
        };

        let from = make_version(1, 0, 0, "hash_1");
        let to = make_version(1, 5, 0, "hash_1b");

        // Compatible versions (same major) -> direct pass-through.
        let result = migrate_weights(&weights, &from, &to).unwrap();
        assert_eq!(result.version, 1);
        assert_eq!(result.q_table_snapshot.get("k"), Some(&42.0));
    }

    #[test]
    fn test_migrate_weights_incompatible_no_migration() {
        let weights = ModelWeights {
            q_table_snapshot: HashMap::new(),
            policy_params: HashMap::new(),
            version: 1,
        };

        let from = make_version(1, 0, 0, "hash_1");
        let to = make_version(2, 0, 0, "hash_2");

        let result = migrate_weights(&weights, &from, &to);
        assert!(result.is_err());
    }

    #[test]
    fn test_model_version_serialization() {
        let v = make_version(1, 2, 3, "deadbeef");
        let json = serde_json::to_string(&v).unwrap();
        let deserialized: ModelVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(v, deserialized);
    }
}
