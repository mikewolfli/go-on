use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Scheduling priority (higher = more urgent)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Priority(pub i64);

/// Task priority with anti-starvation boost
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub task_id: String,
    pub role: String,
    /// Optional provider name for bulkhead isolation.
    pub provider: Option<String>,
    pub priority: Priority,
    /// Base priority score (before aging boost)
    pub base_score: f64,
    /// Urgency component (0.0–1.0)
    pub urgency: f64,
    /// Cost efficiency component (0.0–1.0)
    pub cost_efficiency: f64,
    /// Deadline pressure (0.0–1.0), 0 = no deadline
    pub deadline_pressure: f64,
    /// Aging bonus (increments over time)
    pub aging_bonus: f64,
    /// Submission timestamp (epoch ms)
    pub submitted_at: i64,
    /// Number of retries so far
    pub retries: u32,
    /// Max allowed retries
    pub max_retries: u32,
}

impl ScheduledTask {
    pub fn effective_priority(&self) -> f64 {
        let base = self.base_score * (1.0 + self.aging_bonus);
        let urgency_factor = self.urgency * 2.0;
        let deadline_factor = if self.deadline_pressure > 0.0 {
            self.deadline_pressure * 3.0
        } else {
            0.0
        };
        base + urgency_factor + deadline_factor
    }
}

impl Eq for ScheduledTask {}

impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        self.task_id == other.task_id
    }
}

impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_p = self.effective_priority();
        let other_p = other.effective_priority();

        // NaN from float arithmetic violates the BinaryHeap contract (partial_cmp
        // returns None and falls back to Equal). Clamp non-finite values to 0.0
        // so ordering remains total and consistent.
        let self_p = if self_p.is_finite() { self_p } else { 0.0 };
        let other_p = if other_p.is_finite() { other_p } else { 0.0 };

        self_p
            .partial_cmp(&other_p)
            .unwrap_or(Ordering::Equal)
            // Tie-break by task_id to satisfy the BinaryHeap contract:
            // Eq(a,b) == true  →  cmp(a,b) == Equal
            .then_with(|| self.task_id.cmp(&other.task_id))
        // BinaryHeap is a max-heap: the "greatest" element per Ord is at the top.
        // Higher effective_priority should be "greater", so we do NOT reverse.
    }
}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
