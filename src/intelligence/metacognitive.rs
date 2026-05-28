//! BLUE38 F-GAP-22: Metacognitive Controller (M6 "元认知控制器")
//!
//! Provides a reflection/self-correction loop that monitors execution quality and
//! triggers corrective actions.  All mutable state is guarded behind
//! `Arc<Mutex<>>` for thread-safe concurrent access.

use crate::i18n::{t, tf};
use crate::intelligence::lock_guard;
use crate::intelligence::now_ms;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

// ── Reflection level ────────────────────────────────────────────────────────

/// Depth of reflection applied when assessing execution quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReflectionLevel {
    /// No reflection has been performed.
    None,
    /// Quick surface-level scan of recent observations.
    Surface,
    /// Deeper analysis of execution patterns and root causes.
    Deep,
    /// Critical reflection requiring immediate corrective action.
    Critical,
}

// ── Corrective status ───────────────────────────────────────────────────────

/// Lifecycle status of a proposed corrective action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CorrectiveStatus {
    /// Action proposed but not yet being executed.
    Pending,
    /// Action is currently being executed.
    InProgress,
    /// Action completed successfully.
    Completed,
    /// Action failed during execution.
    Failed,
    /// Action was skipped (e.g. superseded by another action).
    Skipped,
}

// ── Core data structures ────────────────────────────────────────────────────

/// An observation of execution quality at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionObservation {
    /// Unique observation identifier.
    pub id: String,
    /// The task this observation relates to.
    pub task_id: String,
    /// The agent or component that produced the observation.
    pub agent: String,
    /// Type of observation, e.g. "latency_spike", "error", "low_confidence".
    pub observation_type: String,
    /// Severity of the observation (e.g. "low", "medium", "high", "critical").
    pub severity: String,
    /// Human-readable description of what was observed.
    pub description: String,
    /// Unix-millisecond timestamp when the observation was recorded.
    pub timestamp_ms: u64,
    /// Whether the observation has been resolved.
    pub is_resolved: bool,
}

/// A corrective action proposed in response to an observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectiveAction {
    /// Unique action identifier.
    pub id: String,
    /// The observation this action addresses.
    pub observation_id: String,
    /// Type of action, e.g. "retry", "reroute", "escalate", "fallback".
    pub action_type: String,
    /// Human-readable description of the action to take.
    pub description: String,
    /// Current lifecycle status of this action.
    pub status: CorrectiveStatus,
    /// Unix-millisecond timestamp when the action was created.
    pub created_ms: u64,
    /// Unix-millisecond timestamp when the action was resolved (completed /
    /// failed / skipped).  Zero until resolved.
    pub resolved_ms: u64,
}

/// A reflection report summarising observations and actions taken for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionReport {
    /// Unique report identifier.
    pub id: String,
    /// The task the report covers.
    pub task_id: String,
    /// Observations included in this report.
    pub observations: Vec<ExecutionObservation>,
    /// Corrective actions taken for the observations.
    pub actions_taken: Vec<CorrectiveAction>,
    /// Overall assessment text (e.g. "Execution quality degraded due to latency
    /// spikes; two actions triggered, one pending.").
    pub overall_assessment: String,
    /// Aggregate confidence score for the task execution [0.0, 1.0].
    pub confidence_score: f64,
    /// Depth of analysis performed for this report.
    pub reflection_level: ReflectionLevel,
    /// Unix-millisecond timestamp when the report was generated.
    pub created_ms: u64,
}

/// Configuration for the metacognitive controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetacognitiveConfig {
    /// Whether automatic reflection is enabled.
    #[serde(default = "default_enable_auto_reflection")]
    pub enable_auto_reflection: bool,
    /// Minimum number of unresolved observations before auto-reflection.
    #[serde(default = "default_min_observations_for_reflection")]
    pub min_observations_for_reflection: u32,
    /// Maximum number of observations retained in history.
    #[serde(default = "default_max_observations")]
    pub max_observations: usize,
    /// Maximum number of corrective actions retained.
    #[serde(default = "default_max_actions")]
    pub max_actions: usize,
}

fn default_enable_auto_reflection() -> bool {
    true
}
fn default_min_observations_for_reflection() -> u32 {
    3
}
fn default_max_observations() -> usize {
    200
}
fn default_max_actions() -> usize {
    100
}

impl Default for MetacognitiveConfig {
    fn default() -> Self {
        Self {
            enable_auto_reflection: true,
            min_observations_for_reflection: 3,
            max_observations: 200,
            max_actions: 100,
        }
    }
}

/// Runtime metrics snapshot of the metacognitive controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetacognitiveProfile {
    /// Total number of observations recorded.
    pub total_observations: usize,
    /// Number of observations that are still unresolved.
    pub unresolved_observations: usize,
    /// Total number of corrective actions taken (all statuses except Pending).
    pub total_actions_taken: usize,
    /// Number of successful (Completed) actions.
    pub successful_actions: usize,
    /// Total number of reflection reports generated.
    pub total_reports: usize,
    /// Average confidence score across all reports (0.0 if no reports).
    pub avg_confidence: f64,
    /// Action effectiveness ratio (completed / total actions with outcome).
    pub action_effectiveness_ratio: f64,
}

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Inner {
    config: MetacognitiveConfig,
    observations: Vec<ExecutionObservation>,
    actions: Vec<CorrectiveAction>,
    reports: Vec<ReflectionReport>,
    next_id: u64,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Thread-safe controller that monitors execution quality and triggers
/// corrective actions through a reflection/self-correction loop.
#[derive(Debug, Clone)]
pub struct MetacognitiveController {
    inner: Arc<Mutex<Inner>>,
}

impl MetacognitiveController {
    /// Create a new metacognitive controller with the given configuration.
    pub fn new(config: MetacognitiveConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                observations: Vec::new(),
                actions: Vec::new(),
                reports: Vec::new(),
                next_id: 1,
            })),
        }
    }

    // ── Observation management ──────────────────────────────────────────

    /// Record a new execution observation and return its id.
    ///
    /// Old observations beyond `max_observations` are evicted (FIFO).
    pub fn record_observation(
        &self,
        task_id: &str,
        agent: &str,
        observation_type: &str,
        severity: &str,
        description: &str,
    ) -> Result<String> {
        let mut inner = lock_guard(&self.inner);
        let id = format!("obs-{}", inner.next_id);
        inner.next_id += 1;

        let obs = ExecutionObservation {
            id: id.clone(),
            task_id: task_id.to_string(),
            agent: agent.to_string(),
            observation_type: observation_type.to_string(),
            severity: severity.to_string(),
            description: description.to_string(),
            timestamp_ms: now_ms(),
            is_resolved: false,
        };

        inner.observations.push(obs);

        let max = inner.config.max_observations;
        if inner.observations.len() > max {
            let drain_end = inner.observations.len() - max;
            inner.observations.drain(0..drain_end);
        }

        Ok(id)
    }

    /// Get a single observation by id.
    pub fn get_observation(&self, id: &str) -> Result<ExecutionObservation> {
        let inner = lock_guard(&self.inner);
        inner
            .observations
            .iter()
            .find(|o| o.id == id)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.metacognitive.observation_not_found", &[("id", id)])
                )
            })
    }

    /// List all observations, optionally filtered to unresolved ones only.
    pub fn list_observations(&self, unresolved_only: bool) -> Vec<ExecutionObservation> {
        let inner = lock_guard(&self.inner);
        if unresolved_only {
            inner
                .observations
                .iter()
                .filter(|o| !o.is_resolved)
                .cloned()
                .collect()
        } else {
            inner.observations.clone()
        }
    }

    /// Mark an observation as resolved.
    pub fn resolve_observation(&self, id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let obs = inner
            .observations
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.metacognitive.observation_not_found", &[("id", id)])
                )
            })?;
        obs.is_resolved = true;
        Ok(())
    }

    // ── Action management ───────────────────────────────────────────────

    /// Propose a new corrective action for the given observation.
    ///
    /// Returns an error if the observation does not exist.
    /// Old actions beyond `max_actions` are evicted (FIFO).
    pub fn propose_action(
        &self,
        observation_id: &str,
        action_type: &str,
        description: &str,
    ) -> Result<String> {
        let mut inner = lock_guard(&self.inner);

        // Validate that the observation exists.
        if !inner.observations.iter().any(|o| o.id == observation_id) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.metacognitive.observation_not_found",
                    &[("id", observation_id)]
                )
            );
        }

        let id = format!("action-{}", inner.next_id);
        inner.next_id += 1;

        let action = CorrectiveAction {
            id: id.clone(),
            observation_id: observation_id.to_string(),
            action_type: action_type.to_string(),
            description: description.to_string(),
            status: CorrectiveStatus::Pending,
            created_ms: now_ms(),
            resolved_ms: 0,
        };

        inner.actions.push(action);

        let max = inner.config.max_actions;
        if inner.actions.len() > max {
            let drain_end = inner.actions.len() - max;
            inner.actions.drain(0..drain_end);
        }

        Ok(id)
    }

    /// Transition a Pending action to InProgress.
    pub fn execute_action(&self, action_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let action = inner
            .actions
            .iter_mut()
            .find(|a| a.id == action_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.metacognitive.action_not_found", &[("id", action_id)])
                )
            })?;

        if action.status != CorrectiveStatus::Pending {
            anyhow::bail!(
                "{}",
                tf(
                    "error.metacognitive.action_status_pending",
                    &[
                        ("id", action_id),
                        ("status", &format!("{:?}", action.status))
                    ]
                )
            );
        }

        action.status = CorrectiveStatus::InProgress;
        Ok(())
    }

    /// Mark an action as Completed.
    pub fn complete_action(&self, action_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let action = inner
            .actions
            .iter_mut()
            .find(|a| a.id == action_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.metacognitive.action_not_found", &[("id", action_id)])
                )
            })?;

        if action.status != CorrectiveStatus::InProgress {
            anyhow::bail!(
                "{}",
                tf(
                    "error.metacognitive.action_status_in_progress",
                    &[
                        ("id", action_id),
                        ("status", &format!("{:?}", action.status))
                    ]
                )
            );
        }

        action.status = CorrectiveStatus::Completed;
        action.resolved_ms = now_ms();
        Ok(())
    }

    /// Mark an action as Failed with an error reason stored in the description.
    pub fn fail_action(&self, action_id: &str, reason: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let action = inner
            .actions
            .iter_mut()
            .find(|a| a.id == action_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.metacognitive.action_not_found", &[("id", action_id)])
                )
            })?;

        if action.status != CorrectiveStatus::InProgress {
            anyhow::bail!(
                "{}",
                tf(
                    "error.metacognitive.action_status_in_progress",
                    &[
                        ("id", action_id),
                        ("status", &format!("{:?}", action.status))
                    ]
                )
            );
        }

        action.status = CorrectiveStatus::Failed;
        action.resolved_ms = now_ms();
        // Append failure reason to the description for traceability.
        action.description = format!(
            "{} {}",
            action.description,
            tf("error.metacognitive.failed_suffix", &[("reason", reason)])
        );
        Ok(())
    }

    /// Skip a Pending action without executing it.
    pub fn skip_action(&self, action_id: &str) -> Result<()> {
        let mut inner = lock_guard(&self.inner);
        let action = inner
            .actions
            .iter_mut()
            .find(|a| a.id == action_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.metacognitive.action_not_found", &[("id", action_id)])
                )
            })?;

        if action.status != CorrectiveStatus::Pending {
            anyhow::bail!(
                "{}",
                tf(
                    "error.metacognitive.action_status_pending",
                    &[
                        ("id", action_id),
                        ("status", &format!("{:?}", action.status))
                    ]
                )
            );
        }

        action.status = CorrectiveStatus::Skipped;
        action.resolved_ms = now_ms();
        Ok(())
    }

    /// List actions, optionally filtered by status.
    pub fn list_actions(&self, status_filter: Option<CorrectiveStatus>) -> Vec<CorrectiveAction> {
        let inner = lock_guard(&self.inner);
        match status_filter {
            Some(status) => inner
                .actions
                .iter()
                .filter(|a| a.status == status)
                .cloned()
                .collect(),
            None => inner.actions.clone(),
        }
    }

    // ── Reflection reports ──────────────────────────────────────────────

    /// Generate a reflection report for the given task, collecting all
    /// observations and their associated actions.
    ///
    /// The `reflection_level` is auto-detected based on how many observations
    /// and actions exist for the task.
    pub fn generate_reflection_report(&self, task_id: &str) -> Result<String> {
        let mut inner = lock_guard(&self.inner);

        let report_id = format!("report-{}", inner.next_id);
        inner.next_id += 1;

        // Collect observations for this task.
        let task_observations: Vec<ExecutionObservation> = inner
            .observations
            .iter()
            .filter(|o| o.task_id == task_id)
            .cloned()
            .collect();

        // Collect actions tied to those observations.
        let obs_ids: std::collections::HashSet<&str> =
            task_observations.iter().map(|o| o.id.as_str()).collect();
        let task_actions: Vec<CorrectiveAction> = inner
            .actions
            .iter()
            .filter(|a| obs_ids.contains(a.observation_id.as_str()))
            .cloned()
            .collect();

        // Determine reflection level based on volume and severity.
        let num_critical = task_observations
            .iter()
            .filter(|o| o.severity.eq_ignore_ascii_case("critical"))
            .count();
        let reflection_level = if num_critical > 0 {
            ReflectionLevel::Critical
        } else if task_observations.len() >= 10 {
            ReflectionLevel::Deep
        } else if task_observations.len() >= 3 {
            ReflectionLevel::Surface
        } else {
            ReflectionLevel::None
        };

        // Compute average severity-based confidence score.
        // Each observation contributes: 1.0 - penalty(severity)
        //   low      → 0.0 penalty
        //   medium   → 0.2 penalty
        //   high     → 0.4 penalty
        //   critical → 0.7 penalty
        //   unknown  → 0.1 penalty
        let total = task_observations.len();
        let confidence_score = if total == 0 {
            0.0
        } else {
            let sum: f64 = task_observations
                .iter()
                .map(|o| {
                    let penalty = match o.severity.to_lowercase().as_str() {
                        "low" => 0.0,
                        "medium" => 0.2,
                        "high" => 0.4,
                        "critical" => 0.7,
                        _ => 0.1,
                    };
                    f64::max(1.0 - penalty, 0.0)
                })
                .sum();
            sum / total as f64
        };

        // Build an overall assessment sentence.
        let unresolved_count = task_observations.iter().filter(|o| !o.is_resolved).count();
        let action_count = task_actions.len();
        let completed_actions = task_actions
            .iter()
            .filter(|a| a.status == CorrectiveStatus::Completed)
            .count();
        let pending_actions = task_actions
            .iter()
            .filter(|a| a.status == CorrectiveStatus::Pending)
            .count();

        let overall_assessment = tf(
            "status.metacognitive.report_assessment",
            &[
                ("task_id", task_id),
                ("total", &total.to_string()),
                ("unresolved", &unresolved_count.to_string()),
                ("actions", &action_count.to_string()),
                ("completed", &completed_actions.to_string()),
                ("pending", &pending_actions.to_string()),
                ("confidence", &format!("{:.2}", confidence_score)),
            ],
        );

        let report = ReflectionReport {
            id: report_id.clone(),
            task_id: task_id.to_string(),
            observations: task_observations.clone(),
            actions_taken: task_actions.clone(),
            overall_assessment,
            confidence_score,
            reflection_level,
            created_ms: now_ms(),
        };

        inner.reports.push(report);

        // Evict oldest reports beyond max_observations cap.
        let max_reports = inner.config.max_observations;
        while inner.reports.len() > max_reports {
            inner.reports.remove(0);
        }

        Ok(report_id)
    }

    /// Get a single reflection report by id.
    pub fn get_report(&self, id: &str) -> Result<ReflectionReport> {
        let inner = lock_guard(&self.inner);
        inner
            .reports
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.metacognitive.report_not_found", &[("id", id)])
                )
            })
    }

    /// List all generated reflection reports.
    pub fn list_reports(&self) -> Vec<ReflectionReport> {
        let inner = lock_guard(&self.inner);
        inner.reports.clone()
    }

    // ── Auto-reflection ─────────────────────────────────────────────────

    /// Automatically trigger reflection if there are enough unresolved
    /// observations across all tasks (based on `min_observations_for_reflection`
    /// and only when `enable_auto_reflection` is true).
    ///
    /// Returns the list of report ids generated (one per affected task).
    pub fn autoreflect(&self) -> Vec<String> {
        let inner = lock_guard(&self.inner);

        if !inner.config.enable_auto_reflection {
            return Vec::new();
        }

        let unresolved_count = inner.observations.iter().filter(|o| !o.is_resolved).count();

        if (unresolved_count as u32) < inner.config.min_observations_for_reflection {
            return Vec::new();
        }

        // Collect distinct task ids that have at least one unresolved
        // observation.
        let tasks: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            let mut result = Vec::new();
            for obs in &inner.observations {
                if !obs.is_resolved && seen.insert(obs.task_id.clone()) {
                    result.push(obs.task_id.clone());
                }
            }
            result
        };
        // We need to release the lock before calling generate_reflection_report.
        drop(inner);

        let mut report_ids = Vec::new();
        for task_id in tasks {
            if let Ok(id) = self.generate_reflection_report(&task_id) {
                report_ids.push(id);
            }
        }
        report_ids
    }

    // ── BLUE43 Step 8: Actionable insights ──────────────────────────────

    /// Convert high-severity unresolved observations into actionable insight
    /// prompts that can be consumed by the autonomy loop to drive behavior.
    ///
    /// Returns a vector of (action_type, prompt, severity) tuples where
    /// severity is "high" or "critical".
    pub fn get_actionable_insights(&self, task_id: &str) -> Vec<(String, String, String)> {
        let inner = lock_guard(&self.inner);

        let mut insights = Vec::new();
        for obs in &inner.observations {
            if obs.task_id == task_id && !obs.is_resolved {
                let severity_lower = obs.severity.to_ascii_lowercase();
                if severity_lower == "high" || severity_lower == "critical" {
                    let (action_type, prompt) = match obs.observation_type.to_lowercase().as_str() {
                        "latency_spike" | "timeout" => (
                            "adjust_timeout",
                            tf(
                                "status.metacognitive.insight.latency",
                                &[
                                    ("severity", &obs.severity),
                                    ("description", &obs.description),
                                ],
                            ),
                        ),
                        "low_confidence" | "uncertain" => (
                            "request_clarification",
                            tf(
                                "status.metacognitive.insight.clarification",
                                &[
                                    ("severity", &obs.severity),
                                    ("description", &obs.description),
                                ],
                            ),
                        ),
                        "error" | "execution_error" | "tool_failure" => (
                            "fallback_strategy",
                            tf(
                                "status.metacognitive.insight.fallback",
                                &[
                                    ("severity", &obs.severity),
                                    ("description", &obs.description),
                                ],
                            ),
                        ),
                        "reroute_needed" | "agent_switch" => (
                            "reroute",
                            tf(
                                "status.metacognitive.insight.reroute",
                                &[
                                    ("severity", &obs.severity),
                                    ("description", &obs.description),
                                ],
                            ),
                        ),
                        _ => (
                            "review",
                            tf(
                                "status.metacognitive.insight.review",
                                &[
                                    ("severity", &obs.severity),
                                    ("description", &obs.description),
                                ],
                            ),
                        ),
                    };
                    insights.push((action_type.to_string(), prompt, severity_lower));
                }
            }
        }
        insights
    }

    /// Record the outcome of an applied actionable insight for effectiveness tracking.
    /// Returns the action id.
    pub fn record_action_outcome(
        &self,
        action_type: &str,
        observation_id: &str,
        description: &str,
        success: bool,
    ) -> Result<String> {
        let action_id = self.propose_action(observation_id, action_type, description)?;
        self.execute_action(&action_id)?;
        if success {
            self.complete_action(&action_id)?;
        } else {
            self.fail_action(&action_id, &t("status.metacognitive.rl.failed_outcome"))?;
        }
        Ok(action_id)
    }

    /// Get action effectiveness ratio: completed / (completed + failed).
    pub fn action_effectiveness_ratio(&self) -> f64 {
        let inner = lock_guard(&self.inner);
        let completed = inner
            .actions
            .iter()
            .filter(|a| a.status == CorrectiveStatus::Completed)
            .count() as f64;
        let failed = inner
            .actions
            .iter()
            .filter(|a| a.status == CorrectiveStatus::Failed)
            .count() as f64;
        let total = completed + failed;
        if total == 0.0 {
            0.0
        } else {
            completed / total
        }
    }

    // ── Profile ─────────────────────────────────────────────────────────

    /// Return a snapshot of the controller's runtime metrics.
    pub fn profile(&self) -> MetacognitiveProfile {
        let inner = lock_guard(&self.inner);

        let total_observations = inner.observations.len();
        let unresolved_observations = inner.observations.iter().filter(|o| !o.is_resolved).count();

        let total_actions_taken = inner
            .actions
            .iter()
            .filter(|a| a.status != CorrectiveStatus::Pending)
            .count();

        let successful_actions = inner
            .actions
            .iter()
            .filter(|a| a.status == CorrectiveStatus::Completed)
            .count();

        let total_reports = inner.reports.len();
        let avg_confidence = if total_reports == 0 {
            0.0
        } else {
            inner
                .reports
                .iter()
                .map(|r| r.confidence_score)
                .sum::<f64>()
                / total_reports as f64
        };

        // Compute effectiveness ratio from the same locked snapshot to avoid
        // re-locking `inner` (which would deadlock with a non-reentrant Mutex).
        let completed = inner
            .actions
            .iter()
            .filter(|a| a.status == CorrectiveStatus::Completed)
            .count() as f64;
        let failed = inner
            .actions
            .iter()
            .filter(|a| a.status == CorrectiveStatus::Failed)
            .count() as f64;
        let total_outcome_actions = completed + failed;
        let action_effectiveness_ratio = if total_outcome_actions == 0.0 {
            0.0
        } else {
            completed / total_outcome_actions
        };

        MetacognitiveProfile {
            total_observations,
            unresolved_observations,
            total_actions_taken,
            successful_actions,
            total_reports,
            avg_confidence,
            action_effectiveness_ratio,
        }
    }

    // ── RL Feedback ──────────────────────────────────────────────────────────

    /// Generate an RL-oriented reflection report that quantifies how observations
    /// should influence the reward function and exploration strategy.
    ///
    /// Returns (adjusted_reward_multiplier, suggested_exploration_rate, key_insights).
    pub fn reflect_for_rl(&self) -> (f64, f64, Vec<String>) {
        let guard = lock_guard(&self.inner);
        let state = Inner::clone(&guard);
        drop(guard);

        if state.observations.is_empty() {
            return (1.0, 0.1, vec![t("status.metacognitive.rl.no_observations")]);
        }

        let total = state.observations.len();
        let failures = state
            .observations
            .iter()
            .filter(|o| o.severity == "high" || o.severity == "critical")
            .count();
        let success_rate = if total > 0 {
            (total - failures) as f64 / total as f64
        } else {
            1.0
        };

        let mut insights: Vec<String> = Vec::new();

        // Adjust reward multiplier based on recent success rate
        let reward_mult = if success_rate < 0.3 {
            insights.push(tf(
                "status.metacognitive.rl.low_success_rate",
                &[("rate", &format!("{:.2}", success_rate))],
            ));
            0.5 // Decrease reward signal when failing often (focus on exploration)
        } else if success_rate > 0.9 {
            insights.push(tf(
                "status.metacognitive.rl.high_success_rate",
                &[("rate", &format!("{:.2}", success_rate))],
            ));
            1.5 // Increase reward signal when succeeding (focus on exploitation)
        } else {
            1.0
        };

        // Suggest exploration rate based on observation diversity
        let unique_actions: HashSet<&str> = state
            .observations
            .iter()
            .map(|o| o.observation_type.as_str())
            .collect();
        let diversity = unique_actions.len() as f64 / total.max(1) as f64;
        let exploration_rate = if diversity < 0.2 {
            insights.push(tf(
                "status.metacognitive.rl.low_diversity",
                &[("diversity", &format!("{:.2}", diversity))],
            ));
            0.3
        } else if success_rate < 0.4 {
            insights.push(t("status.metacognitive.rl.frequent_failures"));
            0.25
        } else {
            0.1
        };

        // Detect recurring failure patterns
        let mut failure_patterns: HashMap<String, usize> = HashMap::new();
        for obs in &state.observations {
            if obs.severity == "high" || obs.severity == "critical" {
                *failure_patterns
                    .entry(obs.observation_type.clone())
                    .or_insert(0) += 1;
            }
        }
        for (category, count) in failure_patterns.iter().filter(|(_, c)| **c >= 3) {
            insights.push(tf(
                "status.metacognitive.rl.recurring_failure",
                &[("category", category), ("count", &count.to_string())],
            ));
        }

        // Check if corrective actions are addressing root causes
        if state.actions.len() > state.observations.len() / 3 {
            insights.push(tf(
                "status.metacognitive.rl.correction_loop",
                &[
                    ("actions", &state.actions.len().to_string()),
                    ("total", &total.to_string()),
                ],
            ));
        }

        (reward_mult, exploration_rate, insights)
    }

    /// Merge metacognitive insights into a structured feedback payload
    /// that can be sent to the CapabilityBus's evolve() method.
    pub fn generate_evolve_feedback(&self) -> serde_json::Value {
        let (reward_mult, exploration_rate, insights) = self.reflect_for_rl();

        serde_json::json!({
            "source": "metacognitive",
            "reward_multiplier": reward_mult,
            "suggested_exploration_rate": exploration_rate,
            "insights": insights,
            "timestamp_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        })
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> MetacognitiveConfig {
        MetacognitiveConfig {
            enable_auto_reflection: true,
            min_observations_for_reflection: 3,
            max_observations: 200,
            max_actions: 100,
        }
    }

    // ── 1. Fresh controller is empty ─────────────────────────────────────
    #[test]
    fn test_new_controller_empty() {
        let ctrl = MetacognitiveController::new(base_config());
        assert!(ctrl.list_observations(false).is_empty());
        assert!(ctrl.list_actions(None).is_empty());
        assert!(ctrl.list_reports().is_empty());
        let p = ctrl.profile();
        assert_eq!(p.total_observations, 0);
        assert_eq!(p.unresolved_observations, 0);
        assert_eq!(p.total_actions_taken, 0);
        assert_eq!(p.successful_actions, 0);
        assert_eq!(p.total_reports, 0);
        assert!((p.avg_confidence - 0.0).abs() < 1e-9);
    }

    // ── 2. Record an observation and verify fields ──────────────────────
    #[test]
    fn test_record_observation() {
        let ctrl = MetacognitiveController::new(base_config());
        let id = ctrl
            .record_observation(
                "task-1",
                "agent-a",
                "latency_spike",
                "high",
                "Latency exceeded 5s",
            )
            .unwrap();

        assert!(id.starts_with("obs-"));

        let obs = ctrl.get_observation(&id).unwrap();
        assert_eq!(obs.task_id, "task-1");
        assert_eq!(obs.agent, "agent-a");
        assert_eq!(obs.observation_type, "latency_spike");
        assert_eq!(obs.severity, "high");
        assert_eq!(obs.description, "Latency exceeded 5s");
        assert!(!obs.is_resolved);
        assert!(obs.timestamp_ms > 0);

        // Record a second observation to confirm id uniqueness.
        let id2 = ctrl
            .record_observation("task-1", "agent-b", "error", "critical", "Null pointer")
            .unwrap();
        assert_ne!(id, id2);
    }

    // ── 3. List observations with and without unresolved filter ──────────
    #[test]
    fn test_list_observations() {
        let ctrl = MetacognitiveController::new(base_config());
        let id1 = ctrl
            .record_observation("task-1", "agent-a", "latency_spike", "high", "Slow")
            .unwrap();
        let id2 = ctrl
            .record_observation("task-2", "agent-b", "error", "critical", "Crash")
            .unwrap();

        // Both visible in full list.
        assert_eq!(ctrl.list_observations(false).len(), 2);

        // Resolve the first one.
        ctrl.resolve_observation(&id1).unwrap();

        // unresolved_only = true should only return the unresolved id2.
        let unresolved = ctrl.list_observations(true);
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].id, id2);

        // Full list still contains both.
        assert_eq!(ctrl.list_observations(false).len(), 2);
    }

    // ── 4. Resolve an observation ───────────────────────────────────────
    #[test]
    fn test_resolve_observation() {
        let ctrl = MetacognitiveController::new(base_config());
        let id = ctrl
            .record_observation("task-1", "agent-a", "low_confidence", "medium", "Score 0.3")
            .unwrap();

        assert!(!ctrl.get_observation(&id).unwrap().is_resolved);

        ctrl.resolve_observation(&id).unwrap();
        assert!(ctrl.get_observation(&id).unwrap().is_resolved);

        // Resolving a non-existent observation fails.
        assert!(ctrl.resolve_observation("obs-9999").is_err());
    }

    // ── 5. Propose and execute an action ────────────────────────────────
    #[test]
    fn test_propose_and_execute_action() {
        let ctrl = MetacognitiveController::new(base_config());
        let obs_id = ctrl
            .record_observation("task-1", "agent-a", "error", "high", "Timeout")
            .unwrap();

        let action_id = ctrl
            .propose_action(&obs_id, "retry", "Retry the request with backoff")
            .unwrap();
        assert!(action_id.starts_with("action-"));

        // Action starts as Pending.
        let actions = ctrl.list_actions(None);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].status, CorrectiveStatus::Pending);

        // Execute transitions to InProgress.
        ctrl.execute_action(&action_id).unwrap();
        let actions = ctrl.list_actions(Some(CorrectiveStatus::InProgress));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, action_id);

        // Executing an already-executed action fails.
        assert!(ctrl.execute_action(&action_id).is_err());
    }

    // ── 6. Complete an action ───────────────────────────────────────────
    #[test]
    fn test_complete_action() {
        let ctrl = MetacognitiveController::new(base_config());
        let obs_id = ctrl
            .record_observation("task-1", "agent-a", "error", "medium", "Parse failure")
            .unwrap();
        let action_id = ctrl
            .propose_action(&obs_id, "fallback", "Use fallback parser")
            .unwrap();

        // Can't complete a Pending action.
        assert!(ctrl.complete_action(&action_id).is_err());

        ctrl.execute_action(&action_id).unwrap();
        ctrl.complete_action(&action_id).unwrap();

        let action = ctrl
            .list_actions(Some(CorrectiveStatus::Completed))
            .pop()
            .unwrap();
        assert_eq!(action.id, action_id);
        assert!(action.resolved_ms > 0);

        // Completing again fails.
        assert!(ctrl.complete_action(&action_id).is_err());
    }

    // ── 7. Fail an action ───────────────────────────────────────────────
    #[test]
    fn test_fail_action() {
        let ctrl = MetacognitiveController::new(base_config());
        let obs_id = ctrl
            .record_observation("task-1", "agent-a", "error", "high", "Disk full")
            .unwrap();
        let action_id = ctrl
            .propose_action(&obs_id, "retry", "Retry after cleanup")
            .unwrap();

        // Can't fail a Pending action.
        assert!(ctrl.fail_action(&action_id, "timeout").is_err());

        ctrl.execute_action(&action_id).unwrap();
        ctrl.fail_action(&action_id, "out of retries").unwrap();

        let action = ctrl
            .list_actions(Some(CorrectiveStatus::Failed))
            .pop()
            .unwrap();
        assert_eq!(action.id, action_id);
        assert!(action.resolved_ms > 0);
        assert!(
            action.description.contains("out of retries") || action.description.contains("error."),
            "unexpected description: {}",
            action.description
        );

        // Skipping an already failed action fails.
        assert!(ctrl.skip_action(&action_id).is_err());
    }

    // ── 8. List actions by status ───────────────────────────────────────
    #[test]
    fn test_list_actions_by_status() {
        let ctrl = MetacognitiveController::new(base_config());

        // Create three actions with different final statuses.
        let obs = ctrl
            .record_observation("task-1", "a", "error", "low", "E1")
            .unwrap();
        let a1 = ctrl.propose_action(&obs, "retry", "R1").unwrap();
        let a2 = ctrl.propose_action(&obs, "retry", "R2").unwrap();
        let a3 = ctrl.propose_action(&obs, "escalate", "E1").unwrap();

        ctrl.execute_action(&a1).unwrap();
        ctrl.complete_action(&a1).unwrap();

        ctrl.execute_action(&a2).unwrap();
        ctrl.fail_action(&a2, "n/a").unwrap();

        ctrl.skip_action(&a3).unwrap();

        assert_eq!(
            ctrl.list_actions(Some(CorrectiveStatus::Completed)).len(),
            1
        );
        assert_eq!(ctrl.list_actions(Some(CorrectiveStatus::Failed)).len(), 1);
        assert_eq!(ctrl.list_actions(Some(CorrectiveStatus::Skipped)).len(), 1);
        assert_eq!(ctrl.list_actions(Some(CorrectiveStatus::Pending)).len(), 0);
        assert_eq!(ctrl.list_actions(None).len(), 3);
    }

    // ── 9. Generate a reflection report ─────────────────────────────────
    #[test]
    fn test_generate_reflection_report() {
        let ctrl = MetacognitiveController::new(base_config());

        // Record observations for two different tasks.
        ctrl.record_observation("task-1", "agent-a", "latency_spike", "high", "Slow #1")
            .unwrap();
        ctrl.record_observation("task-1", "agent-a", "error", "critical", "Critical error")
            .unwrap();
        ctrl.record_observation("task-2", "agent-b", "low_confidence", "low", "Score 0.6")
            .unwrap();

        let report_id = ctrl.generate_reflection_report("task-1").unwrap();
        assert!(report_id.starts_with("report-"));

        let report = ctrl.get_report(&report_id).unwrap();
        assert_eq!(report.task_id, "task-1");
        assert_eq!(report.observations.len(), 2);
        assert_eq!(report.reflection_level, ReflectionLevel::Critical); // has critical severity
        assert!(report.confidence_score > 0.0);
        assert!(!report.overall_assessment.is_empty());
        assert!(report.created_ms > 0);
    }

    // ── 10. Auto-reflection triggers when enough unresolved obs ──────────
    #[test]
    fn test_autoreflect() {
        // Use config with low threshold so auto-reflection triggers easily.
        let config = MetacognitiveConfig {
            enable_auto_reflection: true,
            min_observations_for_reflection: 2,
            max_observations: 200,
            max_actions: 100,
        };
        let ctrl = MetacognitiveController::new(config);

        // No reports yet.
        assert!(ctrl.list_reports().is_empty());

        // Auto-reflect with zero observations → nothing.
        assert!(ctrl.autoreflect().is_empty());

        // Add one observation (below threshold of 2) → still nothing.
        ctrl.record_observation("task-1", "agent-a", "latency_spike", "medium", "Spike")
            .unwrap();
        assert!(ctrl.autoreflect().is_empty());

        // Add a second observation → should trigger reflection.
        ctrl.record_observation("task-1", "agent-a", "error", "high", "Error")
            .unwrap();
        let ids = ctrl.autoreflect();
        assert_eq!(ids.len(), 1);
        assert_eq!(ctrl.list_reports().len(), 1);

        // Verify the generated report.
        let report = ctrl.get_report(&ids[0]).unwrap();
        assert_eq!(report.task_id, "task-1");
        assert_eq!(report.observations.len(), 2);

        // Auto-reflect again while still having unresolved observations
        // should generate another report for the same task.
        let ids2 = ctrl.autoreflect();
        assert_eq!(ids2.len(), 1);
        assert_eq!(ctrl.list_reports().len(), 2);
    }

    // ── 11. Get report ──────────────────────────────────────────────────
    #[test]
    fn test_get_report() {
        let ctrl = MetacognitiveController::new(base_config());
        ctrl.record_observation("task-1", "a", "error", "low", "Minor")
            .unwrap();

        let id = ctrl.generate_reflection_report("task-1").unwrap();
        let report = ctrl.get_report(&id).unwrap();
        assert_eq!(report.id, id);

        // Non-existent report.
        assert!(ctrl.get_report("report-9999").is_err());
    }

    // ── 12. Profile reflects state accurately ────────────────────────────
    #[test]
    fn test_profile_reflects_state() {
        let ctrl = MetacognitiveController::new(base_config());

        // Record some observations.
        ctrl.record_observation("task-1", "a", "latency_spike", "high", "Slow")
            .unwrap();
        let obs2 = ctrl
            .record_observation("task-1", "b", "error", "critical", "Critical")
            .unwrap();
        ctrl.record_observation("task-2", "c", "low_confidence", "low", "Conf")
            .unwrap();

        // Resolve one.
        ctrl.resolve_observation(&obs2).unwrap();

        // Propose and complete one action.
        let aid = ctrl.propose_action(&obs2, "retry", "Retry").unwrap();
        ctrl.execute_action(&aid).unwrap();
        ctrl.complete_action(&aid).unwrap();

        // Propose and fail another action.
        let obs3 = ctrl
            .record_observation("task-1", "a", "error", "medium", "M")
            .unwrap();
        let aid2 = ctrl.propose_action(&obs3, "fallback", "Fallback").unwrap();
        ctrl.execute_action(&aid2).unwrap();
        ctrl.fail_action(&aid2, "no fallback available").unwrap();

        // Generate two reports.
        ctrl.generate_reflection_report("task-1").unwrap();
        ctrl.generate_reflection_report("task-2").unwrap();

        let p = ctrl.profile();
        assert_eq!(p.total_observations, 4);
        assert_eq!(p.unresolved_observations, 3); // obs2 resolved, the other 3 are not
        assert_eq!(p.total_actions_taken, 2); // both left Pending state
        assert_eq!(p.successful_actions, 1); // one Completed
        assert_eq!(p.total_reports, 2);
        assert!(p.avg_confidence > 0.0);
        // 1 completed / (1 completed + 1 failed) = 0.5
        assert!(
            (p.action_effectiveness_ratio - 0.5).abs() < 0.001,
            "expected action_effectiveness_ratio 0.5, got {}",
            p.action_effectiveness_ratio
        );
        // Also verify direct method matches profile
        assert!(
            (ctrl.action_effectiveness_ratio() - p.action_effectiveness_ratio).abs() < 0.001,
            "direct action_effectiveness_ratio() should match profile"
        );
    }
}
