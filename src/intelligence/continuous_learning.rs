//! BLUE38 F-GAP-24: Continuous Learning Center
//!
//! A thread-safe module that prevents catastrophic forgetting and manages
//! lifelong learning through memory consolidation, forgetting-curve tracking,
//! curriculum scheduling, and experience replay.
//!
//! All mutable state is guarded behind `Arc<Mutex<>>`.

use crate::i18n::{t, tf};
use crate::intelligence::now_ms;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Lock a Mutex, recovering from poison by clearing the mutex data and logging a warning.
///
/// NOTE: This version is intentionally distinct from the shared
/// `crate::intelligence::lock_guard` because it requires `T: Default`
/// and resets the value to its default on poison.
fn lock_guard<T: Default>(mtx: &Mutex<T>) -> MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("continuous_learning mutex poisoned, recovering by clearing state");
            let mut guard = poisoned.into_inner();
            *guard = T::default();
            guard
        }
    }
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// The category of a learning task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LearningTaskType {
    /// Learning from labeled data with teacher signals.
    Supervised,
    /// Learning through trial-and-error with reward feedback.
    Reinforcement,
    /// Learning by mimicking expert demonstrations.
    Imitation,
    /// Applying knowledge from a source domain to a target domain.
    Transfer,
    /// Actively selecting the most informative data to learn from.
    Active,
}

/// The lifecycle status of a learning task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LearningStatus {
    /// Task is queued and waiting to begin.
    Pending,
    /// Task is currently being processed.
    Active,
    /// Task finished successfully.
    Completed,
    /// Task terminated with an error.
    Failed,
    /// Task has been archived and is no longer active.
    Archived,
}

// ---------------------------------------------------------------------------
// Core data structures
// ---------------------------------------------------------------------------

/// A discrete learning task submitted to the continuous learning center.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningTask {
    /// Unique identifier for this task.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The category of learning this task belongs to.
    pub task_type: LearningTaskType,
    /// Epoch millisecond timestamp when the task was created.
    pub created_ms: u64,
    /// Priority from 0 (lowest) to 10 (highest).
    pub priority: u8,
    /// Current lifecycle status.
    pub status: LearningStatus,
}

/// A consolidated memory that the system retains for future reuse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidatedMemory {
    /// Unique identifier for this memory.
    pub id: String,
    /// A key used to group or query related memories.
    pub pattern_key: String,
    /// The serialised content of the memory.
    pub data: String,
    /// A measure of how important this memory is (0.0 – 1.0).
    pub importance: f64,
    /// Epoch millisecond when consolidation happened.
    pub consolidated_ms: u64,
    /// How many times this memory has been accessed.
    pub access_count: u64,
    /// Epoch millisecond of the last access.
    pub last_accessed_ms: u64,
}

/// The forgetting curve for a given memory, modelling strength decay over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgettingCurve {
    /// The memory this curve belongs to.
    pub memory_id: String,
    /// The strength immediately after consolidation.
    pub original_strength: f64,
    /// The strength at the current time (decayed).
    pub current_strength: f64,
    /// Epoch millisecond when the memory was last reinforced.
    pub last_reinforced_ms: u64,
    /// The exponential decay rate (per hour).
    pub decay_rate: f64,
}

/// Tracks consecutive low retention scores for forgetting risk detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgettingRiskRecord {
    /// The memory ID this risk record belongs to.
    pub memory_id: String,
    /// Number of consecutive checks where retention_score < 0.1.
    pub consecutive_critical: u32,
    /// Number of consecutive checks where retention_score < 0.3 (but >= 0.1).
    pub consecutive_low: u32,
    /// The last recorded retention score.
    pub last_score: f64,
    /// Unix millisecond timestamp of the last assessment.
    pub last_assessed_ms: u64,
    /// Whether this memory has been flagged for fast eviction.
    pub flagged_for_eviction: bool,
}

impl ForgettingRiskRecord {
    fn new(memory_id: String) -> Self {
        Self {
            memory_id,
            consecutive_critical: 0,
            consecutive_low: 0,
            last_score: 1.0,
            last_assessed_ms: now_ms(),
            flagged_for_eviction: false,
        }
    }
}

/// A stage in the curriculum schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurriculumStage {
    /// Sequential stage number (0-based).
    pub stage: u32,
    /// Human-readable stage name.
    pub name: String,
    /// Difficulty level of this stage (0.0 – 1.0).
    pub difficulty: f64,
    /// How many tasks in this stage have been completed.
    pub tasks_completed: u32,
    /// The mastery threshold required to advance (0.0 – 1.0).
    pub mastery_threshold: f64,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the `ContinuousLearningCenter`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuousLearningConfig {
    /// Maximum number of consolidated memories retained.
    pub max_memories: usize,
    /// Maximum number of learning tasks tracked at once.
    pub max_tasks: usize,
    /// Default decay rate (per hour) for the forgetting curve.
    pub default_decay_rate: f64,
    /// Minimum importance threshold for memory retention.
    pub min_retention_importance: f64,
    /// Number of curriculum stages.
    pub curriculum_stages: u32,
    /// Tasks needed per curriculum stage.
    pub tasks_per_stage: u32,
}

impl Default for ContinuousLearningConfig {
    fn default() -> Self {
        Self {
            max_memories: 5000,
            max_tasks: 1000,
            default_decay_rate: 0.05,
            min_retention_importance: 0.1,
            curriculum_stages: 5,
            tasks_per_stage: 10,
        }
    }
}

// ---------------------------------------------------------------------------
// Profile (read-only snapshot)
// ---------------------------------------------------------------------------

/// A snapshot of the centre's current state, useful for monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuousLearningProfile {
    /// Number of tasks currently tracked.
    pub total_tasks: usize,
    /// Number of tasks pending execution.
    pub pending_tasks: usize,
    /// Number of tasks currently active.
    pub active_tasks: usize,
    /// Number of completed tasks.
    pub completed_tasks: usize,
    /// Number of failed tasks.
    pub failed_tasks: usize,
    /// Number of archived tasks.
    pub archived_tasks: usize,
    /// Number of consolidated memories.
    pub total_memories: usize,
    /// Current curriculum stage index.
    pub current_stage: u32,
    /// Number of tasks completed in the current stage.
    pub current_stage_tasks_done: u32,
    /// Tasks per stage from config.
    pub tasks_per_stage: u32,
}

// ---------------------------------------------------------------------------
// Continuous Learning Center
// ---------------------------------------------------------------------------

/// The central coordinator for lifelong learning, guarding task management,
/// memory consolidation, forgetting-curve tracking, and curriculum scheduling
/// behind a thread-safe `Arc<Mutex<>>`.
#[derive(Debug, Clone)]
pub struct ContinuousLearningCenter {
    config: ContinuousLearningConfig,
    state: Arc<Mutex<CenterState>>,
}

/// Internal mutable state held by the centre.
#[derive(Debug, Default)]
struct CenterState {
    tasks: HashMap<String, LearningTask>,
    memories: HashMap<String, ConsolidatedMemory>,
    forgetting_curves: HashMap<String, ForgettingCurve>,
    forgetting_risks: HashMap<String, ForgettingRiskRecord>,
    curriculum: Vec<CurriculumStage>,
    next_task_id: u64,
    next_memory_id: u64,
}

impl ContinuousLearningCenter {
    /// Creates a new centre with the given configuration.
    pub fn new(config: ContinuousLearningConfig) -> Self {
        let curriculum = (0..config.curriculum_stages)
            .map(|stage| CurriculumStage {
                stage,
                name: tf(
                    "status.continuous_learning.stage_format",
                    &[("number", &(stage + 1).to_string())],
                ),
                difficulty: (stage as f64 + 1.0) / config.curriculum_stages as f64,
                tasks_completed: 0,
                mastery_threshold: 0.8,
            })
            .collect();

        Self {
            config,
            state: Arc::new(Mutex::new(CenterState {
                tasks: HashMap::new(),
                memories: HashMap::new(),
                forgetting_curves: HashMap::new(),
                forgetting_risks: HashMap::new(),
                curriculum,
                next_task_id: 1,
                next_memory_id: 1,
            })),
        }
    }

    // ── Task management ────────────────────────────────────────────────────

    /// Submits a new learning task and returns its generated ID.
    pub fn submit_task(
        &self,
        name: &str,
        task_type: LearningTaskType,
        priority: u8,
    ) -> Result<String> {
        if priority > 10 {
            bail!(
                "{}",
                tf(
                    "error.continuous_learning.priority_out_of_range",
                    &[("value", &priority.to_string())]
                )
            );
        }
        let mut state = lock_guard(&self.state);
        // Evict the oldest (lowest priority) task when at capacity.
        if state.tasks.len() >= self.config.max_tasks {
            if let Some(oldest_id) = state
                .tasks
                .iter()
                .min_by_key(|(_, t)| (t.priority, t.created_ms))
                .map(|(id, _)| id.clone())
            {
                state.tasks.remove(&oldest_id);
            }
        }

        let id = format!("task-{}", state.next_task_id);
        state.next_task_id += 1;

        let task = LearningTask {
            id: id.clone(),
            name: name.to_string(),
            task_type,
            created_ms: now_ms(),
            priority,
            status: LearningStatus::Pending,
        };
        state.tasks.insert(id.clone(), task);
        Ok(id)
    }

    /// Updates the status of an existing task.
    pub fn update_task_status(&self, task_id: &str, status: LearningStatus) -> Result<()> {
        let was_completed = status == LearningStatus::Completed;
        let mut state = lock_guard(&self.state);
        let task = state.tasks.get_mut(task_id).with_context(|| {
            tf(
                "error.continuous_learning.task_not_found",
                &[("id", task_id)],
            )
        })?;
        task.status = status;

        // If the task completed, advance the curriculum.
        if was_completed {
            if let Some(stage) = state.curriculum.first_mut() {
                stage.tasks_completed += 1;
            }
        }
        Ok(())
    }

    // ── Memory consolidation ───────────────────────────────────────────────

    /// Consolidates a new experience into memory and returns its generated ID.
    ///
    /// This also creates a forgetting curve entry for the new memory.
    pub fn consolidate_experience(
        &self,
        pattern_key: &str,
        data: &str,
        importance: f64,
    ) -> Result<String> {
        let importance = importance.clamp(0.0, 1.0);
        let mut state = lock_guard(&self.state);
        // Evict the least-important memory when at capacity.
        if state.memories.len() >= self.config.max_memories {
            if let Some(oldest_id) = state
                .memories
                .iter()
                .min_by(|(_, a), (_, b)| {
                    let importance_cmp = a
                        .importance
                        .partial_cmp(&b.importance)
                        .unwrap_or(std::cmp::Ordering::Equal);
                    if importance_cmp != std::cmp::Ordering::Equal {
                        importance_cmp
                    } else {
                        a.consolidated_ms.cmp(&b.consolidated_ms)
                    }
                })
                .map(|(id, _)| id.clone())
            {
                state.memories.remove(&oldest_id);
                state.forgetting_curves.remove(&oldest_id);
            }
        }

        let id = format!("mem-{}", state.next_memory_id);
        state.next_memory_id += 1;

        let now = now_ms();
        let memory = ConsolidatedMemory {
            id: id.clone(),
            pattern_key: pattern_key.to_string(),
            data: data.to_string(),
            importance,
            consolidated_ms: now,
            access_count: 0,
            last_accessed_ms: now,
        };

        // Create the forgetting curve for this memory.
        let curve = ForgettingCurve {
            memory_id: id.clone(),
            original_strength: importance,
            current_strength: importance,
            last_reinforced_ms: now,
            decay_rate: self.config.default_decay_rate,
        };

        state.memories.insert(id.clone(), memory);
        state.forgetting_curves.insert(id.clone(), curve);
        Ok(id)
    }

    /// Reinforces a memory by resetting its forgetting curve strength.
    pub fn reinforce_memory(&self, memory_id: &str) -> Result<()> {
        let mut state = lock_guard(&self.state);

        let curve = state
            .forgetting_curves
            .get_mut(memory_id)
            .with_context(|| {
                tf(
                    "error.continuous_learning.memory_not_found",
                    &[("id", memory_id)],
                )
            })?;

        let now = now_ms();
        curve.current_strength = curve.original_strength;
        curve.last_reinforced_ms = now;

        // Update the memory's access stats in a separate borrow scope.
        if let Some(memory) = state.memories.get_mut(memory_id) {
            memory.access_count += 1;
            memory.last_accessed_ms = now;
        }

        Ok(())
    }

    // ── Query ──────────────────────────────────────────────────────────────

    /// Returns memories matching the given pattern key with at least the
    /// specified minimum importance.
    pub fn query_memories(
        &self,
        pattern_key: &str,
        min_importance: f64,
    ) -> Vec<ConsolidatedMemory> {
        let state = lock_guard(&self.state);
        let mut results: Vec<_> = state
            .memories
            .values()
            .filter(|m| m.pattern_key == pattern_key && m.importance >= min_importance)
            .cloned()
            .collect();
        results.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    // ── Forgetting detection ───────────────────────────────────────────────

    /// Detects all memories whose current forgetting-curve strength has
    /// dropped below `min_retention_importance` and returns them.
    pub fn detect_forgetting(&self) -> Vec<ForgettingCurve> {
        let state = lock_guard(&self.state);
        let now = now_ms();
        state
            .forgetting_curves
            .values()
            .filter(|curve| {
                let elapsed_ms = now.saturating_sub(curve.last_reinforced_ms);
                let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
                let strength = curve.original_strength * (-curve.decay_rate * elapsed_hours).exp();
                strength < self.config.min_retention_importance
            })
            .cloned()
            .collect()
    }

    // ── Curriculum ─────────────────────────────────────────────────────────

    /// Returns the current curriculum stage, advancing to the next stage if
    /// the current one has reached the mastery threshold.
    pub fn apply_curriculum(&self) -> Result<CurriculumStage> {
        let mut state = lock_guard(&self.state);
        if state.curriculum.is_empty() {
            // All stages completed; return a terminal stage.
            return Ok(CurriculumStage {
                stage: self.config.curriculum_stages,
                name: t("status.continuous_learning.stage_completed"),
                difficulty: 1.0,
                tasks_completed: self.config.tasks_per_stage,
                mastery_threshold: 1.0,
            });
        }

        let current = &state.curriculum[0];
        if current.tasks_completed >= self.config.tasks_per_stage {
            // Advance to the next stage.
            state.curriculum.remove(0);
            if state.curriculum.is_empty() {
                return Ok(CurriculumStage {
                    stage: self.config.curriculum_stages,
                    name: t("status.continuous_learning.stage_completed"),
                    difficulty: 1.0,
                    tasks_completed: self.config.tasks_per_stage,
                    mastery_threshold: 1.0,
                });
            }
        }

        Ok(state.curriculum[0].clone())
    }

    // ── Experience replay ──────────────────────────────────────────────────

    /// Returns the `count` most important memories for replay (ordered by
    /// importance descending then by last-accessed ascending).
    pub fn replay_important_memories(&self, count: usize) -> Vec<ConsolidatedMemory> {
        let state = lock_guard(&self.state);
        let mut memories: Vec<_> = state.memories.values().cloned().collect();
        // Sort by importance descending, then by last-accessed ascending (LRU bias).
        memories.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.last_accessed_ms.cmp(&b.last_accessed_ms))
        });
        memories.truncate(count);
        memories
    }

    // ── Retention estimation ───────────────────────────────────────────────

    /// Estimates the current retention strength for a given memory using the
    /// exponential forgetting curve:
    ///
    /// `current_strength = original_strength * exp(-decay_rate * elapsed_hours)`
    pub fn estimate_retention(&self, memory_id: &str) -> f64 {
        let state = lock_guard(&self.state);
        match state.forgetting_curves.get(memory_id) {
            Some(curve) => {
                let now = now_ms();
                let elapsed_ms = now.saturating_sub(curve.last_reinforced_ms);
                let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
                curve.original_strength * (-curve.decay_rate * elapsed_hours).exp()
            }
            None => 0.0,
        }
    }

    // ── Retention scoring (GAP-B52-14) ────────────────────────────────────

    /// Compute the retention score for a given memory entry using the
    /// **Ebbinghaus forgetting curve** formula at a given point in time.
    ///
    /// The formula is derived from the exponential decay model:
    ///
    /// ```text
    /// S(t) = I * exp(-d * Δt)
    /// ```
    ///
    /// where:
    /// - `I` = original strength (importance)
    /// - `d` = decay rate (per hour)
    /// - `Δt` = hours elapsed since last reinforcement
    ///
    /// Returns a value in `[0.0, 1.0]`.
    pub fn retention_score(&self, entry: &ConsolidatedMemory, now: u64) -> f64 {
        let state = lock_guard(&self.state);
        match state.forgetting_curves.get(&entry.id) {
            Some(curve) => {
                let elapsed_ms = now.saturating_sub(curve.last_reinforced_ms);
                let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
                // Ebbinghaus: R = e^(-Δt/S) where S = 1/decay_rate (stability)
                // We model as: score = importance * exp(-decay_rate * elapsed_hours)
                entry.importance * (-curve.decay_rate * elapsed_hours).exp()
            }
            None => {
                // No curve → score based purely on recency of consolidation.
                let elapsed_ms = now.saturating_sub(entry.consolidated_ms);
                let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
                let default_decay = self.config.default_decay_rate;
                entry.importance * (-default_decay * elapsed_hours).exp()
            }
        }
    }

    /// Detect memories at risk of being forgotten (retention score < 0.3)
    /// and update their `ForgettingRiskRecord`.
    ///
    /// Memories with score < 0.3 are returned for replay consideration.
    /// Memories with score < 0.1 for 3 consecutive checks are flagged for
    /// fast eviction.
    ///
    /// Returns the list of `ForgettingRiskRecord` entries that are currently
    /// at risk (score < 0.3).
    pub fn detect_forgetting_risk(&self) -> Vec<ForgettingRiskRecord> {
        let mut state = lock_guard(&self.state);
        let now = now_ms();
        let mut at_risk = Vec::new();

        // Collect memory IDs to assess.
        let memory_ids: Vec<String> = state.memories.keys().cloned().collect();

        for id in memory_ids {
            let (importance, consolidated_ms) = match state.memories.get(&id) {
                Some(m) => (m.importance, m.consolidated_ms),
                None => continue,
            };

            // Compute retention score using the Ebbinghaus formula.
            let score = match state.forgetting_curves.get(&id) {
                Some(c) => {
                    let elapsed_ms = now.saturating_sub(c.last_reinforced_ms);
                    let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
                    importance * (-c.decay_rate * elapsed_hours).exp()
                }
                None => {
                    let elapsed_ms = now.saturating_sub(consolidated_ms);
                    let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
                    importance * (-self.config.default_decay_rate * elapsed_hours).exp()
                }
            };

            // Update or create the forgetting risk record.
            let record = state
                .forgetting_risks
                .entry(id.clone())
                .or_insert_with(|| ForgettingRiskRecord::new(id.clone()));

            // Update consecutive counters.
            if score < 0.1 {
                record.consecutive_critical += 1;
                record.consecutive_low = 0;
            } else if score < 0.3 {
                record.consecutive_low += 1;
                record.consecutive_critical = 0;
            } else {
                // Score is healthy; reset counters.
                record.consecutive_critical = 0;
                record.consecutive_low = 0;
            }

            record.last_score = score;
            record.last_assessed_ms = now;

            // Flag for fast eviction if < 0.1 for 3 consecutive checks.
            if record.consecutive_critical >= 3 {
                record.flagged_for_eviction = true;
            }

            // Collect if at risk (score < 0.3).
            if score < 0.3 {
                at_risk.push(record.clone());
            }
        }

        at_risk
    }

    /// Returns the IDs of memories that should be fast-evicted.
    ///
    /// A memory is a candidate for fast eviction when its retention score
    /// has been < 0.1 for 3 consecutive assessments.
    pub fn fast_evict_candidates(&self) -> Vec<String> {
        let state = lock_guard(&self.state);
        state
            .forgetting_risks
            .values()
            .filter(|r| r.flagged_for_eviction)
            .map(|r| r.memory_id.clone())
            .collect()
    }

    /// Schedule a review for a memory entry, returning the `Instant` at which
    /// the review should occur.
    ///
    /// The review interval is calculated from the retention score:
    /// - Score < 0.1 → review immediately (now)
    /// - Score < 0.3 → review within 1 hour
    /// - Score < 0.5 → review within 6 hours
    /// - Score >= 0.5 → review within 24 hours
    ///
    /// This uses an **Expanding Retrieval Practice** schedule similar to
    /// SuperMemo / Anki spaced-repetition algorithms.
    pub fn schedule_review(&self, entry: &ConsolidatedMemory) -> Instant {
        let score = self.retention_score(entry, now_ms());
        let delay_secs = if score < 0.1 {
            0
        } else if score < 0.3 {
            3600 // 1 hour
        } else if score < 0.5 {
            21_600 // 6 hours
        } else {
            86_400 // 24 hours
        };
        Instant::now() + Duration::from_secs(delay_secs)
    }

    /// Perform a forgetting review cycle:
    /// 1. Detect forgetting risks.
    /// 2. Replay memories at risk (score < 0.3).
    /// 3. Fast-evict memories with 3 consecutive critical scores.
    /// 4. Reinforce the replayed memories.
    ///
    /// Returns the number of memories replayed and evicted.
    pub fn review_cycle(&self) -> (usize, usize) {
        // Step 1: Detect forgetting risks.
        let at_risk = self.detect_forgetting_risk();

        // Step 2: Replay important at-risk memories.
        let mut replayed = 0usize;
        for record in &at_risk {
            if record.flagged_for_eviction {
                continue; // Will be evicted instead.
            }
            if self.reinforce_memory(&record.memory_id).is_ok() {
                replayed += 1;
            }
        }

        // Step 3: Fast-evict memories flagged for eviction.
        let evict_ids = self.fast_evict_candidates();
        let evicted = evict_ids.len();
        {
            let mut state = lock_guard(&self.state);
            for id in &evict_ids {
                state.memories.remove(id);
                state.forgetting_curves.remove(id);
                state.forgetting_risks.remove(id);
            }
        }

        (replayed, evicted)
    }

    // ── Profile ────────────────────────────────────────────────────────────

    /// Returns a snapshot of the centre's current state.
    pub fn profile(&self) -> ContinuousLearningProfile {
        let state = lock_guard(&self.state);
        let total_tasks = state.tasks.len();
        let pending_tasks = state
            .tasks
            .values()
            .filter(|t| t.status == LearningStatus::Pending)
            .count();
        let active_tasks = state
            .tasks
            .values()
            .filter(|t| t.status == LearningStatus::Active)
            .count();
        let completed_tasks = state
            .tasks
            .values()
            .filter(|t| t.status == LearningStatus::Completed)
            .count();
        let failed_tasks = state
            .tasks
            .values()
            .filter(|t| t.status == LearningStatus::Failed)
            .count();
        let archived_tasks = state
            .tasks
            .values()
            .filter(|t| t.status == LearningStatus::Archived)
            .count();
        let current_stage = state
            .curriculum
            .first()
            .map(|s| s.stage)
            .unwrap_or(self.config.curriculum_stages);
        let current_stage_tasks_done = state
            .curriculum
            .first()
            .map(|s| s.tasks_completed)
            .unwrap_or(self.config.tasks_per_stage);

        ContinuousLearningProfile {
            total_tasks,
            pending_tasks,
            active_tasks,
            completed_tasks,
            failed_tasks,
            archived_tasks,
            total_memories: state.memories.len(),
            current_stage,
            current_stage_tasks_done,
            tasks_per_stage: self.config.tasks_per_stage,
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: builds a default centre for testing.
    fn test_center() -> ContinuousLearningCenter {
        ContinuousLearningCenter::new(ContinuousLearningConfig::default())
    }

    // ── 1. Empty state ────────────────────────────────────────────────────

    #[test]
    fn test_empty_state() {
        let center = test_center();
        let p = center.profile();
        assert_eq!(p.total_tasks, 0);
        assert_eq!(p.total_memories, 0);
        assert_eq!(p.current_stage, 0);
        assert!(center.detect_forgetting().is_empty());
        assert!(center.replay_important_memories(10).is_empty());
    }

    // ── 2. Submit task ─────────────────────────────────────────────────────

    #[test]
    fn test_submit_task() -> Result<()> {
        let center = test_center();
        let id = center.submit_task("test-supervised", LearningTaskType::Supervised, 5)?;
        assert!(id.starts_with("task-"));

        let p = center.profile();
        assert_eq!(p.total_tasks, 1);
        assert_eq!(p.pending_tasks, 1);
        Ok(())
    }

    #[test]
    fn test_submit_task_invalid_priority() {
        let center = test_center();
        let result = center.submit_task("bad", LearningTaskType::Active, 11);
        assert!(result.is_err());
    }

    // ── 3. Update task status ──────────────────────────────────────────────

    #[test]
    fn test_update_task_status() -> Result<()> {
        let center = test_center();
        let id = center.submit_task("update-me", LearningTaskType::Reinforcement, 3)?;

        center.update_task_status(&id, LearningStatus::Active)?;
        let p = center.profile();
        assert_eq!(p.active_tasks, 1);

        center.update_task_status(&id, LearningStatus::Completed)?;
        let p = center.profile();
        assert_eq!(p.completed_tasks, 1);

        Ok(())
    }

    #[test]
    fn test_update_task_status_not_found() {
        let center = test_center();
        let result = center.update_task_status("nonexistent", LearningStatus::Failed);
        assert!(result.is_err());
    }

    // ── 4. Consolidate / Reinforce / Query memories ────────────────────────

    #[test]
    fn test_consolidate_experience() -> Result<()> {
        let center = test_center();
        let mem_id = center.consolidate_experience("pattern-a", "some data", 0.9)?;
        assert!(mem_id.starts_with("mem-"));

        let p = center.profile();
        assert_eq!(p.total_memories, 1);
        Ok(())
    }

    #[test]
    fn test_reinforce_memory() -> Result<()> {
        let center = test_center();
        let mem_id = center.consolidate_experience("pattern-b", "reinforce me", 0.7)?;

        // Before reinforcement, strength should be roughly original.
        let before = center.estimate_retention(&mem_id);
        assert!((before - 0.7).abs() < 0.01 || before <= 0.7);

        center.reinforce_memory(&mem_id)?;
        let after = center.estimate_retention(&mem_id);
        assert!((after - 0.7).abs() < 0.01);

        Ok(())
    }

    #[test]
    fn test_query_memories() -> Result<()> {
        let center = test_center();
        center.consolidate_experience("topic-x", "data high", 0.9)?;
        center.consolidate_experience("topic-x", "data medium", 0.5)?;
        center.consolidate_experience("topic-x", "data low", 0.1)?;

        let results = center.query_memories("topic-x", 0.5);
        assert_eq!(results.len(), 2);
        // Should be ordered by importance descending.
        assert!(results[0].importance >= results[1].importance);
        Ok(())
    }

    // ── 5. Detect forgetting ──────────────────────────────────────────────

    #[test]
    fn test_detect_forgetting() -> Result<()> {
        // Use a config with a high threshold so a fresh memory with low
        // importance will appear to be forgotten.
        let config = ContinuousLearningConfig {
            min_retention_importance: 0.9,
            default_decay_rate: 1.0, // very fast decay
            ..ContinuousLearningConfig::default()
        };
        let center = ContinuousLearningCenter::new(config);
        center.consolidate_experience("fading", "data", 0.3)?;

        let forgotten = center.detect_forgetting();
        // With decay_rate 1.0 and threshold 0.9, the memory (strength 0.3
        // originally) should already be below threshold after 0 hours.
        assert!(
            !forgotten.is_empty(),
            "expected at least one forgotten memory"
        );
        Ok(())
    }

    // ── 6. Curriculum ──────────────────────────────────────────────────────

    #[test]
    fn test_apply_curriculum() -> Result<()> {
        let config = ContinuousLearningConfig {
            tasks_per_stage: 2,
            curriculum_stages: 3,
            ..ContinuousLearningConfig::default()
        };
        let center = ContinuousLearningCenter::new(config);

        // Stage 0 should be current initially.
        let stage = center.apply_curriculum()?;
        assert_eq!(stage.stage, 0);
        assert_eq!(stage.tasks_completed, 0);

        // Complete 2 tasks (advances to stage 1).
        let t1 = center.submit_task("t1", LearningTaskType::Supervised, 1)?;
        let t2 = center.submit_task("t2", LearningTaskType::Supervised, 1)?;
        center.update_task_status(&t1, LearningStatus::Completed)?;
        center.update_task_status(&t2, LearningStatus::Completed)?;

        let stage = center.apply_curriculum()?;
        assert_eq!(stage.stage, 1);

        Ok(())
    }

    // ── 7. Replay ──────────────────────────────────────────────────────────

    #[test]
    fn test_replay_important_memories() -> Result<()> {
        let center = test_center();
        center.consolidate_experience("a", "low", 0.2)?;
        center.consolidate_experience("b", "high", 0.9)?;
        center.consolidate_experience("c", "mid", 0.5)?;

        let replayed = center.replay_important_memories(2);
        assert_eq!(replayed.len(), 2);
        // Should return the two most important.
        assert_eq!(replayed[0].data, "high");
        assert_eq!(replayed[1].data, "mid");
        Ok(())
    }

    // ── 8. Retention estimation ────────────────────────────────────────────

    #[test]
    fn test_estimate_retention() -> Result<()> {
        let center = test_center();
        let mem_id = center.consolidate_experience("ret", "data", 0.8)?;

        // Immediately after consolidation, retention ≈ original strength.
        let retention = center.estimate_retention(&mem_id);
        assert!((retention - 0.8).abs() < 0.01);

        Ok(())
    }

    // ── 9. Forgetting curve decay ──────────────────────────────────────────

    #[test]
    fn test_forgetting_curve_decay_formula() {
        // Verifies: current_strength = original * exp(-decay * elapsed_hours)
        let original: f64 = 1.0;
        let decay_rate: f64 = 0.1;
        let elapsed_hours: f64 = 10.0;
        let strength = original * (-decay_rate * elapsed_hours).exp();
        let expected = (-1.0_f64).exp(); // e^-1 ≈ 0.3679
        assert!((strength - expected).abs() < 0.001);
    }

    // ── 10. Profile ────────────────────────────────────────────────────────

    #[test]
    fn test_profile() -> Result<()> {
        let center = test_center();

        // Submit a variety of tasks with different statuses.
        let _id1 = center.submit_task("pending", LearningTaskType::Supervised, 1)?;
        let id2 = center.submit_task("active", LearningTaskType::Reinforcement, 2)?;
        let id3 = center.submit_task("completed", LearningTaskType::Imitation, 3)?;
        let id4 = center.submit_task("failed", LearningTaskType::Transfer, 4)?;

        center.update_task_status(&id2, LearningStatus::Active)?;
        center.update_task_status(&id3, LearningStatus::Completed)?;
        center.update_task_status(&id4, LearningStatus::Failed)?;

        // Consolidate some memories.
        center.consolidate_experience("k1", "v1", 0.8)?;
        center.consolidate_experience("k2", "v2", 0.6)?;

        let p = center.profile();
        assert_eq!(p.total_tasks, 4);
        assert_eq!(p.pending_tasks, 1);
        assert_eq!(p.active_tasks, 1);
        assert_eq!(p.completed_tasks, 1);
        assert_eq!(p.failed_tasks, 1);
        assert_eq!(p.total_memories, 2);
        assert_eq!(p.current_stage, 0);

        Ok(())
    }
}
