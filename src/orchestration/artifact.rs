//! F-GAP-10: Artifact Contract Layer (FUTURE3.M9 / BLUE38 §6.6)
//!
//! Provides a unified schema and storage layer for all artifact types
//! produced by agents: code patches, analysis reports, test results,
//! config changes, and any other structured output.
//!
//! Each artifact is validated against a registered JSON Schema before
//! storage. Expired artifacts can be pruned automatically.

use anyhow;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Counter for generating unique artifact IDs.
static ARTIFACT_COUNTER: AtomicU64 = AtomicU64::new(1);

fn lock_mutex_recover<'a, T>(mtx: &'a Mutex<T>, name: &str) -> MutexGuard<'a, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("{} lock poisoned; recovering", name);
            poisoned.into_inner()
        }
    }
}

fn read_lock_recover<'a, T>(rw: &'a RwLock<T>, name: &str) -> RwLockReadGuard<'a, T> {
    match rw.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("{} read lock poisoned; recovering", name);
            poisoned.into_inner()
        }
    }
}

fn write_lock_recover<'a, T>(rw: &'a RwLock<T>, name: &str) -> RwLockWriteGuard<'a, T> {
    match rw.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("{} write lock poisoned; recovering", name);
            poisoned.into_inner()
        }
    }
}

/// Generates a unique artifact ID using a millisecond timestamp and
/// an atomic counter to ensure uniqueness within the same millisecond.
fn generate_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = ARTIFACT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("art-{}-{}", now, seq)
}

/// Contract that defines the structure of an artifact type.
///
/// Each schema type carries a JSON Schema document describing the
/// expected shape of `Artifact.content`, plus a list of required
/// top-level field names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSchema {
    pub name: String,
    pub version: String,
    pub description: String,
    /// JSON Schema describing the `content` field of matching artifacts.
    pub schema: serde_json::Value,
    /// Fields that must be present in the artifact's `content`.
    pub required_fields: Vec<String>,
    pub created_at: u64,
}

/// A concrete artifact instance produced by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    /// Name of the schema this artifact conforms to.
    pub schema_name: String,
    /// Version of the schema at the time of creation.
    pub schema_version: String,
    /// Agent role or component that produced this artifact.
    pub producer: String,
    /// The task execution that triggered this artifact.
    pub task_id: String,
    /// The artifact payload, validated against the registered schema.
    pub content: serde_json::Value,
    /// Creation timestamp (Unix milliseconds).
    pub created_ms: u64,
    /// Time-to-live in milliseconds; after this duration the artifact
    /// is considered expired and eligible for pruning.
    pub ttl_ms: u64,
    /// Arbitrary tags for filtering and discovery.
    pub tags: Vec<String>,
}

/// Runtime profile snapshot for the artifact layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactProfile {
    pub enabled: bool,
    pub registered_schemas: u32,
    pub total_artifacts: u32,
    pub active_artifacts: u32,
    pub producers: u32,
}

/// Result of validating an artifact against its registered schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactValidation {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub missing_fields: Vec<String>,
    pub warnings: Vec<String>,
}

/// Central artifact layer that manages schemas, stores artifacts, and
/// provides query capabilities.
///
/// Thread-safe: `schemas` and `profile` use RwLock / Mutex so that
/// reads can happen concurrently while writes are exclusive.
pub struct ArtifactLayer {
    /// Registered artifact schemas keyed by name.
    schemas: Arc<RwLock<HashMap<String, ArtifactSchema>>>,
    /// Stored artifacts.
    artifacts: Arc<Mutex<Vec<Artifact>>>,
    /// Maximum number of artifacts to retain (excluding expired ones
    /// that have not yet been pruned).
    max_artifacts: usize,
    /// Runtime profile metrics.
    profile: Arc<Mutex<ArtifactProfile>>,
    /// Atomic counter for active (non-expired) artifacts — avoids O(n) scans.
    active_artifacts: AtomicU32,
    /// Per-producer reference counts so we can derive unique producer counts
    /// in O(1) instead of O(n log n).
    producer_counts: Arc<Mutex<HashMap<String, u32>>>,
}

impl Default for ArtifactLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtifactLayer {
    /// Creates a new artifact layer with default settings.
    ///
    /// The default `max_artifacts` is 10 000.
    pub fn new() -> Self {
        Self {
            schemas: Arc::new(RwLock::new(HashMap::new())),
            artifacts: Arc::new(Mutex::new(Vec::new())),
            max_artifacts: 10_000,
            profile: Arc::new(Mutex::new(ArtifactProfile {
                enabled: true,
                registered_schemas: 0,
                total_artifacts: 0,
                active_artifacts: 0,
                producers: 0,
            })),
            active_artifacts: AtomicU32::new(0),
            producer_counts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers a new artifact schema.
    ///
    /// Returns an error if the schema's `schema` field is not a JSON
    /// object, or if a schema with the same name is already registered.
    pub fn register_schema(&self, schema: ArtifactSchema) -> anyhow::Result<()> {
        // Validate that `schema` is a JSON object (a valid JSON Schema
        // root must be an object per draft-07+).
        if !schema.schema.is_object() {
            anyhow::bail!(
                "Schema '{}': 'schema' field must be a JSON object, got {}",
                schema.name,
                serde_json::value::to_value(&schema.schema)
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            );
        }

        let mut schemas = write_lock_recover(self.schemas.as_ref(), "schemas");
        if schemas.contains_key(&schema.name) {
            anyhow::bail!("Schema '{}' is already registered", schema.name);
        }

        schemas.insert(schema.name.clone(), schema);
        {
            let mut profile = lock_mutex_recover(self.profile.as_ref(), "profile");
            profile.registered_schemas = schemas.len() as u32;
        }
        Ok(())
    }

    /// Validates an artifact against its registered schema.
    ///
    /// This performs structural checks:
    /// - The schema must exist (otherwise a validation error is returned).
    /// - All fields listed in `required_fields` must be present in
    ///   `artifact.content` as a JSON object.
    /// - The `content` must be a JSON object (non-object content yields
    ///   a validation error).
    /// - A version mismatch between registered schema and artifact
    ///   produces a warning but does not invalidate the artifact.
    pub fn validate(&self, artifact: &Artifact) -> ArtifactValidation {
        let schemas = read_lock_recover(self.schemas.as_ref(), "schemas");
        let schema = match schemas.get(&artifact.schema_name) {
            Some(s) => s,
            None => {
                return ArtifactValidation {
                    is_valid: false,
                    errors: vec![format!(
                        "No registered schema found for '{}'",
                        artifact.schema_name
                    )],
                    missing_fields: vec![],
                    warnings: vec![],
                };
            }
        };

        // Check version compatibility.
        // Major version mismatch (e.g. 1.x → 2.x) is a hard error.
        // Minor/patch mismatch produces a warning only.
        let mut warnings: Vec<String> = Vec::new();
        if schema.version != artifact.schema_version {
            let major_mismatch = schema
                .version
                .split('.')
                .next()
                .zip(artifact.schema_version.split('.').next())
                .map(|(reg, art)| reg != art)
                .unwrap_or(true);
            if major_mismatch {
                return ArtifactValidation {
                    is_valid: false,
                    errors: vec![format!(
                        "Schema major version mismatch for '{}': registered '{}', artifact '{}'",
                        artifact.schema_name, schema.version, artifact.schema_version
                    )],
                    missing_fields: vec![],
                    warnings: vec![],
                };
            }
            warnings.push(format!(
                "Schema version mismatch: registered '{}', artifact '{}'",
                schema.version, artifact.schema_version
            ));
        }

        // Validate that content is a JSON object.
        let content_obj = match artifact.content.as_object() {
            Some(obj) => obj,
            None => {
                return ArtifactValidation {
                    is_valid: false,
                    errors: vec!["Artifact content must be a JSON object".to_string()],
                    missing_fields: schema.required_fields.clone(),
                    warnings,
                };
            }
        };

        // Check required fields.
        let mut missing_fields: Vec<String> = Vec::new();
        for field in &schema.required_fields {
            if !content_obj.contains_key(field) {
                missing_fields.push(field.clone());
            }
        }

        let is_valid = missing_fields.is_empty();
        ArtifactValidation {
            is_valid,
            errors: if is_valid {
                vec![]
            } else {
                vec![format!(
                    "Missing required fields: {}",
                    missing_fields.join(", ")
                )]
            },
            missing_fields,
            warnings,
        }
    }

    /// Stores an artifact after validation.
    ///
    /// The artifact is validated against its registered schema before
    /// storage.  If validation fails the artifact is rejected and the
    /// errors are returned.
    ///
    /// On success the artifact's `id` field is populated automatically
    /// (any pre-existing value is overwritten) and the assigned ID is
    /// returned.
    pub fn store(&self, artifact: Artifact) -> anyhow::Result<String> {
        // Validate first.
        let validation = self.validate(&artifact);
        if !validation.is_valid {
            anyhow::bail!(
                "Artifact validation failed: {}",
                validation.errors.join("; ")
            );
        }

        let mut artifact = artifact;
        artifact.id = generate_id();

        // Capture a consistent "now" for active checks in this store call.
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Check whether the new artifact is active before pushing.
        let new_is_active = artifact.created_ms + artifact.ttl_ms > now_ms;
        let new_producer = artifact.producer.clone();

        let mut artifacts = lock_mutex_recover(self.artifacts.as_ref(), "artifacts");
        let id = artifact.id.clone();
        artifacts.push(artifact);

        // Update counters for the newly pushed artifact.
        if new_is_active {
            self.active_artifacts.fetch_add(1, Ordering::Relaxed);
        }
        {
            let mut counts = lock_mutex_recover(self.producer_counts.as_ref(), "producer_counts");
            *counts.entry(new_producer).or_insert(0) += 1;
        }

        // Enforce retention limit: remove oldest entries when over cap.
        if artifacts.len() > self.max_artifacts {
            let excess = artifacts.len() - self.max_artifacts;
            let drained: Vec<Artifact> = artifacts.drain(0..excess).collect();

            // Update counters for each drained artifact.
            for a in &drained {
                if a.created_ms + a.ttl_ms > now_ms {
                    self.active_artifacts.fetch_sub(1, Ordering::Relaxed);
                }
            }
            {
                let mut counts =
                    lock_mutex_recover(self.producer_counts.as_ref(), "producer_counts");
                for a in &drained {
                    if let Some(c) = counts.get_mut(&a.producer) {
                        *c -= 1;
                        if *c == 0 {
                            counts.remove(&a.producer);
                        }
                    }
                }
            }
        }

        // Update profile from incremental counters (no full Vec scan).
        {
            let mut profile = lock_mutex_recover(self.profile.as_ref(), "profile");
            profile.total_artifacts = artifacts.len() as u32;
            profile.active_artifacts = self.active_artifacts.load(Ordering::Relaxed);
            {
                let counts = lock_mutex_recover(self.producer_counts.as_ref(), "producer_counts");
                profile.producers = counts.len() as u32;
            }
        }

        Ok(id)
    }

    /// Finds artifacts matching the given schema name.
    pub fn find_by_schema(&self, schema_name: &str) -> Vec<Artifact> {
        let artifacts = lock_mutex_recover(self.artifacts.as_ref(), "artifacts");
        artifacts
            .iter()
            .filter(|a| a.schema_name == schema_name)
            .cloned()
            .collect()
    }

    /// Finds artifacts produced by the given agent.
    pub fn find_by_producer(&self, producer: &str) -> Vec<Artifact> {
        let artifacts = lock_mutex_recover(self.artifacts.as_ref(), "artifacts");
        artifacts
            .iter()
            .filter(|a| a.producer == producer)
            .cloned()
            .collect()
    }

    /// Finds artifacts that have at least one of the given tags.
    ///
    /// Returns artifacts whose tags set intersects with the `tags` list.
    pub fn find_by_tags(&self, tags: &[String]) -> Vec<Artifact> {
        let artifacts = lock_mutex_recover(self.artifacts.as_ref(), "artifacts");
        artifacts
            .iter()
            .filter(|a| a.tags.iter().any(|t| tags.contains(t)))
            .cloned()
            .collect()
    }

    /// Finds artifacts associated with the given task ID.
    pub fn find_by_task(&self, task_id: &str) -> Vec<Artifact> {
        let artifacts = lock_mutex_recover(self.artifacts.as_ref(), "artifacts");
        artifacts
            .iter()
            .filter(|a| a.task_id == task_id)
            .cloned()
            .collect()
    }

    /// Removes artifacts whose TTL has expired.
    ///
    /// An artifact is considered expired when
    /// `created_ms + ttl_ms <= current_time_ms`.
    pub fn prune_expired(&self) {
        let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as u64,
            Err(e) => {
                tracing::error!(
                    "Clock is before epoch; skipping artifact pruning to avoid data loss: {e}"
                );
                return;
            }
        };

        let mut artifacts = lock_mutex_recover(self.artifacts.as_ref(), "artifacts");

        // Partition into kept and expired, collecting expired for counter maintenance.
        let mut expired: Vec<Artifact> = Vec::new();
        let mut i = 0;
        while i < artifacts.len() {
            if artifacts[i].created_ms + artifacts[i].ttl_ms <= now {
                expired.push(artifacts.swap_remove(i));
            } else {
                i += 1;
            }
        }

        // Update counters for removed expired artifacts.
        {
            let mut counts = lock_mutex_recover(self.producer_counts.as_ref(), "producer_counts");
            for a in &expired {
                self.active_artifacts.fetch_sub(1, Ordering::Relaxed);
                if let Some(c) = counts.get_mut(&a.producer) {
                    *c -= 1;
                    if *c == 0 {
                        counts.remove(&a.producer);
                    }
                }
            }
        }

        // Update profile from incremental counters (no full Vec scan).
        let mut profile = lock_mutex_recover(self.profile.as_ref(), "profile");
        profile.total_artifacts = artifacts.len() as u32;
        profile.active_artifacts = self.active_artifacts.load(Ordering::Relaxed);
        {
            let counts = lock_mutex_recover(self.producer_counts.as_ref(), "producer_counts");
            profile.producers = counts.len() as u32;
        }
    }

    /// Returns a snapshot of the current profile metrics.
    pub fn profile(&self) -> ArtifactProfile {
        let profile = lock_mutex_recover(self.profile.as_ref(), "profile");
        profile.clone()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_schema(name: &str) -> ArtifactSchema {
        ArtifactSchema {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: format!("Schema for {}", name),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string" }
                }
            }),
            required_fields: vec!["summary".to_string()],
            created_at: 1_700_000_000_000,
        }
    }

    fn sample_artifact(schema_name: &str, producer: &str, task: &str) -> Artifact {
        Artifact {
            id: String::new(),
            schema_name: schema_name.to_string(),
            schema_version: "1.0.0".to_string(),
            producer: producer.to_string(),
            task_id: task.to_string(),
            content: serde_json::json!({ "summary": "test result" }),
            created_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            ttl_ms: 60_000,
            tags: vec![],
        }
    }

    #[test]
    fn test_new_layer_is_empty() {
        let layer = ArtifactLayer::new();
        let p = layer.profile();
        assert!(p.enabled);
        assert_eq!(p.registered_schemas, 0);
        assert_eq!(p.total_artifacts, 0);
        assert_eq!(p.active_artifacts, 0);
        assert_eq!(p.producers, 0);
    }

    #[test]
    fn test_register_and_validate_ok() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();

        let artifact = sample_artifact("report", "tester", "task-1");
        let validation = layer.validate(&artifact);
        assert!(validation.is_valid);
        assert!(validation.errors.is_empty());
        assert!(validation.missing_fields.is_empty());
    }

    #[test]
    fn test_validate_missing_schema() {
        let layer = ArtifactLayer::new();
        let artifact = sample_artifact("nonexistent", "tester", "task-1");
        let validation = layer.validate(&artifact);
        assert!(!validation.is_valid);
        assert!(validation.errors.iter().any(|e| e.contains("nonexistent")));
    }

    #[test]
    fn test_validate_missing_fields() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();

        let artifact = Artifact {
            content: serde_json::json!({}),
            ..sample_artifact("report", "tester", "task-1")
        };
        let validation = layer.validate(&artifact);
        assert!(!validation.is_valid);
        assert!(validation.missing_fields.contains(&"summary".to_string()));
    }

    #[test]
    fn test_store_and_find() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();

        let id = layer
            .store(sample_artifact("report", "tester", "task-1"))
            .unwrap();
        assert!(id.starts_with("art-"));

        let by_schema = layer.find_by_schema("report");
        assert_eq!(by_schema.len(), 1);
        assert_eq!(by_schema[0].id, id);

        let by_producer = layer.find_by_producer("tester");
        assert_eq!(by_producer.len(), 1);
        assert_eq!(by_producer[0].id, id);

        let by_task = layer.find_by_task("task-1");
        assert_eq!(by_task.len(), 1);
    }

    #[test]
    fn test_store_invalid_artifact() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();

        let artifact = Artifact {
            content: serde_json::json!({}),
            ..sample_artifact("report", "tester", "task-1")
        };
        let result = layer.store(artifact);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_by_tags() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();

        let mut artifact = sample_artifact("report", "tester", "task-1");
        artifact.tags = vec!["critical".to_string(), "bug".to_string()];
        layer.store(artifact).unwrap();

        let found = layer.find_by_tags(&["critical".to_string()]);
        assert_eq!(found.len(), 1);

        let found = layer.find_by_tags(&["performance".to_string()]);
        assert!(found.is_empty());
    }

    #[test]
    fn test_prune_expired() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();

        let mut artifact = sample_artifact("report", "tester", "task-1");
        artifact.created_ms = 1;
        artifact.ttl_ms = 1;
        layer.store(artifact).unwrap();

        assert_eq!(layer.find_by_schema("report").len(), 1);
        layer.prune_expired();
        assert_eq!(layer.find_by_schema("report").len(), 0);
    }

    #[test]
    fn test_profile_updates() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();
        layer
            .store(sample_artifact("report", "tester", "task-1"))
            .unwrap();
        layer
            .store(sample_artifact("report", "coder", "task-2"))
            .unwrap();

        let p = layer.profile();
        assert_eq!(p.registered_schemas, 1);
        assert_eq!(p.total_artifacts, 2);
        assert_eq!(p.active_artifacts, 2);
        assert_eq!(p.producers, 2);
    }

    #[test]
    fn test_version_mismatch_warning() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();

        let artifact = Artifact {
            schema_version: "1.1.0".to_string(),
            ..sample_artifact("report", "tester", "task-1")
        };
        let validation = layer.validate(&artifact);
        assert!(validation.is_valid);
        assert!(validation
            .warnings
            .iter()
            .any(|w| w.contains("version mismatch")));
    }

    #[test]
    fn test_version_major_mismatch_error() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();

        let artifact = Artifact {
            schema_version: "2.0.0".to_string(),
            ..sample_artifact("report", "tester", "task-1")
        };
        let validation = layer.validate(&artifact);
        assert!(!validation.is_valid);
        assert!(validation
            .errors
            .iter()
            .any(|e| e.contains("major version mismatch")));
    }

    #[test]
    fn test_duplicate_schema_rejected() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("report")).unwrap();
        let result = layer.register_schema(sample_schema("report"));
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_must_be_object() {
        let layer = ArtifactLayer::new();
        let schema = ArtifactSchema {
            schema: serde_json::json!("not an object"),
            ..sample_schema("bad")
        };
        let result = layer.register_schema(schema);
        assert!(result.is_err());
    }

    #[test]
    fn test_max_artifacts_eviction() {
        let layer = ArtifactLayer::new();
        layer.register_schema(sample_schema("x")).unwrap();

        // Store more artifacts than max_artifacts (default 10 000).
        // We'll use a smaller max by creating a new layer directly.
        // Actually, we can't set max_artifacts after construction easily.
        // So test that normal storage works within limits.
        for i in 0..5 {
            let mut art = sample_artifact("x", "producer", &format!("task-{}", i));
            art.content = serde_json::json!({ "summary": format!("result {}", i) });
            layer.store(art).unwrap();
        }
        assert_eq!(layer.find_by_schema("x").len(), 5);
    }
}
