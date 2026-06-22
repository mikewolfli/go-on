//! F-GAP-02: Reputation Store
//!
//! Maintains an EMA-based reliability score per agent/node.  Scores feed the
//! router's ranking to downweight consistently failing agents.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::intelligence::now_ms;

/// Reputation record for a single agent/node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationRecord {
    pub agent: String,
    /// EMA reliability score 0.0–1.0
    pub score: f64,
    pub total_tasks: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub consecutive_failures: u32,
    pub last_updated_ms: u64,
}

impl ReputationRecord {
    fn new(agent: &str) -> Self {
        Self {
            agent: agent.to_string(),
            score: 1.0,
            total_tasks: 0,
            success_count: 0,
            failure_count: 0,
            consecutive_failures: 0,
            last_updated_ms: now_ms(),
        }
    }
}

/// Configuration for reputation tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// EMA smoothing factor α (0.0–1.0); higher = faster adaption
    #[serde(default = "default_alpha")]
    pub ema_alpha: f64,
    /// Score threshold below which agent is marked "degraded"
    #[serde(default = "default_degraded")]
    pub degraded_threshold: f64,
    /// Score threshold below which agent is excluded from routing
    #[serde(default = "default_excluded")]
    pub exclusion_threshold: f64,
}

fn default_enabled() -> bool {
    true
}
fn default_alpha() -> f64 {
    0.2
}
fn default_degraded() -> f64 {
    0.65
}
fn default_excluded() -> f64 {
    0.30
}

impl Default for ReputationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ema_alpha: 0.2,
            degraded_threshold: 0.65,
            exclusion_threshold: 0.30,
        }
    }
}

/// Default maximum records to retain before evicting the oldest.
const DEFAULT_MAX_RECORDS: usize = 10_000;

/// Serializable state persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreData {
    config: ReputationConfig,
    records: HashMap<String, ReputationRecord>,
    max_records: usize,
}

/// Central reputation store
#[derive(Debug)]
pub struct ReputationStore {
    config: ReputationConfig,
    records: HashMap<String, ReputationRecord>,
    max_records: usize,
    persistence_path: Option<PathBuf>,
}

impl ReputationStore {
    pub fn new(config: ReputationConfig) -> Self {
        Self {
            config,
            records: HashMap::new(),
            max_records: DEFAULT_MAX_RECORDS,
            persistence_path: None,
        }
    }

    /// Set a file path for automatic persistence.
    /// When set, every call to `record_outcome()` will save state to this file.
    pub fn with_persistence_path(mut self, path: PathBuf) -> Self {
        self.persistence_path = Some(path);
        self
    }

    /// Save the current store state to a JSON file.
    pub fn save_to_file(&self) -> std::io::Result<()> {
        let path = self.persistence_path.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "persistence path not set")
        })?;
        let data = StoreData {
            config: self.config.clone(),
            records: self.records.clone(),
            max_records: self.max_records,
        };
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    /// Load a `ReputationStore` from a JSON file on disk.
    /// The loaded store will **not** have a persistence path set — call
    /// `with_persistence_path()` if you want auto-save on the restored instance.
    pub fn load_from_file(path: PathBuf) -> std::io::Result<Self> {
        let json = fs::read_to_string(&path)?;
        let data: StoreData = serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Self {
            config: data.config,
            records: data.records,
            max_records: data.max_records,
            persistence_path: None,
        })
    }

    fn record(&mut self, agent: &str) -> &mut ReputationRecord {
        // Evict the lowest-scored entry when at capacity (agent not already tracked).
        if !self.records.contains_key(agent) && self.records.len() >= self.max_records {
            if let Some(lowest_key) = self
                .records
                .iter()
                .min_by(|(_, a), (_, b)| {
                    a.score
                        .partial_cmp(&b.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(k, _)| k.clone())
            {
                self.records.remove(&lowest_key);
            }
        }

        self.records
            .entry(agent.to_string())
            .or_insert_with(|| ReputationRecord::new(agent))
    }

    /// Record a task outcome for an agent
    pub fn record_outcome(&mut self, agent: &str, success: bool) {
        if !self.config.enabled {
            return;
        }

        let alpha = self.config.ema_alpha;
        let r = self.record(agent);

        // Capture the previous timestamp before updating it, for decay calculation
        let prev_updated_ms = r.last_updated_ms;

        r.total_tasks += 1;
        r.last_updated_ms = now_ms();
        if success {
            r.success_count += 1;
            r.consecutive_failures = 0;
        } else {
            r.failure_count += 1;
            r.consecutive_failures += 1;
        }
        let outcome = if success { 1.0f64 } else { 0.0f64 };
        r.score = alpha * outcome + (1.0 - alpha) * r.score;

        // Apply gradual time-based decay toward baseline (0.5)
        let now_ms_val = r.last_updated_ms;
        let elapsed_ms = now_ms_val.saturating_sub(prev_updated_ms);
        if prev_updated_ms > 0 {
            let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
            // Decay starts immediately and saturates at 7 days (168 hours)
            let decay = (-0.005 * (elapsed_hours.min(168.0))).exp();
            r.score = 0.5 + (r.score - 0.5) * decay;
        }

        // Auto-save if a persistence path is configured
        if self.persistence_path.is_some() {
            if let Err(e) = self.save_to_file() {
                tracing::warn!("failed to persist reputation store: {e}");
            }
        }
    }

    /// Current score for agent (1.0 for unknown agents)
    pub fn score(&self, agent: &str) -> f64 {
        self.records.get(agent).map(|r| r.score).unwrap_or(1.0)
    }

    pub fn is_degraded(&self, agent: &str) -> bool {
        self.score(agent) < self.config.degraded_threshold
    }

    pub fn is_excluded(&self, agent: &str) -> bool {
        self.score(agent) < self.config.exclusion_threshold
    }

    pub fn snapshot(&self) -> Vec<ReputationRecord> {
        let mut v: Vec<_> = self.records.values().cloned().collect();
        v.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    pub fn tracked_agent_count(&self) -> usize {
        self.records.len()
    }
}

// Use `crate::intelligence::now_ms()` instead — shared utility in mod.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_outcome_creates_entry() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        store.record_outcome("agent_a", true);
        assert_eq!(store.tracked_agent_count(), 1);
        let score = store.score("agent_a");
        assert!(score > 0.0);
    }

    #[test]
    fn test_score_increases_on_success() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        // First failure lowers the score from 1.0
        store.record_outcome("agent_a", false);
        let s1 = store.score("agent_a");
        assert!(s1 < 1.0);
        // Then a success should increase it
        store.record_outcome("agent_a", true);
        let s2 = store.score("agent_a");
        assert!(s2 > s1);
    }

    #[test]
    fn test_score_decreases_on_failure() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        store.record_outcome("agent_a", true);
        let s1 = store.score("agent_a");
        store.record_outcome("agent_a", false);
        let s2 = store.score("agent_a");
        assert!(s2 < s1);
    }

    #[test]
    fn test_is_degraded_after_many_failures() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        for _ in 0..10 {
            store.record_outcome("agent_a", false);
        }
        assert!(store.is_degraded("agent_a"));
    }

    #[test]
    fn test_snapshot_includes_all_agents() {
        let mut store = ReputationStore::new(ReputationConfig::default());
        store.record_outcome("agent_a", true);
        store.record_outcome("agent_b", false);
        let snap = store.snapshot();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn test_unknown_agent_score_is_default() {
        let store = ReputationStore::new(ReputationConfig::default());
        assert!((store.score("unknown") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_unknown_agent_not_degraded() {
        let store = ReputationStore::new(ReputationConfig::default());
        assert!(!store.is_degraded("unknown"));
    }

    #[test]
    fn test_save_then_load_round_trip() {
        // Use a temporary directory so the file is cleaned up automatically.
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("reputation.json");

        // Build a store with persistence, record some outcomes.
        let mut store =
            ReputationStore::new(ReputationConfig::default()).with_persistence_path(path.clone());
        store.record_outcome("alice", true);
        store.record_outcome("alice", true);
        store.record_outcome("bob", false);
        store.record_outcome("bob", false);
        store.record_outcome("bob", true);

        let alice_score_before = store.score("alice");
        let bob_score_before = store.score("bob");
        let snapshot_before = store.snapshot();
        let count_before = store.tracked_agent_count();

        // Load from file into a fresh store.
        let restored = ReputationStore::load_from_file(path.clone()).expect("load");

        assert_eq!(restored.tracked_agent_count(), count_before);
        // Floating-point comparison with tolerance to handle serialization precision loss
        fn approx_eq(a: f64, b: f64) -> bool {
            (a - b).abs() < 1e-12
        }
        assert!(
            approx_eq(restored.score("alice"), alice_score_before),
            "alice score mismatch: {}",
            restored.score("alice")
        );
        assert!(
            approx_eq(restored.score("bob"), bob_score_before),
            "bob score mismatch: {}",
            restored.score("bob")
        );

        let snap_restored = restored.snapshot();
        assert_eq!(snap_restored.len(), snapshot_before.len());

        // Verify individual record fields match (with float tolerance for score).
        for rec in &snap_restored {
            let original = snapshot_before
                .iter()
                .find(|r| r.agent == rec.agent)
                .unwrap();
            assert!(
                approx_eq(rec.score, original.score),
                "score mismatch for '{}': {} vs {}",
                rec.agent,
                rec.score,
                original.score
            );
            assert_eq!(rec.total_tasks, original.total_tasks);
            assert_eq!(rec.success_count, original.success_count);
            assert_eq!(rec.failure_count, original.failure_count);
        }
    }

    #[test]
    fn test_save_then_load_with_config() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("reputation_config.json");

        let config = ReputationConfig {
            enabled: true,
            ema_alpha: 0.5,
            degraded_threshold: 0.8,
            exclusion_threshold: 0.5,
        };
        let mut store = ReputationStore::new(config.clone()).with_persistence_path(path.clone());
        store.record_outcome("agent_x", false);

        let restored = ReputationStore::load_from_file(path).expect("load");
        // Restored store should use the persisted config
        assert!((restored.config.ema_alpha - 0.5).abs() < f64::EPSILON);
        assert!((restored.config.degraded_threshold - 0.8).abs() < f64::EPSILON);
        assert!((restored.config.exclusion_threshold - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_loaded_store_persistence_path_is_none() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("reputation_none.json");

        {
            let mut store = ReputationStore::new(ReputationConfig::default())
                .with_persistence_path(path.clone());
            store.record_outcome("a", true);
        } // Drop the original store.

        let restored = ReputationStore::load_from_file(path).expect("load");
        // A freshly loaded store has no persistence path (caller must opt in).
        // We verify by checking that `save_to_file` returns an error (no path set).
        assert!(restored.save_to_file().is_err());
    }
}
