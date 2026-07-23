//! UnifiedKnowledgeBus — merged KnowledgeBus + ReputationStore + ExperienceKnowledgeBase (BLUE70 §2.2.1)
//!
//! Provides a unified knowledge management interface that combines:
//! - Knowledge insights (patterns, solutions)
//! - Agent reputation scores (EMA-smoothed)
//! - Experience cases (success/failure history)
//!
//! This replaces the three separate KnowledgeBus, ReputationStore, and
//! ExperienceKnowledgeBase components with a single cohesive bus.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A reusable knowledge insight (migrated from KnowledgeBus).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeInsight {
    pub id: String,
    pub pattern: String,
    pub solution_summary: String,
    pub applicability_tags: Vec<String>,
    pub confidence: f64,
    pub created_ms: u64,
}

/// Agent reputation score with EMA smoothing (migrated from ReputationStore).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationScore {
    pub agent: String,
    /// EMA-smoothed reliability score (0.0 - 1.0).
    pub score: f64,
    pub total_tasks: u64,
    pub successful_tasks: u64,
    pub last_updated_ms: u64,
}

/// A recorded experience case (migrated from ExperienceKnowledgeBase).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceCase {
    pub agent: String,
    pub task_type: String,
    pub success: bool,
    pub summary: String,
    pub timestamp_ms: u64,
}

/// Unified result from a knowledge query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedKnowledgeResult {
    pub reputation: Option<ReputationScore>,
    pub relevant_experiences: Vec<ExperienceCase>,
    pub applicable_knowledge: Vec<KnowledgeInsight>,
}

const MAX_KNOWLEDGE_INSIGHTS: usize = 500;
const MAX_EXPERIENCE_CASES: usize = 1000;
/// Maximum matching insights per task type before we stop recording new ones.
const NOVELTY_THRESHOLD: usize = 3;

/// Unified knowledge bus combining knowledge, reputation, and experience.
///
/// Design notes:
/// - All three knowledge dimensions are stored in one struct, reducing
///   the number of Arc<RwLock<>> acquisitions from 3 to 1.
/// - Reputation uses EMA (Exponential Moving Average) with alpha=0.3.
/// - Experience cases are FIFO-limited to prevent unbounded memory growth.
#[derive(Debug)]
pub struct UnifiedKnowledgeBus {
    /// Reusable knowledge insights (was KnowledgeBus).
    insights: Vec<KnowledgeInsight>,
    /// Agent reputation scores (was ReputationStore).
    reputation_scores: HashMap<String, ReputationScore>,
    /// Experience case history (was ExperienceKnowledgeBase).
    experience_cases: Vec<ExperienceCase>,
    /// Knowledge entries indexed by task type for quick lookup.
    knowledge_by_task: HashMap<String, Vec<KnowledgeInsight>>,
    /// EMA alpha factor for reputation smoothing.
    ema_alpha: f64,
}

impl UnifiedKnowledgeBus {
    /// Create a new UnifiedKnowledgeBus with default settings.
    pub fn new() -> Self {
        Self {
            insights: Vec::with_capacity(MAX_KNOWLEDGE_INSIGHTS.min(64)),
            reputation_scores: HashMap::new(),
            experience_cases: Vec::with_capacity(MAX_EXPERIENCE_CASES.min(128)),
            knowledge_by_task: HashMap::new(),
            ema_alpha: 0.3,
        }
    }

    /// Set a custom EMA alpha factor.
    pub fn with_ema_alpha(mut self, alpha: f64) -> Self {
        self.ema_alpha = alpha.clamp(0.01, 0.99);
        self
    }

    // ── Unified Query ─────────────────────────────────────────────

    /// Unified query: retrieve reputation, experiences, and knowledge in one call.
    pub fn query(&self, agent: &str, task_type: &str) -> UnifiedKnowledgeResult {
        UnifiedKnowledgeResult {
            reputation: self.reputation_scores.get(agent).cloned(),
            relevant_experiences: self.experience_cases.iter()
                .filter(|e| e.agent == agent || e.task_type == task_type)
                .take(5)
                .cloned()
                .collect(),
            applicable_knowledge: self.knowledge_by_task.get(task_type)
                .cloned()
                .unwrap_or_default(),
        }
    }

    // ── Record Outcome (unified write) ────────────────────────────

    /// Record an execution outcome, updating reputation, experience, and knowledge.
    pub fn record_outcome(&mut self, agent: &str, task_type: &str, success: bool, summary: String) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // 1. Update reputation (EMA)
        self.update_reputation(agent, success, now_ms);

        // 2. Add experience case
        self.experience_cases.push(ExperienceCase {
            agent: agent.to_string(),
            task_type: task_type.to_string(),
            success,
            summary: summary.clone(),
            timestamp_ms: now_ms,
        });
        if self.experience_cases.len() > MAX_EXPERIENCE_CASES {
            self.experience_cases.remove(0);
        }

        // 3. If successful and novel pattern, add as knowledge insight
        if success && self.is_novel_pattern(task_type, &summary) {
            let insight = KnowledgeInsight {
                id: format!("ki_{}", now_ms),
                pattern: summary.clone(),
                solution_summary: summary,
                applicability_tags: vec![task_type.to_string()],
                confidence: 0.5,
                created_ms: now_ms,
            };
            self.add_insight(insight);
        }
    }

    // ── Knowledge Insights ────────────────────────────────────────

    /// Add a knowledge insight (equivalent to KnowledgeBus::add_insight).
    pub fn add_insight(&mut self, insight: KnowledgeInsight) {
        if self.insights.len() >= MAX_KNOWLEDGE_INSIGHTS {
            self.insights.remove(0);
        }
        // Index by tags
        for tag in &insight.applicability_tags {
            self.knowledge_by_task
                .entry(tag.clone())
                .or_default()
                .push(insight.clone());
        }
        self.insights.push(insight);
    }

    /// Find matching insights by tags (equivalent to KnowledgeBus::find_matching).
    pub fn find_matching(&self, tags: &[String]) -> Vec<&KnowledgeInsight> {
        self.insights
            .iter()
            .filter(|i| tags.iter().any(|t| i.applicability_tags.contains(t)))
            .collect()
    }

    // ── Reputation ────────────────────────────────────────────────

    /// Get the reputation score for an agent.
    pub fn get_reputation(&self, agent: &str) -> Option<f64> {
        self.reputation_scores.get(agent).map(|r| r.score)
    }

    /// Get full reputation info for an agent.
    pub fn reputation_info(&self, agent: &str) -> Option<&ReputationScore> {
        self.reputation_scores.get(agent)
    }

    /// Get all reputation scores.
    pub fn all_reputations(&self) -> Vec<ReputationScore> {
        self.reputation_scores.values().cloned().collect()
    }

    // ── Experience ────────────────────────────────────────────────

    /// Get recent experience cases for an agent.
    pub fn agent_experiences(&self, agent: &str) -> Vec<&ExperienceCase> {
        self.experience_cases.iter().filter(|e| e.agent == agent).collect()
    }

    /// Get recent experience cases for a task type.
    pub fn task_experiences(&self, task_type: &str) -> Vec<&ExperienceCase> {
        self.experience_cases.iter().filter(|e| e.task_type == task_type).collect()
    }

    /// Calculate success rate for an agent.
    pub fn agent_success_rate(&self, agent: &str) -> Option<f64> {
        let cases: Vec<_> = self.experience_cases.iter().filter(|e| e.agent == agent).collect();
        if cases.is_empty() {
            return None;
        }
        let successes = cases.iter().filter(|e| e.success).count();
        Some(successes as f64 / cases.len() as f64)
    }

    // ── Snapshots ─────────────────────────────────────────────────

    /// Full snapshot of all knowledge insights.
    pub fn knowledge_snapshot(&self) -> Vec<KnowledgeInsight> {
        self.insights.clone()
    }

    /// Full snapshot of all experience cases.
    pub fn experience_snapshot(&self) -> Vec<ExperienceCase> {
        self.experience_cases.clone()
    }

    /// Number of insights stored.
    pub fn insight_count(&self) -> usize {
        self.insights.len()
    }

    /// Number of experience cases stored.
    pub fn experience_count(&self) -> usize {
        self.experience_cases.len()
    }

    /// Number of agents with reputation scores.
    pub fn reputation_count(&self) -> usize {
        self.reputation_scores.len()
    }

    /// Total number of tasks tracked across all agents.
    pub fn total_tasks(&self) -> u64 {
        self.reputation_scores.values().map(|r| r.total_tasks).sum()
    }

    // ── Private helpers ──────────────────────────────────────────

    fn update_reputation(&mut self, agent: &str, success: bool, now_ms: u64) {
        let entry = self.reputation_scores
            .entry(agent.to_string())
            .or_insert_with(|| ReputationScore {
                agent: agent.to_string(),
                score: 0.5, // Start at neutral
                total_tasks: 0,
                successful_tasks: 0,
                last_updated_ms: now_ms,
            });

        entry.total_tasks += 1;
        if success {
            entry.successful_tasks += 1;
        }

        // EMA update: new_score = alpha * current_outcome + (1-alpha) * old_score
        let outcome = if success { 1.0 } else { 0.0 };
        entry.score = self.ema_alpha * outcome + (1.0 - self.ema_alpha) * entry.score;
        entry.last_updated_ms = now_ms;
    }

    fn is_novel_pattern(&self, task_type: &str, summary: &str) -> bool {
        // A pattern is novel if we have fewer than NOVELTY_THRESHOLD insights
        // for this task type, OR the summary doesn't match existing insights.
        let count = self.knowledge_by_task
            .get(task_type)
            .map(|v| v.len())
            .unwrap_or(0);
        if count < NOVELTY_THRESHOLD {
            return true;
        }
        // Even if over threshold, check if this summary is semantically different
        // from existing insights (simple text overlap heuristic).
        if let Some(existing) = self.knowledge_by_task.get(task_type) {
            let summary_lower = summary.to_lowercase();
            !existing.iter().any(|insight| {
                insight.pattern.to_lowercase().contains(&summary_lower)
                    || summary_lower.contains(&insight.pattern.to_lowercase())
            })
        } else {
            true
        }
    }
}

impl Default for UnifiedKnowledgeBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_bus() {
        let bus = UnifiedKnowledgeBus::new();
        assert_eq!(bus.insight_count(), 0);
        assert_eq!(bus.experience_count(), 0);
        assert_eq!(bus.reputation_count(), 0);
    }

    #[test]
    fn test_record_outcome_updates_reputation() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "research", true, "Completed research task".to_string());
        let rep = bus.get_reputation("agent_a");
        assert!(rep.is_some());
        assert!(rep.unwrap() > 0.5); // Success should increase score

        bus.record_outcome("agent_a", "research", false, "Failed task".to_string());
        let rep = bus.get_reputation("agent_a").unwrap();
        assert!(rep < 0.7); // Failure should decrease score
    }

    #[test]
    fn test_record_outcome_adds_experience() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "coding", true, "Wrote feature X".to_string());
        assert_eq!(bus.experience_count(), 1);

        let experiences = bus.agent_experiences("agent_a");
        assert_eq!(experiences.len(), 1);
        assert!(experiences[0].success);
    }

    #[test]
    fn test_successful_outcome_adds_knowledge() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "research", true, "Found pattern: use BFS for graph traversal".to_string());
        assert_eq!(bus.insight_count(), 1);

        let matching = bus.find_matching(&["research".to_string()]);
        assert!(!matching.is_empty());
    }

    #[test]
    fn test_failed_outcome_does_not_add_knowledge() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "research", false, "Failed".to_string());
        assert_eq!(bus.insight_count(), 0);
    }

    #[test]
    fn test_unified_query() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "research", true, "Found useful pattern".to_string());

        let result = bus.query("agent_a", "research");
        assert!(result.reputation.is_some());
        assert!(!result.relevant_experiences.is_empty());
    }

    #[test]
    fn test_agent_success_rate() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "t1", true, "ok".to_string());
        bus.record_outcome("agent_a", "t2", true, "ok".to_string());
        bus.record_outcome("agent_a", "t3", false, "fail".to_string());

        let rate = bus.agent_success_rate("agent_a").unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_default_ema_alpha() {
        let bus = UnifiedKnowledgeBus::new();
        assert!((bus.ema_alpha - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_custom_ema_alpha() {
        let bus = UnifiedKnowledgeBus::with_ema_alpha(UnifiedKnowledgeBus::new(), 0.5);
        assert!((bus.ema_alpha - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_ema_alpha_clamped() {
        let bus = UnifiedKnowledgeBus::with_ema_alpha(UnifiedKnowledgeBus::new(), 1.5);
        assert!((bus.ema_alpha - 0.99).abs() < 0.01);
    }

    #[test]
    fn test_knowledge_snapshot() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "research", true, "Pattern A".to_string());
        bus.record_outcome("agent_b", "coding", true, "Pattern B".to_string());
        assert_eq!(bus.knowledge_snapshot().len(), 2);
    }

    #[test]
    fn test_experience_snapshot() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "research", true, "done".to_string());
        bus.record_outcome("agent_b", "coding", false, "failed".to_string());
        assert_eq!(bus.experience_snapshot().len(), 2);
    }

    #[test]
    fn test_task_experiences() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_a", "research", true, "ok".to_string());
        bus.record_outcome("agent_b", "research", false, "fail".to_string());
        bus.record_outcome("agent_c", "coding", true, "ok".to_string());

        let research_exp = bus.task_experiences("research");
        assert_eq!(research_exp.len(), 2);
    }

    #[test]
    fn test_reputation_info() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("agent_x", "test", true, "done".to_string());
        let info = bus.reputation_info("agent_x");
        assert!(info.is_some());
        assert_eq!(info.unwrap().total_tasks, 1);
    }

    #[test]
    fn test_all_reputations() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("a1", "t1", true, "ok".to_string());
        bus.record_outcome("a2", "t2", true, "ok".to_string());
        assert_eq!(bus.all_reputations().len(), 2);
    }

    #[test]
    fn test_query_unknown_agent() {
        let bus = UnifiedKnowledgeBus::new();
        let result = bus.query("nonexistent", "unknown");
        assert!(result.reputation.is_none());
        assert!(result.relevant_experiences.is_empty());
        assert!(result.applicable_knowledge.is_empty());
    }

    #[test]
    fn test_total_tasks() {
        let mut bus = UnifiedKnowledgeBus::new();
        bus.record_outcome("a1", "t1", true, "ok".to_string());
        bus.record_outcome("a1", "t2", false, "fail".to_string());
        bus.record_outcome("a2", "t3", true, "ok".to_string());
        assert_eq!(bus.total_tasks(), 3);
    }
}
