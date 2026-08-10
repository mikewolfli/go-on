//! Learning module — feedback collection, pattern analysis, Q-learning, and knowledge base.
//!
//! Extracted from the original monolithic `reinforcement.rs`.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::health::now_ts;
use super::ArtifactLedger;

// ── Learning events ────────────────────────────────────────────────────────

/// Workflow learning event persisted to the artifact ledger by the
/// `exec_pack` / knowledge-distillation chain
/// (`persist_workflow_learning_event`).
///
/// A second, same-named `WorkflowLearningEvent` (7-field runtime snapshot)
/// previously existed in `capability_bus::core` and was duplicated 1:1 from
/// [`LearningOptimizationBus`](crate::intelligence::capability_bus::learning_optimization_bus::LearningEvent)
/// in `sense()`; it has been deleted — the capability-bus sensing chain now
/// consumes `LearningEvent` directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowLearningEvent {
    pub generated_at: i64,
    pub task: String,
    pub complexity: u8,
    pub predicted_success_rate: f32,
    pub subtasks_total: usize,
    pub subtasks_completed: usize,
    pub subtasks_failed: usize,
    pub subtasks_skipped: usize,
    pub serial_work_ms: u64,
    pub critical_path_ms: u64,
    pub parallel_speedup: f64,
    pub parallel_efficiency: f64,
    pub executor: String,
    pub source: String,
    #[serde(default)]
    pub runtime_healthy: bool,
    #[serde(default = "default_workflow_learning_gates_ok")]
    pub gates_ok: bool,
    #[serde(default)]
    pub work_grade: String,
    #[serde(default)]
    pub risk_score: f64,
    #[serde(default)]
    pub clarification_rounds: u32,
    #[serde(default)]
    pub clarification_quality_score: f64,
    #[serde(default)]
    pub requirement_change_count: u32,
    #[serde(default)]
    pub review_reject_root_cause: String,
    #[serde(default)]
    pub primary_stability_score: f64,
    #[serde(default)]
    pub secondary_utilization_rate: f64,
    #[serde(default)]
    pub failover_count: u32,
    #[serde(default)]
    pub failover_root_cause: String,
}

fn default_workflow_learning_gates_ok() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowLearningBusArtifact {
    pub generated_at: i64,
    pub total_events: usize,
    pub events: Vec<WorkflowLearningEvent>,
}

// ── Knowledge artifacts ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeInsightArtifact {
    pub generated_at: i64,
    pub conversation_id: String,
    pub branch_id: String,
    pub phase: String,
    pub task: String,
    pub agent: String,
    pub source: String,
    pub request_excerpt: String,
    pub response_excerpt: String,
    pub reusable_insights: Vec<String>,
    pub verification_steps: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBusArtifact {
    pub generated_at: i64,
    pub total_events: usize,
    pub events: Vec<KnowledgeInsightArtifact>,
}

// ── Persist functions for learning events ──────────────────────────────────

/// Persist a workflow learning event to the artifact ledger (with dedup ring buffer).
pub fn persist_workflow_learning_event(
    ledger: &ArtifactLedger,
    event: WorkflowLearningEvent,
    max_events: usize,
) -> Result<PathBuf> {
    ledger.ensure_ready()?;

    let latest_path = ledger.latest_path("spec", "latest-learning.json");
    let mut existing = std::fs::read_to_string(&latest_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<WorkflowLearningBusArtifact>(&raw).ok())
        .unwrap_or(WorkflowLearningBusArtifact {
            generated_at: now_ts(),
            total_events: 0,
            events: Vec::new(),
        });

    existing.events.push(event);
    if existing.events.len() > max_events {
        let overflow = existing.events.len() - max_events;
        existing.events.drain(0..overflow);
    }
    existing.generated_at = now_ts();
    existing.total_events = existing.events.len();

    ledger.write_json("spec", "latest-learning.json", &existing)
}

/// Persist a knowledge insight event to the artifact ledger (with confidence-based dedup).
pub fn persist_knowledge_insight_event(
    ledger: &ArtifactLedger,
    event: KnowledgeInsightArtifact,
    max_events: usize,
) -> Result<PathBuf> {
    ledger.ensure_ready()?;

    let max_events = max_events.max(1);
    let latest_path = ledger.latest_path("spec", "latest-knowledge.json");
    let mut existing = std::fs::read_to_string(&latest_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<KnowledgeBusArtifact>(&raw).ok())
        .unwrap_or(KnowledgeBusArtifact {
            generated_at: now_ts(),
            total_events: 0,
            events: Vec::new(),
        });

    // Dedup + confidence arbitration:
    // For events sharing (task, phase, agent), keep whichever has the higher confidence.
    let existing_pos = existing
        .events
        .iter()
        .position(|e| e.task == event.task && e.phase == event.phase && e.agent == event.agent);
    match existing_pos {
        Some(idx) if existing.events[idx].confidence >= event.confidence => {
            // Existing entry is at least as confident — no change needed.
        }
        Some(idx) => {
            // Incoming event supersedes the existing one.
            existing.events[idx] = event;
        }
        None => {
            // No duplicate — append normally.
            existing.events.push(event);
            if existing.events.len() > max_events {
                let overflow = existing.events.len() - max_events;
                existing.events.drain(0..overflow);
            }
        }
    }

    existing.generated_at = now_ts();
    existing.total_events = existing.events.len();

    ledger.write_json("spec", "latest-knowledge.json", &existing)
}

// ── Reward function (consumed by capability_bus::evolve_q_learning) ────────
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RlTaskExecutionMetrics {
    pub tokens_used: u64,
    pub success: bool,
    pub quality_score: f64,
    pub duration_ms: u64,
}

// ── Reward function ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardFunction {
    pub token_saving_weight: f64,
    pub success_weight: f64,
    pub quality_weight: f64,
    pub speed_weight: f64,
}

impl Default for RewardFunction {
    fn default() -> Self {
        Self {
            token_saving_weight: 0.2,
            success_weight: 0.5,
            quality_weight: 0.2,
            speed_weight: 0.1,
        }
    }
}

impl RewardFunction {
    pub fn calculate(&self, metrics: &RlTaskExecutionMetrics) -> f64 {
        let success_reward = if metrics.success { 1.0 } else { -0.5 };
        let quality_reward = metrics.quality_score;
        let speed_ratio = (metrics.duration_ms as f64 / 5000.0).clamp(0.0, 1.0);
        let speed_reward = 1.0 - speed_ratio;
        let token_saving = (metrics.tokens_used as f64 / 1000.0).clamp(0.0, 1.0);
        let token_reward = 1.0 - token_saving;

        self.success_weight * success_reward
            + self.quality_weight * quality_reward
            + self.speed_weight * speed_reward
            + self.token_saving_weight * token_reward
    }
}
