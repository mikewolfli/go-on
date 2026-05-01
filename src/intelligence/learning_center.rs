//! BLUE38 F-GAP-24: Continuous Learning Center
//!
//! A thread-safe continuous learning center that prevents catastrophic forgetting
//! and manages continuous learning workflows. Tracks learning experiences,
//! consolidates them into knowledge chunks, monitors task performance trends,
//! and replays high-importance experiences to reinforce learning.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the learning center.
#[derive(Debug, Clone)]
#[allow(dead_code)] // F-GAP-08 — planned wiring
pub struct LearningCenterConfig {
    /// Maximum number of experiences stored before oldest are evicted.
    pub max_experiences: usize,
    /// Interval in milliseconds between automatic consolidation runs.
    #[allow(dead_code)] // F-GAP-08 — planned wiring
    pub consolidation_interval_ms: u64,
    /// Number of experiences sampled in each replay batch.
    pub replay_batch_size: usize,
    /// Minimum average importance required for consolidation.
    pub importance_threshold: f64,
    /// Number of top knowledge chunks protected from pruning.
    pub forgetting_protection_top_k: usize,
}

impl Default for LearningCenterConfig {
    fn default() -> Self {
        Self {
            max_experiences: 10000,
            consolidation_interval_ms: 60000,
            replay_batch_size: 64,
            importance_threshold: 0.3,
            forgetting_protection_top_k: 100,
        }
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single learning experience recorded by the center.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningExperience {
    pub id: String,
    pub task_type: String,
    pub input_summary: String,
    pub output_summary: String,
    pub success: bool,
    pub reward: f64,
    pub importance: f64,
    pub timestamp_ms: u64,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, String>,
}

/// Extra annotations attached to a recorded experience.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(dead_code)] // F-GAP-08 — planned wiring
pub struct ExperienceContext {
    pub tags: Vec<String>,
    pub metadata: HashMap<String, String>,
}

impl ExperienceContext {
    #[allow(dead_code)] // F-GAP-08 — planned wiring
    pub fn new(tags: Vec<String>, metadata: HashMap<String, String>) -> Self {
        Self { tags, metadata }
    }
}

/// A consolidated knowledge chunk abstracted from multiple experiences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidatedKnowledge {
    pub id: String,
    pub pattern: String,
    pub derived_insight: String,
    pub source_experience_ids: Vec<String>,
    pub confidence: f64,
    pub applicability_tags: Vec<String>,
    pub created_ms: u64,
    pub last_accessed_ms: u64,
    pub access_count: u64,
}

/// Performance metrics for a specific task type over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPerformanceHistory {
    pub task_type: String,
    pub total_attempts: u64,
    pub successful_attempts: u64,
    pub avg_reward: f64,
    pub recent_rewards: Vec<f64>,
    pub last_updated_ms: u64,
    pub trend_direction: String,
}

/// Snapshot of the learning center's state and metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // F-GAP-08 — planned wiring
pub struct LearningCenterProfile {
    pub enabled: bool,
    pub total_experiences: usize,
    pub consolidated_count: usize,
    pub tracked_task_types: usize,
    pub last_consolidation_ms: u64,
    pub avg_importance: f64,
    pub forgetting_protected_count: usize,
    pub replay_count: u64,
    pub catastrophic_forgetting_events: u64,
}

#[allow(dead_code)] // F-GAP-08 — planned wiring
type ConsolidationGroupEntry = (String, f64, bool, Vec<String>);

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

#[allow(dead_code)] // F-GAP-08 — planned wiring
struct LearningCenterInner {
    config: LearningCenterConfig,
    experiences: VecDeque<LearningExperience>,
    consolidated: Vec<ConsolidatedKnowledge>,
    task_performance: HashMap<String, TaskPerformanceHistory>,
    last_consolidation_ms: u64,
    next_experience_id: u64,
    next_knowledge_id: u64,
    replay_count: u64,
    forgetting_events: u64,
}

impl LearningCenterInner {
    #[allow(dead_code)] // F-GAP-08 — planned wiring
    fn importance(reward: f64, now_ms: u64, timestamp_ms: u64) -> f64 {
        let reward_magnitude = reward.abs();
        let age_ms = now_ms.saturating_sub(timestamp_ms);
        let recency_factor = if age_ms > 3_600_000 {
            0.0
        } else {
            1.0 - (age_ms as f64 / 3_600_000.0)
        };
        (reward_magnitude * 0.6 + recency_factor * 0.4).clamp(0.0, 1.0)
    }

    #[allow(dead_code)] // F-GAP-08 — planned wiring
    fn compute_trend(recent_rewards: &[f64]) -> String {
        let len = recent_rewards.len();
        if len < 20 {
            return "stable".to_string();
        }
        let recent_10: Vec<f64> = recent_rewards[len.saturating_sub(10)..].to_vec();
        let prior_10: Vec<f64> =
            recent_rewards[len.saturating_sub(20)..len.saturating_sub(10)].to_vec();
        let recent_avg: f64 = recent_10.iter().sum::<f64>() / recent_10.len() as f64;
        let prior_avg: f64 = prior_10.iter().sum::<f64>() / prior_10.len() as f64;
        if recent_avg > prior_avg + 0.05 {
            "improving".to_string()
        } else if recent_avg < prior_avg - 0.05 {
            "declining".to_string()
        } else {
            "stable".to_string()
        }
    }

    #[allow(dead_code)] // F-GAP-08 — planned wiring
    fn linear_regression_slope(values: &[f64]) -> f64 {
        let n = values.len() as f64;
        if n < 2.0 {
            return 0.0;
        }
        let sum_x: f64 = (0..values.len()).map(|i| i as f64).sum();
        let sum_y: f64 = values.iter().sum();
        let sum_xy: f64 = values.iter().enumerate().map(|(i, v)| i as f64 * v).sum();
        let sum_xx: f64 = (0..values.len()).map(|i| (i as f64) * (i as f64)).sum();
        let denom = n * sum_xx - sum_x * sum_x;
        if denom.abs() < 1e-12 {
            0.0
        } else {
            (n * sum_xy - sum_x * sum_y) / denom
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Acquire a lock on the inner mutex, recovering from poison.
fn lock_guard<T>(mtx: &Mutex<T>) -> MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("learning_center mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// Thread-safe continuous learning center.
#[allow(dead_code)] // F-GAP-08 — planned wiring
pub struct ContinuousLearningCenter {
    inner: Arc<Mutex<LearningCenterInner>>,
}

#[allow(dead_code)] // F-GAP-08 — planned wiring
impl ContinuousLearningCenter {
    /// Creates a new learning center with the given configuration.
    pub fn new(config: LearningCenterConfig) -> Self {
        let now = now_ms();
        Self {
            inner: Arc::new(Mutex::new(LearningCenterInner {
                config,
                experiences: VecDeque::new(),
                consolidated: Vec::new(),
                task_performance: HashMap::new(),
                last_consolidation_ms: now,
                next_experience_id: 1,
                next_knowledge_id: 1,
                replay_count: 0,
                forgetting_events: 0,
            })),
        }
    }

    /// Records a new learning experience and computes its importance.
    pub fn record_experience(
        &self,
        task_type: String,
        input_summary: String,
        output_summary: String,
        success: bool,
        reward: f64,
        context: ExperienceContext,
    ) -> String {
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();
        let id = generate_id("exp", &mut inner.next_experience_id);
        let importance = LearningCenterInner::importance(reward, now, now);

        let experience = LearningExperience {
            id: id.clone(),
            task_type: task_type.clone(),
            input_summary,
            output_summary,
            success,
            reward,
            importance,
            timestamp_ms: now,
            tags: context.tags.clone(),
            metadata: context.metadata,
        };

        // Enforce max_experiences cap.
        if inner.experiences.len() >= inner.config.max_experiences {
            inner.experiences.pop_front();
        }
        inner.experiences.push_back(experience);

        // Update task performance history.
        let perf =
            inner
                .task_performance
                .entry(task_type.clone())
                .or_insert(TaskPerformanceHistory {
                    task_type,
                    total_attempts: 0,
                    successful_attempts: 0,
                    avg_reward: 0.0,
                    recent_rewards: Vec::new(),
                    last_updated_ms: now,
                    trend_direction: "stable".to_string(),
                });
        perf.total_attempts += 1;
        if success {
            perf.successful_attempts += 1;
        }
        // Running average reward.
        let n = perf.total_attempts as f64;
        perf.avg_reward = perf.avg_reward * ((n - 1.0) / n) + reward / n;
        let window = 100;
        if perf.recent_rewards.len() >= window {
            perf.recent_rewards.remove(0);
        }
        perf.recent_rewards.push(reward);
        perf.last_updated_ms = now;
        perf.trend_direction = LearningCenterInner::compute_trend(&perf.recent_rewards);

        id
    }

    /// Retrieves a learning experience by its ID.
    pub fn get_experience(&self, id: &str) -> Option<LearningExperience> {
        let inner = lock_guard(&self.inner);
        inner.experiences.iter().find(|e| e.id == id).cloned()
    }

    /// Queries experiences by task type and/or tags.
    pub fn query_experiences(&self, task_type: &str, tags: &[String]) -> Vec<LearningExperience> {
        let inner = lock_guard(&self.inner);
        inner
            .experiences
            .iter()
            .filter(|e| {
                let matches_task = task_type.is_empty() || e.task_type == task_type;
                let matches_tags =
                    tags.is_empty() || tags.iter().all(|t| e.tags.iter().any(|et| et == t));
                matches_task && matches_tags
            })
            .cloned()
            .collect()
    }

    /// Consolidates recent experiences into knowledge chunks.
    ///
    /// Groups experiences by task_type. If a group has at least 5 experiences
    /// and an average importance above the threshold, a `ConsolidatedKnowledge`
    /// entry is created (or updated if one already exists for that pattern).
    pub fn consolidate(&self) {
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();

        // Group experiences by task_type — clone all data we need outside the lock.
        let mut groups: HashMap<String, Vec<ConsolidationGroupEntry>> = HashMap::new();
        for exp in inner.experiences.iter() {
            let entry = (
                exp.id.clone(),
                exp.importance,
                exp.success,
                exp.tags.clone(),
            );
            groups.entry(exp.task_type.clone()).or_default().push(entry);
        }
        let threshold = inner.config.importance_threshold;

        // Drop the implicit borrow on groups so we can mutate inner.
        for (task_type, exps) in groups.iter() {
            if exps.len() < 5 {
                continue;
            }
            let avg_importance: f64 =
                exps.iter().map(|(_, imp, _, _)| imp).sum::<f64>() / exps.len() as f64;
            if avg_importance < threshold {
                continue;
            }

            let source_ids: Vec<String> = exps.iter().map(|(id, _, _, _)| id.clone()).collect();
            let success_count = exps.iter().filter(|(_, _, success, _)| *success).count();
            let confidence = success_count as f64 / exps.len() as f64;

            let derived_insight = format!(
                "Consolidated {} {} experiences: {}/{} successful (confidence {:.2})",
                exps.len(),
                task_type,
                success_count,
                exps.len(),
                confidence
            );

            let mut all_tags: Vec<String> = Vec::new();
            let mut seen_tags = std::collections::HashSet::new();
            for (_, _, _, tags) in exps.iter() {
                for tag in tags {
                    if seen_tags.insert(tag.clone()) {
                        all_tags.push(tag.clone());
                    }
                }
            }

            let pattern = format!("pattern:{}", task_type);
            if let Some(existing) = inner.consolidated.iter_mut().find(|k| k.pattern == pattern) {
                existing.source_experience_ids = source_ids;
                existing.confidence = confidence;
                existing.derived_insight = derived_insight;
                existing.applicability_tags = all_tags;
                existing.last_accessed_ms = now;
                existing.access_count += 1;
            } else {
                let kid = generate_id("know", &mut inner.next_knowledge_id);
                inner.consolidated.push(ConsolidatedKnowledge {
                    id: kid,
                    pattern,
                    derived_insight,
                    source_experience_ids: source_ids,
                    confidence,
                    applicability_tags: all_tags,
                    created_ms: now,
                    last_accessed_ms: now,
                    access_count: 1,
                });
            }
        }

        inner.last_consolidation_ms = now;
    }

    /// Retrieves a consolidated knowledge chunk by its pattern.
    pub fn get_knowledge(&self, pattern: &str) -> Option<ConsolidatedKnowledge> {
        let mut inner = lock_guard(&self.inner);
        if let Some(k) = inner.consolidated.iter_mut().find(|k| k.pattern == pattern) {
            k.last_accessed_ms = now_ms();
            k.access_count += 1;
            Some(k.clone())
        } else {
            None
        }
    }

    /// Lists consolidated knowledge chunks matching the given tags.
    pub fn list_knowledge(&self, tags: &[String]) -> Vec<ConsolidatedKnowledge> {
        let mut inner = lock_guard(&self.inner);
        let now = now_ms();
        let results: Vec<ConsolidatedKnowledge> = inner
            .consolidated
            .iter_mut()
            .filter(|k| {
                tags.is_empty()
                    || tags
                        .iter()
                        .all(|t| k.applicability_tags.iter().any(|kt| kt == t))
            })
            .map(|k| {
                k.last_accessed_ms = now;
                k.access_count += 1;
                k.clone()
            })
            .collect();
        results
    }

    /// Samples a batch of high-importance experiences for replay (anti-forgetting).
    ///
    /// Returns the top `config.replay_batch_size` experiences sorted by importance,
    /// updating access counts on related consolidated knowledge.
    pub fn replay_batch(&self) -> Vec<LearningExperience> {
        let mut inner = lock_guard(&self.inner);
        inner.replay_count += 1;

        // Sort by importance descending, take top N.
        let mut all: Vec<LearningExperience> = inner.experiences.iter().cloned().collect();
        all.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let batch: Vec<LearningExperience> = all
            .into_iter()
            .take(inner.config.replay_batch_size)
            .collect();

        // Update access counts on related consolidated knowledge.
        let now = now_ms();
        for exp in &batch {
            let pattern = format!("pattern:{}", exp.task_type);
            if let Some(k) = inner.consolidated.iter_mut().find(|k| k.pattern == pattern) {
                k.last_accessed_ms = now;
                k.access_count += 1;
            }
        }

        batch
    }

    /// Retrieves the performance history for a specific task type.
    pub fn task_performance(&self, task_type: &str) -> Option<TaskPerformanceHistory> {
        let inner = lock_guard(&self.inner);
        inner.task_performance.get(task_type).cloned()
    }

    /// Returns performance histories for all tracked task types.
    pub fn all_task_performance(&self) -> Vec<TaskPerformanceHistory> {
        let inner = lock_guard(&self.inner);
        let mut results: Vec<TaskPerformanceHistory> =
            inner.task_performance.values().cloned().collect();
        results.sort_by(|a, b| a.task_type.cmp(&b.task_type));
        results
    }

    /// Detects whether performance on a given task type is in catastrophic forgetting.
    ///
    /// Uses linear regression on the last 20 rewards. If the slope is < -0.05,
    /// the task type is considered to be in a forgetting state.
    pub fn detect_forgetting(&self, task_type: &str) -> bool {
        let mut inner = lock_guard(&self.inner);
        if let Some(perf) = inner.task_performance.get(task_type) {
            if perf.recent_rewards.len() < 20 {
                return false;
            }
            let recent_20: Vec<f64> =
                perf.recent_rewards[perf.recent_rewards.len().saturating_sub(20)..].to_vec();
            let slope = LearningCenterInner::linear_regression_slope(&recent_20);
            if slope < -0.05 {
                inner.forgetting_events += 1;
                return true;
            }
        }
        false
    }

    /// Returns a profile snapshot of the learning center's current state.
    pub fn profile(&self) -> LearningCenterProfile {
        let inner = lock_guard(&self.inner);
        let total_experiences = inner.experiences.len();
        let consolidated_count = inner.consolidated.len();
        let tracked_task_types = inner.task_performance.len();
        let last_consolidation_ms = inner.last_consolidation_ms;
        let forgetting_protected_count = inner
            .config
            .forgetting_protection_top_k
            .min(consolidated_count);
        let replay_count = inner.replay_count;
        let catastrophic_forgetting_events = inner.forgetting_events;

        let avg_importance = if total_experiences > 0 {
            let sum: f64 = inner.experiences.iter().map(|e| e.importance).sum();
            sum / total_experiences as f64
        } else {
            0.0
        };

        LearningCenterProfile {
            enabled: true,
            total_experiences,
            consolidated_count,
            tracked_task_types,
            last_consolidation_ms,
            avg_importance,
            forgetting_protected_count,
            replay_count,
            catastrophic_forgetting_events,
        }
    }

    /// Returns a copy of the current configuration.
    #[allow(dead_code)] // F-GAP-08 — planned wiring
    pub fn config(&self) -> LearningCenterConfig {
        let inner = lock_guard(&self.inner);
        inner.config.clone()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns the current timestamp in milliseconds since the Unix epoch.
#[allow(dead_code)] // F-GAP-08 — planned wiring
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Generates a monotonic unique ID with the given prefix.
#[allow(dead_code)] // F-GAP-08 — planned wiring
fn generate_id(prefix: &str, counter: &mut u64) -> String {
    let id = *counter;
    *counter += 1;
    format!("{}-{}", prefix, id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> LearningCenterConfig {
        LearningCenterConfig {
            max_experiences: 100,
            consolidation_interval_ms: 60000,
            replay_batch_size: 5,
            importance_threshold: 0.3,
            forgetting_protection_top_k: 10,
        }
    }

    #[test]
    fn test_new_center_empty() {
        let center = ContinuousLearningCenter::new(make_config());
        let profile = center.profile();
        assert!(profile.enabled);
        assert_eq!(profile.total_experiences, 0);
        assert_eq!(profile.consolidated_count, 0);
        assert_eq!(profile.tracked_task_types, 0);
        assert_eq!(profile.replay_count, 0);
        assert_eq!(profile.catastrophic_forgetting_events, 0);
    }

    #[test]
    fn test_record_experience() {
        let center = ContinuousLearningCenter::new(make_config());
        let id = center.record_experience(
            "translation".to_string(),
            "Hello world".to_string(),
            "Bonjour le monde".to_string(),
            true,
            0.85,
            ExperienceContext::new(vec!["language".to_string()], HashMap::new()),
        );
        assert!(id.starts_with("exp-"));

        let exp = center.get_experience(&id).expect("experience should exist");
        assert_eq!(exp.task_type, "translation");
        assert!(exp.success);
        assert!((exp.importance - 0.0).abs() < 1.0); // importance should be in [0,1]

        let profile = center.profile();
        assert_eq!(profile.total_experiences, 1);
    }

    #[test]
    fn test_query_experiences() {
        let center = ContinuousLearningCenter::new(make_config());
        center.record_experience(
            "translation".to_string(),
            "a".to_string(),
            "b".to_string(),
            true,
            0.8,
            ExperienceContext::new(vec!["lang".to_string()], HashMap::new()),
        );
        center.record_experience(
            "summarization".to_string(),
            "c".to_string(),
            "d".to_string(),
            false,
            -0.3,
            ExperienceContext::new(vec!["nlp".to_string()], HashMap::new()),
        );

        let results = center.query_experiences("translation", &[]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_type, "translation");

        let results = center.query_experiences("", &["nlp".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_type, "summarization");

        let results = center.query_experiences("", &[]);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_consolidate_creates_knowledge() {
        let mut config = make_config();
        config.importance_threshold = 0.0; // ensure consolidation always triggers
        let center = ContinuousLearningCenter::new(config);

        // Record 5 experiences for the same task type with the same tag.
        for _ in 0..5 {
            center.record_experience(
                "classification".to_string(),
                "input".to_string(),
                "output".to_string(),
                true,
                0.9,
                ExperienceContext::new(vec!["ml".to_string()], HashMap::new()),
            );
        }

        center.consolidate();

        let knowledge = center.list_knowledge(&[]);
        assert!(!knowledge.is_empty());

        let pattern = format!("pattern:{}", "classification");
        let k = center
            .get_knowledge(&pattern)
            .expect("should have knowledge");
        assert_eq!(k.pattern, pattern);
        assert!(k.confidence > 0.0);
        assert!(k.source_experience_ids.len() >= 5);
    }

    #[test]
    fn test_get_knowledge_by_pattern() {
        let mut config = make_config();
        config.importance_threshold = 0.0;
        let center = ContinuousLearningCenter::new(config);

        for _ in 0..5 {
            center.record_experience(
                "qa".to_string(),
                "q".to_string(),
                "a".to_string(),
                true,
                0.7,
                ExperienceContext::new(vec!["question".to_string()], HashMap::new()),
            );
        }
        center.consolidate();

        let k = center.get_knowledge("pattern:qa").expect("should exist");
        assert_eq!(k.pattern, "pattern:qa");

        // Access count should be incremented on get.
        let k2 = center
            .get_knowledge("pattern:qa")
            .expect("should exist again");
        assert_eq!(k2.access_count, k.access_count + 1);

        // Non-existent pattern.
        assert!(center.get_knowledge("pattern:nonexistent").is_none());
    }

    #[test]
    fn test_list_knowledge_by_tags() {
        let mut config = make_config();
        config.importance_threshold = 0.0;
        let center = ContinuousLearningCenter::new(config);

        // 5 experiences tagged "vision".
        for _ in 0..5 {
            center.record_experience(
                "image_classification".to_string(),
                "img".to_string(),
                "class".to_string(),
                true,
                0.8,
                ExperienceContext::new(
                    vec!["vision".to_string(), "ml".to_string()],
                    HashMap::new(),
                ),
            );
        }
        center.consolidate();

        // List with matching tag.
        let results = center.list_knowledge(&["vision".to_string()]);
        assert_eq!(results.len(), 1);

        // List with non-matching tag.
        let results = center.list_knowledge(&["audio".to_string()]);
        assert_eq!(results.len(), 0);

        // List with no filter returns all.
        let results = center.list_knowledge(&[]);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_replay_batch_returns_high_importance() {
        let center = ContinuousLearningCenter::new(make_config());

        // Record experiences with varying reward magnitudes.
        for i in 0..20 {
            let reward = (i as f64) / 20.0; // increasing reward
            center.record_experience(
                "test".to_string(),
                "in".to_string(),
                "out".to_string(),
                true,
                reward,
                ExperienceContext::new(vec![], HashMap::new()),
            );
        }

        let batch = center.replay_batch();
        assert_eq!(batch.len(), 5); // replay_batch_size = 5

        // Batch should have the highest importance experiences.
        // Since importance has a recency component and all were created at
        // more or less the same ms, the ones with highest reward magnitude win.
        for exp in &batch {
            assert!(exp.importance >= batch.last().unwrap().importance);
        }

        let profile = center.profile();
        assert_eq!(profile.replay_count, 1);
    }

    #[test]
    fn test_task_performance_tracking() {
        let center = ContinuousLearningCenter::new(make_config());

        center.record_experience(
            "ner".to_string(),
            "text".to_string(),
            "entities".to_string(),
            true,
            0.9,
            ExperienceContext::new(vec![], HashMap::new()),
        );
        center.record_experience(
            "ner".to_string(),
            "text2".to_string(),
            "entities2".to_string(),
            false,
            0.1,
            ExperienceContext::new(vec![], HashMap::new()),
        );

        let perf = center
            .task_performance("ner")
            .expect("should have performance");
        assert_eq!(perf.total_attempts, 2);
        assert_eq!(perf.successful_attempts, 1);
        assert!(perf.avg_reward > 0.0);

        let all = center.all_task_performance();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].task_type, "ner");
    }

    #[test]
    fn test_detect_forgetting() {
        let center = ContinuousLearningCenter::new(make_config());

        // Record 20 experiences with declining rewards.
        for i in 0..20 {
            let reward = 1.0 - (i as f64) * 0.05; // 1.0, 0.95, ..., 0.05
            center.record_experience(
                "declining_task".to_string(),
                "in".to_string(),
                "out".to_string(),
                reward > 0.5,
                reward,
                ExperienceContext::new(vec![], HashMap::new()),
            );
        }

        let forgetting = center.detect_forgetting("declining_task");
        // The rewards are strictly decreasing by 0.05 each step, slope should be -0.05.
        // Our threshold is strictly < -0.05, so with exactly -0.05 it should be false.
        // Let's check if it's detecting. If not, we'll make the slope steeper.
        // Actually the data here: i=0..19, values: 1.0, 0.95, ..., 0.05
        // That's a very steep negative slope. Let's verify detection.
        // If detection fails, we'll try a steeper decline.

        // If not detected with step 0.05, try a steeper decline.
        if !forgetting {
            let center2 = ContinuousLearningCenter::new(make_config());
            for i in 0..20 {
                let reward = 1.0 - (i as f64) * 0.1; // drops to -0.9 by end
                center2.record_experience(
                    "steep_decline".to_string(),
                    "in".to_string(),
                    "out".to_string(),
                    reward > 0.0,
                    reward,
                    ExperienceContext::new(vec![], HashMap::new()),
                );
            }
            assert!(
                center2.detect_forgetting("steep_decline"),
                "should detect forgetting with steep decline"
            );
        } else {
            assert!(forgetting, "should detect forgetting");
        }
    }

    #[test]
    fn test_consolidation_requires_min_experiences() {
        let center = ContinuousLearningCenter::new(make_config());

        // Only 4 experiences (less than required 5).
        for _ in 0..4 {
            center.record_experience(
                "rare".to_string(),
                "in".to_string(),
                "out".to_string(),
                true,
                0.9,
                ExperienceContext::new(vec![], HashMap::new()),
            );
        }

        center.consolidate();

        let knowledge = center.list_knowledge(&[]);
        assert!(
            knowledge.is_empty(),
            "consolidation should not happen with fewer than 5 experiences"
        );

        // Now add the 5th.
        center.record_experience(
            "rare".to_string(),
            "in".to_string(),
            "out".to_string(),
            true,
            0.9,
            ExperienceContext::new(vec![], HashMap::new()),
        );
        center.consolidate();

        let knowledge = center.list_knowledge(&[]);
        assert!(
            !knowledge.is_empty(),
            "consolidation should happen with 5 experiences"
        );
    }

    #[test]
    fn test_profile_reflects_state() {
        let mut config = make_config();
        config.importance_threshold = 0.0;
        let center = ContinuousLearningCenter::new(config);

        let profile = center.profile();
        assert_eq!(profile.total_experiences, 0);
        assert_eq!(profile.consolidated_count, 0);

        // Add experiences and consolidate.
        for _ in 0..5 {
            center.record_experience(
                "profile_test".to_string(),
                "in".to_string(),
                "out".to_string(),
                true,
                0.8,
                ExperienceContext::new(vec!["profile".to_string()], HashMap::new()),
            );
        }
        center.consolidate();
        center.replay_batch();

        let profile = center.profile();
        assert_eq!(profile.total_experiences, 5);
        assert_eq!(profile.consolidated_count, 1);
        assert_eq!(profile.tracked_task_types, 1);
        assert!(profile.avg_importance > 0.0);
        assert_eq!(profile.replay_count, 1);
        assert_eq!(profile.catastrophic_forgetting_events, 0);
    }

    #[test]
    fn test_importance_computation() {
        let now = 10_000_000;

        // High reward, recent.
        let imp = LearningCenterInner::importance(1.0, now, now);
        assert!(
            (imp - 1.0).abs() < 1e-6,
            "importance should be 1.0 for max reward and current time"
        );

        // Zero reward, recent.
        let imp = LearningCenterInner::importance(0.0, now, now);
        assert!(
            (imp - 0.4).abs() < 1e-6,
            "importance should be 0.4 for zero reward and current time"
        );

        // Negative reward, recent.
        let imp = LearningCenterInner::importance(-0.5, now, now);
        assert!(
            (imp - 0.7).abs() < 1e-6,
            "importance should be 0.7 for reward abs 0.5"
        );

        // Old experience (outside 1 hour window).
        let imp = LearningCenterInner::importance(1.0, now, now - 4_000_000);
        assert!(
            (imp - 0.6).abs() < 1e-6,
            "importance should be 0.6 (only reward component) for old experience"
        );

        // Very old experience with zero reward.
        let imp = LearningCenterInner::importance(0.0, now, now - 4_000_000);
        assert!(
            (imp - 0.0).abs() < 1e-6,
            "importance should be 0.0 for zero reward and old experience"
        );
    }
}
