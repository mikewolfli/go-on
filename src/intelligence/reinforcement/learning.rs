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
    let mut existing = if latest_path.exists() {
        std::fs::read_to_string(&latest_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<WorkflowLearningBusArtifact>(&raw).ok())
            .unwrap_or(WorkflowLearningBusArtifact {
                generated_at: now_ts(),
                total_events: 0,
                events: Vec::new(),
            })
    } else {
        WorkflowLearningBusArtifact {
            generated_at: now_ts(),
            total_events: 0,
            events: Vec::new(),
        }
    };

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
    let mut existing = if latest_path.exists() {
        std::fs::read_to_string(&latest_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<KnowledgeBusArtifact>(&raw).ok())
            .unwrap_or(KnowledgeBusArtifact {
                generated_at: now_ts(),
                total_events: 0,
                events: Vec::new(),
            })
    } else {
        KnowledgeBusArtifact {
            generated_at: now_ts(),
            total_events: 0,
            events: Vec::new(),
        }
    };

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
    if !file_path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(&file_path)?;
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

// ── Experience knowledge base ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessCase {
    pub objective: String,
    pub strategy: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailurePattern {
    pub objective: String,
    pub root_cause: String,
    pub frequency: usize,
}

#[derive(Debug, Default)]
pub struct ExperienceKnowledgeBase {
    pub success_cases: Vec<SuccessCase>,
    pub failure_patterns: Vec<FailurePattern>,
}

impl ExperienceKnowledgeBase {
    pub fn add_success_case(&mut self, case: SuccessCase) {
        self.success_cases.push(case);
    }

    pub fn find_similar(&self, objective: &str) -> Option<&SuccessCase> {
        let objective_lower = objective.to_ascii_lowercase();
        self.success_cases
            .iter()
            .filter(|case| {
                case.objective
                    .to_ascii_lowercase()
                    .contains(&objective_lower)
                    || objective_lower.contains(&case.objective.to_ascii_lowercase())
            })
            .max_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    pub fn top_failure_patterns(&self, n: usize) -> Vec<&FailurePattern> {
        let mut sorted: Vec<_> = self.failure_patterns.iter().collect();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.frequency));
        sorted.into_iter().take(n).collect()
    }
}

// ── Q-Learning agent ───────────────────────────────────────────────────────

use std::collections::{HashMap, VecDeque};

/// Sample type returned by ReplayBuffer::sample.
type ReplaySample = Vec<((String, String), String, f64, (String, String))>;

/// A replay buffer that stores experiences for batch learning.
#[derive(Debug, Clone)]
pub struct ReplayBuffer {
    capacity: usize,
    buffer: VecDeque<(String, String, String, f64, String, String)>,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: VecDeque::with_capacity(capacity.min(10000)),
        }
    }

    pub fn push(
        &mut self,
        state: (&str, &str),
        action: &str,
        reward: f64,
        next_state: (&str, &str),
    ) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back((
            state.0.to_string(),
            state.1.to_string(),
            action.to_string(),
            reward,
            next_state.0.to_string(),
            next_state.1.to_string(),
        ));
    }

    pub fn sample(&self, batch_size: usize) -> ReplaySample {
        let len = self.buffer.len();
        if len == 0 {
            return Vec::new();
        }
        let count = batch_size.min(len);
        // Use hash-based indices for deterministic sampling (no rand dependency)
        let mut indices: Vec<usize> = (0..len).collect();
        // Simple Fisher-Yates partial shuffle using the existing simple_random functions
        for i in (0..count).rev() {
            let j = (simple_random_u64() as usize) % (i + 1);
            indices.swap(i, j);
        }
        indices[..count]
            .iter()
            .map(|&idx| {
                let s = &self.buffer[idx];
                (
                    (s.0.clone(), s.1.clone()),
                    s.2.clone(),
                    s.3,
                    (s.4.clone(), s.5.clone()),
                )
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

impl Default for ReplayBuffer {
    fn default() -> Self {
        Self::new(10000)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RlTaskExecutionMetrics {
    pub tokens_used: u64,
    pub success: bool,
    pub quality_score: f64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QLearningAgent {
    pub q_table: HashMap<(String, String), HashMap<String, f64>>,
    /// Double Q-Learning table for bias reduction
    pub q_table_2: HashMap<(String, String), HashMap<String, f64>>,
    pub learning_rate: f64,
    pub discount_factor: f64,
    pub exploration_rate: f64,
    /// Experience replay buffer
    #[serde(skip)]
    pub replay_buffer: ReplayBuffer,
}

impl Default for QLearningAgent {
    fn default() -> Self {
        Self {
            q_table: HashMap::new(),
            q_table_2: HashMap::new(),
            learning_rate: 0.1,
            discount_factor: 0.9,
            exploration_rate: 1.0,
            replay_buffer: ReplayBuffer::new(10000),
        }
    }
}

impl QLearningAgent {
    pub fn choose_action(&self, state: &(String, String), actions: &[String]) -> Option<String> {
        if self.exploration_rate > simple_random_f64() {
            // Explore: pick a random action using simple hash-based approach
            let idx = (simple_random_u64() as usize) % actions.len();
            actions.get(idx).cloned()
        } else {
            // Exploit: pick action with highest Q-value
            let state_q = self.q_table.get(state);
            actions
                .iter()
                .max_by(|a, b| {
                    let qa = state_q.and_then(|m| m.get(*a)).copied().unwrap_or(0.0);
                    let qb = state_q.and_then(|m| m.get(*b)).copied().unwrap_or(0.0);
                    qa.partial_cmp(&qb).unwrap_or(std::cmp::Ordering::Equal)
                })
                .cloned()
        }
    }

    pub fn update(
        &mut self,
        state: &(String, String),
        action: &str,
        reward: f64,
        next_state: &(String, String),
    ) {
        let current_q = self
            .q_table
            .entry(state.clone())
            .or_default()
            .get(action)
            .copied()
            .unwrap_or(0.0);

        let max_future_q = self
            .q_table
            .get(next_state)
            .and_then(|m| {
                m.values()
                    .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            })
            .copied()
            .unwrap_or(0.0);

        let new_q = current_q
            + self.learning_rate * (reward + self.discount_factor * max_future_q - current_q);

        self.q_table
            .entry(state.clone())
            .or_default()
            .insert(action.to_string(), new_q);
    }

    /// Perform a Double Q-Learning update to reduce overestimation bias.
    /// Randomly updates one of the two Q-tables, using the other for action selection.
    pub fn double_q_update(
        &mut self,
        state: &(String, String),
        action: &str,
        reward: f64,
        next_state: &(String, String),
    ) {
        let alpha = self.learning_rate;
        let gamma = self.discount_factor;

        // Use a simple hash-based coin flip instead of depending on rand
        let coin = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            state.hash(&mut hasher);
            action.hash(&mut hasher);
            hasher.finish().is_multiple_of(2)
        };

        if coin {
            // Update q_table, using q_table_2 for next-state action selection
            let next_max = self
                .q_table_2
                .get(next_state)
                .and_then(|m| {
                    m.values()
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                })
                .copied()
                .unwrap_or(0.0);
            let current = self
                .q_table
                .get(state)
                .and_then(|m| m.get(action))
                .copied()
                .unwrap_or(0.0);
            let new_q = current + alpha * (reward + gamma * next_max - current);
            self.q_table
                .entry(state.clone())
                .or_default()
                .insert(action.to_string(), new_q);
        } else {
            // Update q_table_2, using q_table for next-state action selection
            let next_max = self
                .q_table
                .get(next_state)
                .and_then(|m| {
                    m.values()
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                })
                .copied()
                .unwrap_or(0.0);
            let current = self
                .q_table_2
                .get(state)
                .and_then(|m| m.get(action))
                .copied()
                .unwrap_or(0.0);
            let new_q = current + alpha * (reward + gamma * next_max - current);
            self.q_table_2
                .entry(state.clone())
                .or_default()
                .insert(action.to_string(), new_q);
        }
    }

    /// Select the best action using the average of both Q-tables (Double Q-Learning).
    pub fn best_action_using_both(&self, state: &(String, String)) -> Option<(String, f64)> {
        let actions_q1 = self.q_table.get(state);
        let actions_q2 = self.q_table_2.get(state);

        let mut all_actions: Vec<String> = Vec::new();
        if let Some(m) = actions_q1 {
            all_actions.extend(m.keys().cloned());
        }
        if let Some(m) = actions_q2 {
            all_actions.extend(m.keys().cloned());
        }
        all_actions.sort();
        all_actions.dedup();

        all_actions
            .into_iter()
            .map(|a| {
                let q1 = actions_q1.and_then(|m| m.get(&a)).copied().unwrap_or(0.0);
                let q2 = actions_q2.and_then(|m| m.get(&a)).copied().unwrap_or(0.0);
                let avg = (q1 + q2) / 2.0;
                (a.clone(), avg)
            })
            .max_by(|(_, v1), (_, v2)| v1.partial_cmp(v2).unwrap_or(std::cmp::Ordering::Equal))
    }

    pub fn decay_exploration(&mut self, decay_rate: f64) {
        self.exploration_rate = (self.exploration_rate * decay_rate).max(0.01);
    }

    /// Perform a batch update using sampled experiences from the replay buffer.
    pub fn batch_update(&mut self, batch_size: usize, _gamma: f64, _alpha: f64) -> usize {
        if self.replay_buffer.is_empty() {
            return 0;
        }
        let batch = self
            .replay_buffer
            .sample(batch_size.min(self.replay_buffer.len()));
        let count = batch.len();
        for (state, action, reward, next_state) in batch {
            self.update(&state, &action, reward, &next_state);
        }
        count
    }
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

/// Simple deterministic "random" value based on hashing the current timestamp.
/// Used to avoid adding a `rand` dependency for exploration in Q-learning.
fn simple_random_f64() -> f64 {
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    nanos.hash(&mut hasher);
    let hash = hasher.finish();
    (hash as f64) / (u64::MAX as f64)
}

fn simple_random_u64() -> u64 {
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    nanos.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod qlearning_tests {
    use super::*;

    #[test]
    fn test_new_agent_empty() {
        let agent = QLearningAgent::default();
        assert!(agent.q_table.is_empty());
    }

    #[test]
    fn test_choose_action_returns_valid_action() {
        let mut agent = QLearningAgent::default();
        // Add a Q-value for a known state-action pair
        agent
            .q_table
            .entry(("s1".to_string(), "s1".to_string()))
            .or_default()
            .insert("action_a".to_string(), 1.0);
        let action = agent.choose_action(
            &("s1".to_string(), "s1".to_string()),
            &["action_a".to_string(), "action_b".to_string()],
        );
        assert!(action == Some("action_a".to_string()) || action == Some("action_b".to_string()));
    }

    #[test]
    fn test_update_adds_entry() {
        let mut agent = QLearningAgent::default();
        agent.update(
            &("s1".to_string(), "s1".to_string()),
            "action_a",
            1.0,
            &("s2".to_string(), "s2".to_string()),
        );
        assert!(agent
            .q_table
            .contains_key(&("s1".to_string(), "s1".to_string())));
    }

    #[test]
    fn test_double_q_update_creates_tables() {
        let mut agent = QLearningAgent::default();
        // Use two different state/action combos to hit both coin-flip branches
        agent.double_q_update(
            &("s1".to_string(), "s1".to_string()),
            "a1",
            1.0,
            &("s2".to_string(), "s2".to_string()),
        );
        agent.double_q_update(
            &("s3".to_string(), "s3".to_string()),
            "b1",
            0.5,
            &("s4".to_string(), "s4".to_string()),
        );
        // At least one table should have entries from the two updates
        let q1_has = agent
            .q_table
            .contains_key(&("s1".to_string(), "s1".to_string()))
            || agent
                .q_table
                .contains_key(&("s3".to_string(), "s3".to_string()));
        let q2_has = agent
            .q_table_2
            .contains_key(&("s1".to_string(), "s1".to_string()))
            || agent
                .q_table_2
                .contains_key(&("s3".to_string(), "s3".to_string()));
        assert!(q1_has, "q_table should have at least one entry");
        assert!(q2_has, "q_table_2 should have at least one entry");
    }

    #[test]
    fn test_best_action_using_both() {
        let mut agent = QLearningAgent::default();
        agent.double_q_update(
            &("s1".to_string(), "s1".to_string()),
            "a1",
            1.0,
            &("s2".to_string(), "s2".to_string()),
        );
        agent.double_q_update(
            &("s1".to_string(), "s1".to_string()),
            "a2",
            0.5,
            &("s2".to_string(), "s2".to_string()),
        );
        let best = agent.best_action_using_both(&("s1".to_string(), "s1".to_string()));
        assert!(best.is_some());
        let (action, _) = best.unwrap();
        assert_eq!(action, "a1");
    }

    #[test]
    fn test_best_action_using_both_empty_state() {
        let agent = QLearningAgent::default();
        let best = agent.best_action_using_both(&("unknown".to_string(), "unknown".to_string()));
        assert!(best.is_none());
    }

    #[test]
    fn test_decay_exploration_reduces_epsilon() {
        let mut agent = QLearningAgent::default();
        let initial = agent.exploration_rate;
        agent.decay_exploration(0.99);
        assert!(agent.exploration_rate < initial);
    }

    #[test]
    fn test_replay_buffer_push_and_sample() {
        let mut buf = ReplayBuffer::new(100);
        buf.push(("s1", "t1"), "a1", 1.0, ("s2", "t2"));
        buf.push(("s2", "t2"), "a2", 0.5, ("s3", "t3"));
        assert_eq!(buf.len(), 2);
        let samples = buf.sample(2);
        assert_eq!(samples.len(), 2);
    }

    #[test]
    fn test_batch_update_processes_samples() {
        let mut agent = QLearningAgent::default();
        agent
            .replay_buffer
            .push(("s1", "t1"), "a1", 1.0, ("s2", "t2"));
        agent
            .replay_buffer
            .push(("s2", "t2"), "a2", 0.5, ("s3", "t3"));
        let count = agent.batch_update(5, 0.9, 0.1);
        assert_eq!(count, 2);
    }
}
