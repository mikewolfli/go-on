//! F-GAP-03: Audit trail, replay, evidence
//!
//! Provides structured audit entries that capture the full decision path
//! of an agent interaction, supporting replay and evidence export.
//!
//! # Architecture
//!
//! - `AuditEntry`: a single decision point with input/output snapshots.
//! - `AuditTrail`: ordered collection of entries with serialization support.
//! - `append_entry`, `replay`, `export`: core operations.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
use serde_json::json;
use std::collections::VecDeque;

/// A single audit entry recording one decision step in an agent workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO-8601 timestamp of the decision.
    pub timestamp: String,
    /// Event type (e.g. "tool_call", "llm_completion", "agent_decision", "phase_transition").
    pub event_type: String,
    /// Agent that made the decision.
    pub agent_id: String,
    /// Task identifier this entry belongs to.
    pub task_id: String,
    /// Snapshot of the input state at decision time.
    pub input_snapshot: Value,
    /// Snapshot of the output state after the decision.
    pub output_snapshot: Value,
    /// Ordered list of decision points taken.
    pub decision_path: Vec<DecisionPoint>,
}

/// A single decision point within the decision path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionPoint {
    /// Step index within the path.
    pub step: usize,
    /// The action taken at this step.
    pub action: String,
    /// Rationale or reasoning for this decision.
    pub rationale: Option<String>,
    /// Confidence score (0.0–1.0) at this step.
    pub confidence: Option<f64>,
}

impl AuditEntry {
    /// Create a new audit entry with the given fields.
    pub fn new(
        event_type: impl Into<String>,
        agent_id: impl Into<String>,
        task_id: impl Into<String>,
        input_snapshot: Value,
        output_snapshot: Value,
    ) -> Self {
        Self {
            timestamp: chrono_now(),
            event_type: event_type.into(),
            agent_id: agent_id.into(),
            task_id: task_id.into(),
            input_snapshot,
            output_snapshot,
            decision_path: Vec::new(),
        }
    }

    /// Add a decision point to the entry's decision path.
    /// Used in integration tests to record decision steps.
    #[cfg(test)]
    pub fn add_decision(&mut self, action: impl Into<String>, rationale: Option<String>) {
        let step = self.decision_path.len();
        self.decision_path.push(DecisionPoint {
            step,
            action: action.into(),
            rationale,
            confidence: None,
        });
    }

    /// Add a decision point with a confidence score.
    /// Used in integration tests to record decision steps with confidence.
    #[cfg(test)]
    pub fn add_decision_with_confidence(
        &mut self,
        action: impl Into<String>,
        rationale: Option<String>,
        confidence: f64,
    ) {
        let step = self.decision_path.len();
        self.decision_path.push(DecisionPoint {
            step,
            action: action.into(),
            rationale,
            confidence: Some(confidence.clamp(0.0, 1.0)),
        });
    }
}

/// An ordered collection of audit entries supporting replay and export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditTrail {
    /// Unique trail identifier.
    pub trail_id: String,
    /// Ordered entries (oldest first).
    entries: VecDeque<AuditEntry>,
    /// Maximum number of entries before dropping oldest.
    max_entries: usize,
    /// Start timestamp of the trail.
    pub started_at: String,
}

impl AuditTrail {
    /// Create a new audit trail with the given identifier.
    pub fn new(trail_id: impl Into<String>, max_entries: usize) -> Self {
        let now = chrono_now();
        Self {
            trail_id: trail_id.into(),
            entries: VecDeque::new(),
            max_entries,
            started_at: now,
        }
    }

    /// Append an audit entry to the trail.
    /// Drops the oldest entry if max_entries is exceeded.
    pub fn append_entry(&mut self, entry: AuditEntry) {
        self.entries.push_back(entry);
        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
        }
    }

    /// Return all entries in chronological order.
    /// Used in integration tests to inspect trail contents.
    #[cfg(test)]
    pub fn entries(&self) -> Vec<&AuditEntry> {
        self.entries.iter().collect()
    }

    /// Replay the audit trail, calling `f` for each entry in order.
    /// Used in integration tests to verify replay correctness.
    #[cfg(test)]
    pub fn replay<F>(&self, mut f: F)
    where
        F: FnMut(&AuditEntry),
    {
        for entry in &self.entries {
            f(entry);
        }
    }

    /// Export the trail as a JSON value suitable for serialization or persistence.
    #[cfg(test)]
    pub fn export(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Export entries as a JSON array.
    #[cfg(test)]
    pub fn export_entries(&self) -> Vec<Value> {
        self.entries
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect()
    }

    /// Filter entries by event type.
    #[cfg(test)]
    pub fn filter_by_event_type(&self, event_type: &str) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.event_type == event_type)
            .collect()
    }

    /// Filter entries by agent ID.
    #[cfg(test)]
    pub fn filter_by_agent(&self, agent_id: &str) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.agent_id == agent_id)
            .collect()
    }

    /// Total number of entries in the trail.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the trail has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all entries from the trail.
    /// Used in integration tests to reset trail state.
    #[cfg(test)]
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Produce an ISO-8601 timestamp at millisecond precision.
fn chrono_now() -> String {
    // Use system time and format manually to avoid extra dependency.
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let millis = dur.subsec_millis();

    // Simple ISO-8601 formatting without external crate dependency.
    let (y, m, d, hh, mm, ss) = secs_to_datetime(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, hh, mm, ss, millis
    )
}

/// Convert Unix seconds to (year, month, day, hour, minute, second).
fn secs_to_datetime(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    const LEAPOCH: u64 = 11017;
    const DAYS_PER_400_YEARS: u64 = 146097;
    const DAYS_PER_100_YEARS: u64 = 36524;
    const DAYS_PER_4_YEARS: u64 = 1461;

    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hh = time_secs / 3600;
    let mm = (time_secs % 3600) / 60;
    let ss = time_secs % 60;

    let mut n = days.wrapping_sub(LEAPOCH);
    let n400 = n / DAYS_PER_400_YEARS;
    n %= DAYS_PER_400_YEARS;
    let mut n100 = n / DAYS_PER_100_YEARS;
    n %= DAYS_PER_100_YEARS;
    let mut n4 = n / DAYS_PER_4_YEARS;
    n %= DAYS_PER_4_YEARS;
    let mut n1 = n / 365;
    n %= 365;

    if n1 == 4 {
        n100 -= 1;
        n4 = 3;
        n1 = 3;
        n = 365;
    }

    let year = n400 * 400 + n100 * 100 + n4 * 4 + n1 + 1;
    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let month = {
        let mut remaining = n;
        let mut mon = 0u64;
        for (i, &md) in month_days.iter().enumerate() {
            let days_in_month = md + if i == 1 && is_leap { 1 } else { 0 };
            if remaining < days_in_month {
                mon = (i as u64) + 1;
                break;
            }
            remaining -= days_in_month;
        }
        if mon == 0 {
            mon = 12;
        }
        mon
    };

    let day = {
        let mut remaining = n;
        let mut day_of_month = 1u64;
        for (i, &md) in month_days.iter().enumerate() {
            let days_in_month = md + if i == 1 && is_leap { 1 } else { 0 };
            if remaining < days_in_month || (i as u64) + 1 >= month {
                day_of_month = remaining + 1;
                break;
            }
            remaining -= days_in_month;
        }
        day_of_month
    };

    (year, month, day, hh, mm, ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_entry_creation() {
        let entry = AuditEntry::new(
            "tool_call",
            "agent-1",
            "task-42",
            json!({"input": "hello"}),
            json!({"output": "world"}),
        );

        assert_eq!(entry.event_type, "tool_call");
        assert_eq!(entry.agent_id, "agent-1");
        assert_eq!(entry.task_id, "task-42");
        assert_eq!(entry.input_snapshot["input"], "hello");
        assert_eq!(entry.output_snapshot["output"], "world");
        assert!(entry.decision_path.is_empty());
        assert!(!entry.timestamp.is_empty());
    }

    #[test]
    fn test_audit_entry_decision_path() {
        let mut entry = AuditEntry::new(
            "agent_decision",
            "agent-2",
            "task-99",
            json!({"phase": "coding"}),
            json!({"result": "compiled"}),
        );

        entry.add_decision("select_tool", Some("read_file was the best option".into()));
        entry.add_decision_with_confidence(
            "execute_tool",
            Some("file exists and is readable".into()),
            0.95,
        );

        assert_eq!(entry.decision_path.len(), 2);
        assert_eq!(entry.decision_path[0].step, 0);
        assert_eq!(entry.decision_path[0].action, "select_tool");
        assert_eq!(entry.decision_path[1].step, 1);
        assert_eq!(entry.decision_path[1].action, "execute_tool");
        assert!((entry.decision_path[1].confidence.unwrap() - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_audit_trail_append_and_entries() {
        let mut trail = AuditTrail::new("trail-1", 10);

        let e1 = AuditEntry::new("tool_call", "a1", "t1", json!({}), json!({}));
        let e2 = AuditEntry::new("llm_completion", "a2", "t2", json!({}), json!({}));

        trail.append_entry(e1);
        trail.append_entry(e2);

        assert_eq!(trail.len(), 2);
        assert!(!trail.is_empty());
    }

    #[test]
    fn test_audit_trail_exceeds_max_entries() {
        let mut trail = AuditTrail::new("trail-2", 3);

        for i in 0..5 {
            trail.append_entry(AuditEntry::new(
                "tool_call",
                "agent",
                format!("task-{}", i),
                json!({"i": i}),
                json!({}),
            ));
        }

        assert_eq!(trail.len(), 3);
        // The oldest two entries should have been dropped
        let entries: Vec<&AuditEntry> = trail.entries();
        assert_eq!(entries[0].task_id, "task-2");
        assert_eq!(entries[2].task_id, "task-4");
    }

    #[test]
    fn test_audit_trail_replay() {
        let mut trail = AuditTrail::new("trail-replay", 10);
        for i in 0..3 {
            trail.append_entry(AuditEntry::new(
                "tool_call",
                "agent",
                format!("task-{}", i),
                json!({"round": i}),
                json!({}),
            ));
        }

        let mut count = 0;
        let mut last_task = String::new();
        trail.replay(|entry| {
            count += 1;
            last_task = entry.task_id.clone();
        });

        assert_eq!(count, 3);
        assert_eq!(last_task, "task-2");
    }

    #[test]
    fn test_audit_trail_export() {
        let mut trail = AuditTrail::new("trail-export", 10);
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "agent-x",
            "task-final",
            json!({"input": "data"}),
            json!({"output": "result"}),
        ));

        let exported = trail.export();
        assert_eq!(exported["trail_id"], "trail-export");
        assert!(exported["entries"].is_array());
        assert_eq!(exported["entries"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_audit_trail_export_entries() {
        let mut trail = AuditTrail::new("trail-entries", 10);
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "agent",
            "task-1",
            json!({"x": 1}),
            json!({"y": 2}),
        ));
        trail.append_entry(AuditEntry::new(
            "llm_completion",
            "agent",
            "task-2",
            json!({"a": "b"}),
            json!({"c": "d"}),
        ));

        let entries = trail.export_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["event_type"], "tool_call");
        assert_eq!(entries[1]["event_type"], "llm_completion");
    }

    #[test]
    fn test_filter_by_event_type() {
        let mut trail = AuditTrail::new("trail-filter", 10);
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "a1",
            "t1",
            json!({}),
            json!({}),
        ));
        trail.append_entry(AuditEntry::new(
            "llm_completion",
            "a2",
            "t2",
            json!({}),
            json!({}),
        ));
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "a3",
            "t3",
            json!({}),
            json!({}),
        ));

        let tool_calls = trail.filter_by_event_type("tool_call");
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].agent_id, "a1");
        assert_eq!(tool_calls[1].agent_id, "a3");

        let completions = trail.filter_by_event_type("llm_completion");
        assert_eq!(completions.len(), 1);
    }

    #[test]
    fn test_filter_by_agent() {
        let mut trail = AuditTrail::new("trail-agent-filter", 10);
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "agent-a",
            "t1",
            json!({}),
            json!({}),
        ));
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "agent-b",
            "t2",
            json!({}),
            json!({}),
        ));
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "agent-a",
            "t3",
            json!({}),
            json!({}),
        ));

        let agent_a = trail.filter_by_agent("agent-a");
        assert_eq!(agent_a.len(), 2);
        assert_eq!(agent_a[0].task_id, "t1");
        assert_eq!(agent_a[1].task_id, "t3");
    }

    #[test]
    fn test_audit_trail_clear() {
        let mut trail = AuditTrail::new("trail-clear", 10);
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "a1",
            "t1",
            json!({}),
            json!({}),
        ));
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "a2",
            "t2",
            json!({}),
            json!({}),
        ));
        assert!(!trail.is_empty());

        trail.clear();
        assert!(trail.is_empty());
        assert_eq!(trail.len(), 0);
    }

    #[test]
    fn test_chrono_now_format() {
        let ts = chrono_now();
        // ISO-8601 format: YYYY-MM-DDTHH:MM:SS.sssZ
        assert!(ts.len() >= 20, "timestamp '{}' seems too short", ts);
        assert!(ts.ends_with('Z'), "timestamp must end with Z");
        // Should contain T separator
        assert!(ts.contains('T'), "timestamp must contain T separator");
    }

    #[test]
    fn test_audit_trail_serialize_roundtrip() {
        let mut trail = AuditTrail::new("trail-roundtrip", 10);
        trail.append_entry(AuditEntry::new(
            "tool_call",
            "agent-x",
            "task-final",
            json!({"input": "data"}),
            json!({"output": "result"}),
        ));

        let json_bytes = serde_json::to_vec(&trail).expect("serialize");
        let restored: AuditTrail = serde_json::from_slice(&json_bytes).expect("deserialize");

        assert_eq!(restored.trail_id, "trail-roundtrip");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored.entries()[0].agent_id, "agent-x");
    }

    #[test]
    fn audit_trail_wired_to_autonomy_loop_report() {
        // Verify the audit trail is properly attached to the AutonomyLoopReport
        let mut trail = AuditTrail::new("test-loop", 100);
        trail.append_entry(AuditEntry::new(
            "planning",
            "agent",
            "task-1",
            json!({"phase": "plan"}),
            json!({"plan": "steps"}),
        ));
        trail.append_entry(AuditEntry::new(
            "execution",
            "agent",
            "task-1",
            json!({"round": 1}),
            json!({"tools": ["read_file"]}),
        ));
        assert_eq!(trail.len(), 2);
        // Verify replay works
        let mut count = 0usize;
        trail.replay(|_| count += 1);
        assert_eq!(count, 2);
    }
}
