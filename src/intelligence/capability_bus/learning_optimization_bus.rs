//! LearningOptimizationBus — merged WorkflowLearningBus + OptimizationBus (BLUE70 §2.2.3)
//!
//! Combines historical execution learning with optimization and failure prevention.
//! WorkflowLearningBus events feed directly into the optimization analysis pipeline
//! without intermediate event passing.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// A single execution event for learning (migrated from WorkflowLearningBus).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEvent {
    pub task_type: String,
    pub agent: String,
    pub success: bool,
    pub duration_ms: u64,
    pub token_cost: u64,
    pub quality_score: f64,
    pub timestamp_ms: u64,
}

/// A failure prevention rule (migrated from OptimizationBus).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreventionRule {
    pub id: String,
    pub pattern: String,
    pub action: String,
    pub confidence: f64,
    pub created_ms: u64,
}

/// An optimization suggestion derived from learned patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationSuggestion {
    pub task_type: String,
    pub recommended_agent: Option<String>,
    pub expected_duration_ms: Option<u64>,
    pub expected_token_cost: Option<u64>,
    pub confidence: f64,
    pub based_on_samples: usize,
}

const MAX_EVENTS: usize = 2000;
const MAX_PREVENTION_RULES: usize = 100;

/// Learning and optimization bus (BLUE70 §2.2.3).
///
/// Design notes:
/// - `record_and_optimize()` is a single atomic operation that both learns
///   from an event and triggers optimization analysis.
/// - Prevention rules are generated when failure patterns are detected
///   (e.g., >60% failure rate for a task type).
/// - Optimization suggestions are cached by task type for O(1) lookup.
#[derive(Debug)]
pub struct LearningOptimizationBus {
    /// Historical execution events (was WorkflowLearningBus).
    events: VecDeque<LearningEvent>,
    /// Failure prevention rules (was OptimizationBus).
    prevention_rules: Vec<PreventionRule>,
    /// Cached optimization suggestions.
    optimization_cache: HashMap<String, OptimizationSuggestion>,
    /// Maximum number of events to retain.
    max_events: usize,
    /// Rule ID counter.
    next_rule_id: u64,
}

impl LearningOptimizationBus {
    /// Create a new LearningOptimizationBus.
    pub fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(MAX_EVENTS.min(256)),
            prevention_rules: Vec::with_capacity(MAX_PREVENTION_RULES.min(32)),
            optimization_cache: HashMap::new(),
            max_events: MAX_EVENTS,
            next_rule_id: 0,
        }
    }

    /// Set a custom max events limit.
    pub fn with_max_events(mut self, max: usize) -> Self {
        self.max_events = max.max(100);
        self
    }

    // ── Record & Optimize (unified operation) ─────────────────────

    /// Record an execution event and trigger optimization analysis atomically.
    ///
    /// This is the primary entry point — it both learns from the event
    /// and generates any applicable optimization suggestions or prevention rules.
    pub fn record_and_optimize(&mut self, event: LearningEvent) {
        // 1. Store the event
        if self.events.len() >= self.max_events {
            self.events.pop_front();
        }
        self.events.push_back(event.clone());

        // 2. Check for optimization opportunity
        if let Some(suggestion) = self.analyze_for_optimization(&event) {
            self.optimization_cache
                .insert(event.task_type.clone(), suggestion);
        }

        // 3. Check for failure prevention rule
        if let Some(rule) = self.analyze_for_prevention(&event) {
            self.preventions_push(rule);
        }
    }

    // ── Query ─────────────────────────────────────────────────────

    /// Get optimization suggestion for a task type.
    pub fn suggestion_for(&self, task_type: &str) -> Option<&OptimizationSuggestion> {
        self.optimization_cache.get(task_type)
    }

    /// Get agent success rate.
    pub fn agent_success_rate(&self, agent: &str) -> Option<f64> {
        let (total, successes) = self
            .events
            .iter()
            .filter(|e| e.agent == agent)
            .fold((0usize, 0usize), |(t, s), e| (t + 1, s + e.success as usize));
        if total == 0 {
            None
        } else {
            Some(successes as f64 / total as f64)
        }
    }

    /// Get task type success rate.
    pub fn task_type_success_rate(&self, task_type: &str) -> Option<f64> {
        let (total, successes) = self
            .events
            .iter()
            .filter(|e| e.task_type == task_type)
            .fold((0usize, 0usize), |(t, s), e| (t + 1, s + e.success as usize));
        if total == 0 {
            None
        } else {
            Some(successes as f64 / total as f64)
        }
    }

    /// Get average duration for an agent on a task type.
    pub fn avg_duration_ms(&self, agent: &str, task_type: &str) -> Option<u64> {
        let events: Vec<_> = self
            .events
            .iter()
            .filter(|e| e.agent == agent && e.task_type == task_type)
            .collect();
        if events.is_empty() {
            return None;
        }
        let total: u64 = events.iter().map(|e| e.duration_ms).sum();
        Some(total / events.len() as u64)
    }

    /// Get all prevention rules.
    pub fn prevention_rules(&self) -> &[PreventionRule] {
        &self.prevention_rules
    }

    /// Get all cached optimization suggestions.
    pub fn all_suggestions(&self) -> Vec<&OptimizationSuggestion> {
        self.optimization_cache.values().collect()
    }

    /// Get event count.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Get rule count.
    pub fn rule_count(&self) -> usize {
        self.prevention_rules.len()
    }

    /// Get all events (for snapshot).
    pub fn events_snapshot(&self) -> Vec<LearningEvent> {
        self.events.iter().cloned().collect()
    }

    // ── Private helpers ───────────────────────────────────────────

    fn preventions_push(&mut self, rule: PreventionRule) {
        if self.prevention_rules.len() >= MAX_PREVENTION_RULES {
            self.prevention_rules.remove(0);
        }
        self.prevention_rules.push(rule);
    }

    fn analyze_for_optimization(&self, event: &LearningEvent) -> Option<OptimizationSuggestion> {
        // Check if we have enough samples for this task type
        let samples: Vec<_> = self
            .events
            .iter()
            .filter(|e| e.task_type == event.task_type)
            .collect();

        if samples.len() < 3 {
            return None; // Not enough data
        }

        // Find the best agent for this task type
        let mut agent_scores: HashMap<&str, (usize, usize, u64)> = HashMap::new();
        for s in &samples {
            let entry = agent_scores.entry(&s.agent).or_insert((0, 0, 0));
            entry.0 += 1; // total
            if s.success {
                entry.1 += 1; // successes
            }
            entry.2 += s.duration_ms; // total duration
        }

        let best_agent = agent_scores
            .iter()
            .filter(|(_, (total, _, _))| *total >= 2)
            .max_by(|a, b| {
                let a_rate = a.1 .1 as f64 / a.1 .0 as f64;
                let b_rate = b.1 .1 as f64 / b.1 .0 as f64;
                a_rate.partial_cmp(&b_rate).unwrap_or(std::cmp::Ordering::Equal)
            });

        best_agent.map(|(agent, (total, successes, total_dur))| {
            let rate = *successes as f64 / *total as f64;
            OptimizationSuggestion {
                task_type: event.task_type.clone(),
                recommended_agent: Some(agent.to_string()),
                expected_duration_ms: Some(*total_dur / *total as u64),
                expected_token_cost: None,
                confidence: rate,
                based_on_samples: *total,
            }
        })
    }

    fn analyze_for_prevention(&mut self, event: &LearningEvent) -> Option<PreventionRule> {
        // If this event is a failure, check if the task type has a high failure rate
        if !event.success {
            let rate = self
                .task_type_success_rate(&event.task_type)
                .unwrap_or(1.0);
            if rate < 0.4 {
                // >60% failure rate → create prevention rule
                self.next_rule_id += 1;
                return Some(PreventionRule {
                    id: format!("pr_{}", self.next_rule_id),
                    pattern: format!("high_failure_rate:{}", event.task_type),
                    action: format!("consider alternative agent for '{}'", event.task_type),
                    confidence: 1.0 - rate,
                    created_ms: event.timestamp_ms,
                });
            }
        }
        None
    }
}

impl Default for LearningOptimizationBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(task_type: &str, agent: &str, success: bool, dur_ms: u64) -> LearningEvent {
        LearningEvent {
            task_type: task_type.to_string(),
            agent: agent.to_string(),
            success,
            duration_ms: dur_ms,
            token_cost: 1000,
            quality_score: if success { 0.9 } else { 0.1 },
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    #[test]
    fn test_new_bus() {
        let bus = LearningOptimizationBus::new();
        assert_eq!(bus.event_count(), 0);
        assert_eq!(bus.rule_count(), 0);
    }

    #[test]
    fn test_record_event() {
        let mut bus = LearningOptimizationBus::new();
        bus.record_and_optimize(make_event("research", "agent_a", true, 1000));
        assert_eq!(bus.event_count(), 1);
    }

    #[test]
    fn test_agent_success_rate() {
        let mut bus = LearningOptimizationBus::new();
        bus.record_and_optimize(make_event("t1", "agent_a", true, 100));
        bus.record_and_optimize(make_event("t2", "agent_a", true, 200));
        bus.record_and_optimize(make_event("t3", "agent_a", false, 150));

        let rate = bus.agent_success_rate("agent_a").unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_task_type_success_rate() {
        let mut bus = LearningOptimizationBus::new();
        bus.record_and_optimize(make_event("research", "a1", true, 100));
        bus.record_and_optimize(make_event("research", "a2", false, 100));
        bus.record_and_optimize(make_event("research", "a3", true, 100));

        let rate = bus.task_type_success_rate("research").unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_optimization_suggestion_after_enough_samples() {
        let mut bus = LearningOptimizationBus::new();
        // Add 3 successful samples for agent_a on "research"
        for _ in 0..3 {
            bus.record_and_optimize(make_event("research", "agent_a", true, 500));
        }
        // Add 1 failed sample for agent_b on "research"
        bus.record_and_optimize(make_event("research", "agent_b", false, 1000));

        let suggestion = bus.suggestion_for("research");
        assert!(suggestion.is_some());
        assert_eq!(suggestion.unwrap().recommended_agent.as_deref(), Some("agent_a"));
    }

    #[test]
    fn test_no_suggestion_with_few_samples() {
        let mut bus = LearningOptimizationBus::new();
        bus.record_and_optimize(make_event("rare_task", "agent_a", true, 100));
        bus.record_and_optimize(make_event("rare_task", "agent_a", true, 100));

        let suggestion = bus.suggestion_for("rare_task");
        assert!(suggestion.is_none()); // Need at least 3 samples
    }

    #[test]
    fn test_prevention_rule_on_high_failure() {
        let mut bus = LearningOptimizationBus::new();
        // Add 3 failures for "unstable_task"
        for _ in 0..3 {
            bus.record_and_optimize(make_event("unstable_task", "agent_x", false, 500));
        }

        // The failure rate should now be 100% → prevention rule should be created
        assert!(bus.rule_count() >= 1);
        let rule = &bus.prevention_rules()[0];
        assert!(rule.pattern.contains("unstable_task"));
    }

    #[test]
    fn test_avg_duration() {
        let mut bus = LearningOptimizationBus::new();
        bus.record_and_optimize(make_event("research", "agent_a", true, 1000));
        bus.record_and_optimize(make_event("research", "agent_a", true, 2000));

        let avg = bus.avg_duration_ms("agent_a", "research");
        assert_eq!(avg, Some(1500));
    }

    #[test]
    fn test_all_suggestions() {
        let mut bus = LearningOptimizationBus::new();
        for _ in 0..3 {
            bus.record_and_optimize(make_event("task_a", "agent_a", true, 100));
        }
        for _ in 0..3 {
            bus.record_and_optimize(make_event("task_b", "agent_b", true, 200));
        }

        let suggestions = bus.all_suggestions();
        assert_eq!(suggestions.len(), 2);
    }

    #[test]
    fn test_events_snapshot() {
        let mut bus = LearningOptimizationBus::new();
        bus.record_and_optimize(make_event("t1", "a1", true, 100));
        bus.record_and_optimize(make_event("t2", "a2", false, 200));

        let snapshot = bus.events_snapshot();
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn test_custom_max_events() {
        let bus = LearningOptimizationBus::with_max_events(LearningOptimizationBus::new(), 50);
        // Events will be limited to 50
        assert!(bus.max_events >= 100); // Clamped to min 100
    }

    #[test]
    fn test_unknown_agent_rate() {
        let bus = LearningOptimizationBus::new();
        assert!(bus.agent_success_rate("nonexistent").is_none());
    }

    #[test]
    fn test_unknown_task_rate() {
        let bus = LearningOptimizationBus::new();
        assert!(bus.task_type_success_rate("nonexistent").is_none());
    }
}
