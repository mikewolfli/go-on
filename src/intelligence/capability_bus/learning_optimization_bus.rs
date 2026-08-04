//! LearningOptimizationBus — merged WorkflowLearningBus + OptimizationBus (BLUE70 §2.2.3)
//!
//! Combines historical execution learning with optimization and failure prevention.
//! WorkflowLearningBus events feed directly into the optimization analysis pipeline
//! without intermediate event passing.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

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

const MAX_EVENTS: usize = 2000;

/// Learning and optimization bus (BLUE70 §2.2.3).
///
/// Keeps a bounded ring of execution events. The former per-event
/// optimization/prevention analysis (full scans of `events` on every
/// `record_and_optimize` call) was removed: its outputs — optimization
/// suggestions and prevention rules — had zero production readers, so every
/// feedback() was paying 2×O(2000) scans to populate data nothing consumed.
/// The events ring itself remains: it feeds `event_count()` (profile) and
/// `events_snapshot()` (sense recency/outcome scoring).
#[derive(Debug)]
pub struct LearningOptimizationBus {
    /// Historical execution events (was WorkflowLearningBus).
    events: VecDeque<LearningEvent>,
    /// Maximum number of events to retain.
    max_events: usize,
}

impl LearningOptimizationBus {
    /// Create a new LearningOptimizationBus.
    pub fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(MAX_EVENTS.min(256)),
            max_events: MAX_EVENTS,
        }
    }

    /// Set a custom max events limit.
    pub fn with_max_events(mut self, max: usize) -> Self {
        self.max_events = max.max(100);
        self
    }

    // ── Record ────────────────────────────────────────────────────────

    /// Record an execution event (bounded FIFO ring).
    pub fn record_and_optimize(&mut self, event: LearningEvent) {
        if self.events.len() >= self.max_events {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    // ── Query ─────────────────────────────────────────────────────────

    /// Get agent success rate.
    pub fn agent_success_rate(&self, agent: &str) -> Option<f64> {
        let (total, successes) = self
            .events
            .iter()
            .filter(|e| e.agent == agent)
            .fold((0usize, 0usize), |(t, s), e| {
                (t + 1, s + e.success as usize)
            });
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
            .fold((0usize, 0usize), |(t, s), e| {
                (t + 1, s + e.success as usize)
            });
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

    /// Get event count.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Get all events (for snapshot).
    pub fn events_snapshot(&self) -> Vec<LearningEvent> {
        self.events.iter().cloned().collect()
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
    fn test_events_evict_oldest_when_over_limit() {
        let mut bus = LearningOptimizationBus::with_max_events(LearningOptimizationBus::new(), 100);
        for i in 0..150 {
            bus.record_and_optimize(make_event("t", "a", true, i));
        }
        assert_eq!(bus.event_count(), 100);
        // The oldest events were evicted; the newest remain.
        let snapshot = bus.events_snapshot();
        assert_eq!(snapshot.len(), 100);
        assert_eq!(snapshot.first().map(|e| e.duration_ms), Some(50));
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
