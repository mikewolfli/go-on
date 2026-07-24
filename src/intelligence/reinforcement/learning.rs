//! Learning module — feedback collection, pattern analysis, Q-learning, and knowledge base.
//!
//! Extracted from the original monolithic `reinforcement.rs`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::pua::{append_learning_record, LearningRecord};

use super::health::now_ts;
use super::ArtifactLedger;

// ── Learning events ────────────────────────────────────────────────────────

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

// ── Learning feedback system ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningPattern {
    pub key: String,
    pub sample_count: usize,
    pub success_rate: f64,
    pub avg_speedup: f64,
}

#[derive(Debug, Clone)]
pub struct LearningFeedbackSystem {
    pub events: Vec<WorkflowLearningEvent>,
    pub storage_path: PathBuf,
}

impl LearningFeedbackSystem {
    pub fn new(storage_path: PathBuf) -> Self {
        Self {
            events: Vec::new(),
            storage_path,
        }
    }

    pub fn collect(&mut self, event: WorkflowLearningEvent) {
        self.events.push(event.clone());
        let _ = self.persist_event(&event);
    }

    pub fn analyze_patterns(&self, window: usize) -> Vec<LearningPattern> {
        let window = window.max(1);
        let mut grouped: std::collections::HashMap<String, (usize, usize, f64)> =
            std::collections::HashMap::new();

        let recent = self
            .events
            .iter()
            .rev()
            .take(window)
            .cloned()
            .collect::<Vec<_>>();
        for event in recent {
            let key = format!("{}::{}", event.source, event.executor);
            let entry = grouped.entry(key).or_insert((0, 0, 0.0));
            entry.0 += 1;
            if event.subtasks_failed == 0 {
                entry.1 += 1;
            }
            entry.2 += event.parallel_speedup;
        }

        if let Ok(records) = load_learning_records(&self.storage_path, window * 4) {
            for record in records {
                match record {
                    LearningRecord::Workflow(event) => {
                        let Ok(event) = serde_json::from_value::<WorkflowLearningEvent>(event)
                        else {
                            continue;
                        };
                        let key = format!("{}::{}", event.source, event.executor);
                        let entry = grouped.entry(key).or_insert((0, 0, 0.0));
                        entry.0 += 1;
                        if event.subtasks_failed == 0 {
                            entry.1 += 1;
                        }
                        entry.2 += event.parallel_speedup;
                    }
                    LearningRecord::Pua(record) => {
                        let key = format!("pua::{}", record.stage);
                        let entry = grouped.entry(key).or_insert((0, 0, 0.0));
                        entry.0 += 1;
                        if record.passed {
                            entry.1 += 1;
                        }
                    }
                }
            }
        }

        grouped
            .into_iter()
            .map(
                |(key, (sample_count, success_count, speedup_sum))| LearningPattern {
                    key,
                    sample_count,
                    success_rate: success_count as f64 / sample_count.max(1) as f64,
                    avg_speedup: speedup_sum / sample_count.max(1) as f64,
                },
            )
            .collect()
    }

    pub fn extract_insights(&self) -> Vec<KnowledgeInsightArtifact> {
        self.events
            .iter()
            .filter(|event| event.subtasks_failed == 0 && event.predicted_success_rate >= 0.7)
            .map(|event| KnowledgeInsightArtifact {
                generated_at: now_ts(),
                conversation_id: format!("learning-{}", event.generated_at),
                branch_id: "main".to_string(),
                phase: "execution".to_string(),
                task: event.task.clone(),
                agent: event.executor.clone(),
                source: event.source.clone(),
                request_excerpt: trim_chars(&event.task, 160),
                response_excerpt: format!(
                    "success_rate={:.2}, speedup={:.2}",
                    event.predicted_success_rate, event.parallel_speedup
                ),
                reusable_insights: vec![
                    "Prefer historical successful execution template".to_string()
                ],
                verification_steps: vec!["Replay with same constraints".to_string()],
                confidence: event.predicted_success_rate as f64,
            })
            .collect()
    }

    fn persist_event(&self, event: &WorkflowLearningEvent) -> Result<()> {
        std::fs::create_dir_all(&self.storage_path)?;
        let filename = format!("event-{}-{}.json", event.generated_at, self.events.len());
        let path = self.storage_path.join(filename);
        let payload = serde_json::to_string_pretty(event)?;
        std::fs::write(path, payload)?;
        let workflow_value =
            serde_json::to_value(event).context("serialize workflow event for learning record")?;
        append_learning_record(
            &self.storage_path,
            &LearningRecord::Workflow(workflow_value),
        )
        .context("persist workflow learning record")?;
        Ok(())
    }
}

fn load_learning_records(
    storage_path: &std::path::Path,
    limit: usize,
) -> Result<Vec<LearningRecord>> {
    use std::io::BufRead;
    let file_path = storage_path.join(crate::pua::LEARNING_RECORDS_FILE);
    let file = match std::fs::File::open(&file_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let reader = std::io::BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines().take(limit) {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<LearningRecord>(&line) {
            records.push(record);
        }
    }
    Ok(records)
}

fn trim_chars(text: &str, max_chars: usize) -> String {
    let mut result = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        result.push_str("...");
    }
    result
}

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
