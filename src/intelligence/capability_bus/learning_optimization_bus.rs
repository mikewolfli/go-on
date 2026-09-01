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

    // ── Record ────────────────────────────────────────────────────────

    /// Record an execution event (bounded FIFO ring).
    ///
    /// Named `record_event` (not `record_and_optimize`): the former per-event
    /// optimization/prevention analysis had zero production readers and was
    /// removed, so recording is all this method does.
    pub fn record_event(&mut self, event: LearningEvent) {
        if self.events.len() >= self.max_events {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    // ── Query ─────────────────────────────────────────────────────────

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
        bus.record_event(make_event("research", "agent_a", true, 1000));
        assert_eq!(bus.event_count(), 1);
    }

    #[test]
    fn test_events_evict_oldest_when_over_limit() {
        let mut bus = LearningOptimizationBus::new();
        bus.max_events = 100;
        for i in 0..150 {
            bus.record_event(make_event("t", "a", true, i));
        }
        assert_eq!(bus.event_count(), 100);
        // The oldest events were evicted; the newest remain.
        let snapshot = bus.events_snapshot();
        assert_eq!(snapshot.len(), 100);
        assert_eq!(snapshot.first().map(|e| e.duration_ms), Some(50));
    }

    #[test]
    fn test_events_snapshot() {
        let mut bus = LearningOptimizationBus::new();
        bus.record_event(make_event("t1", "a1", true, 100));
        bus.record_event(make_event("t2", "a2", false, 200));

        let snapshot = bus.events_snapshot();
        assert_eq!(snapshot.len(), 2);
    }
}
