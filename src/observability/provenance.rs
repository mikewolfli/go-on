//! S12: Provenance Ledger
//!
//! Appends an immutable provenance record for every tool call, showing the
//! data lineage chain: input → tool → output → consumer.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A single provenance entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub id: String,
    pub task_id: String,
    pub phase: String,
    pub agent: String,
    pub tool: String,
    pub input_digest: String,
    pub output_digest: String,
    /// Chain of upstream provenance IDs this output depends on
    pub upstream_ids: Vec<String>,
    pub timestamp_ms: u64,
    /// Optional human- or agent-readable justification for this entry
    pub rationale: Option<String>,
    /// Arbitrary metadata associated with this entry
    pub metadata: serde_json::Value,
}

/// In-process provenance ledger (append-only, bounded)
#[derive(Debug, Clone)]
pub struct ProvenanceLedger {
    inner: Arc<Mutex<ProvenanceLedgerInner>>,
}

#[derive(Debug)]
struct ProvenanceLedgerInner {
    entries: VecDeque<ProvenanceEntry>,
    max_entries: usize,
}

impl Default for ProvenanceLedger {
    fn default() -> Self {
        Self::new(2000)
    }
}

impl ProvenanceLedger {
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProvenanceLedgerInner {
                entries: VecDeque::new(),
                max_entries,
            })),
        }
    }

    /// Append a new entry; oldest entries are evicted when at capacity.
    pub fn append(&self, entry: ProvenanceEntry) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "provenance", "Mutex poisoned – recovering in append");
            poisoned.into_inner()
        });
        if inner.entries.len() >= inner.max_entries {
            inner.entries.pop_front();
        }
        inner.entries.push_back(entry);
    }

    /// Get all entries for a given task_id
    pub fn entries_for_task(&self, task_id: &str) -> Vec<ProvenanceEntry> {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "provenance", "Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        inner
            .entries
            .iter()
            .filter(|e| e.task_id == task_id)
            .cloned()
            .collect()
    }

    /// Build a digest of the input value (SHA-256 hex).
    /// Uses the `sha2` crate to produce a deterministic cryptographic hash.
    pub fn digest(value: &serde_json::Value) -> String {
        use sha2::{Digest, Sha256};
        let s = value.to_string();
        let hash = Sha256::digest(s.as_bytes());
        hash.iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .concat()
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.entries.len())
            .unwrap_or_else(|poisoned| {
                tracing::warn!(target: "provenance", "Mutex poisoned – recovering in len");
                poisoned.into_inner().entries.len()
            })
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Compute a content-digest of the entire ledger (SHA-256 hex).
    pub fn ledger_digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let combined: String = {
            let inner = self.inner.lock().unwrap_or_else(|poisoned| {
                tracing::warn!(target: "provenance", "Mutex poisoned – recovering in ledger_digest");
                poisoned.into_inner()
            });
            inner
                .entries
                .iter()
                .map(|e| {
                    format!(
                        "{}|{}|{}|{}|{}",
                        e.id, e.task_id, e.phase, e.tool, e.timestamp_ms
                    )
                })
                .collect::<Vec<_>>()
                .join("::")
        };
        let hash = Sha256::digest(combined.as_bytes());
        hash.iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .concat()
    }

    /// Query entries by phase (action type).
    pub fn entries_by_phase(&self, phase: &str) -> Vec<ProvenanceEntry> {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "provenance", "Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        inner
            .entries
            .iter()
            .filter(|e| e.phase == phase)
            .cloned()
            .collect()
    }

    /// Query entries by tool (component).
    pub fn entries_by_tool(&self, tool: &str) -> Vec<ProvenanceEntry> {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "provenance", "Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        inner
            .entries
            .iter()
            .filter(|e| e.tool == tool)
            .cloned()
            .collect()
    }

    /// Query entries within a time window (inclusive, in milliseconds since Unix epoch).
    pub fn entries_between(&self, start_ms: u64, end_ms: u64) -> Vec<ProvenanceEntry> {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "provenance", "Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        inner
            .entries
            .iter()
            .filter(|e| e.timestamp_ms >= start_ms && e.timestamp_ms <= end_ms)
            .cloned()
            .collect()
    }

    /// Clear all entries (for testing / reset).
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "provenance", "Mutex poisoned – recovering in clear");
            poisoned.into_inner()
        });
        inner.entries.clear();
    }
}

// ── ProvenanceEntryBuilder ──────────────────────────────────────────────────

/// Builder for [`ProvenanceEntry`] that avoids long argument lists.
///
/// # Usage
///
/// ```ignore
/// use crate::observability::provenance::ProvenanceEntryBuilder;
///
/// let entry = ProvenanceEntryBuilder::new("task-001", "chat", "agent-a", "read_file")
///     .input(&serde_json::json!({"path": "/foo"}))
///     .output(&serde_json::json!({"content": "..."}))
///     .upstream_ids(vec!["prev-id".to_string()])
///     .build();
/// ```
#[allow(dead_code)] // Public API — reserved for adoption over the old positional functions
pub struct ProvenanceEntryBuilder {
    task_id: String,
    phase: String,
    agent: String,
    tool: String,
    input: serde_json::Value,
    output: serde_json::Value,
    upstream_ids: Vec<String>,
    rationale: Option<String>,
    metadata: serde_json::Value,
}

#[allow(dead_code)] // Public API — reserved for adoption over the old positional functions
impl ProvenanceEntryBuilder {
    /// Start building a provenance entry with the minimum required fields.
    /// `input` and `output` default to `serde_json::Value::Null`; call
    /// `.input()` / `.output()` to override.
    pub fn new(task_id: &str, phase: &str, agent: &str, tool: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            phase: phase.to_string(),
            agent: agent.to_string(),
            tool: tool.to_string(),
            input: serde_json::Value::Null,
            output: serde_json::Value::Null,
            upstream_ids: vec![],
            rationale: None,
            metadata: serde_json::Value::Object(Default::default()),
        }
    }

    /// Set the input value (used to compute `input_digest`).
    pub fn input(mut self, value: &serde_json::Value) -> Self {
        self.input = value.clone();
        self
    }

    /// Set the output value (used to compute `output_digest`).
    pub fn output(mut self, value: &serde_json::Value) -> Self {
        self.output = value.clone();
        self
    }

    /// Set the upstream provenance IDs.
    pub fn upstream_ids(mut self, ids: Vec<String>) -> Self {
        self.upstream_ids = ids;
        self
    }

    /// Attach an optional rationale string.
    pub fn rationale(mut self, rationale: &str) -> Self {
        self.rationale = Some(rationale.to_string());
        self
    }

    /// Set arbitrary metadata.
    pub fn metadata(mut self, meta: serde_json::Value) -> Self {
        self.metadata = meta;
        self
    }

    /// Consume the builder and produce a [`ProvenanceEntry`].
    pub fn build(self) -> ProvenanceEntry {
        ProvenanceEntry {
            id: uuid_v4(),
            task_id: self.task_id,
            phase: self.phase,
            agent: self.agent,
            tool: self.tool,
            input_digest: ProvenanceLedger::digest(&self.input),
            output_digest: ProvenanceLedger::digest(&self.output),
            upstream_ids: self.upstream_ids,
            timestamp_ms: now_ms(),
            rationale: self.rationale,
            metadata: self.metadata,
        }
    }
}

/// Helper to create a provenance entry.
///
/// Prefer [`ProvenanceEntryBuilder`] for new code — it avoids the long
/// parameter list and makes call sites self-documenting.
pub fn make_entry(
    task_id: &str,
    phase: &str,
    agent: &str,
    tool: &str,
    input: &serde_json::Value,
    output: &serde_json::Value,
    upstream_ids: Vec<String>,
) -> ProvenanceEntry {
    ProvenanceEntryBuilder::new(task_id, phase, agent, tool)
        .input(input)
        .output(output)
        .upstream_ids(upstream_ids)
        .build()
}

/// Helper to create a provenance entry with an optional rationale.
///
/// Prefer [`ProvenanceEntryBuilder`] for new code — it avoids the long
/// parameter list and makes call sites self-documenting.
#[allow(dead_code, clippy::too_many_arguments)] // Reserved for future wiring — callers will adopt ProvenanceEntryBuilder
pub fn make_entry_with_rationale(
    task_id: &str,
    phase: &str,
    agent: &str,
    tool: &str,
    input: &serde_json::Value,
    output: &serde_json::Value,
    upstream_ids: Vec<String>,
    rationale: Option<String>,
) -> ProvenanceEntry {
    let mut builder = ProvenanceEntryBuilder::new(task_id, phase, agent, tool)
        .input(input)
        .output(output)
        .upstream_ids(upstream_ids);
    if let Some(r) = rationale {
        builder = builder.rationale(&r);
    }
    builder.build()
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_ms() -> u64 {
    crate::acp::prelude::now_ts_ms() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_test_entry(
        task_id: &str,
        phase: &str,
        agent: &str,
        tool: &str,
        input: &serde_json::Value,
        output: &serde_json::Value,
        upstream_ids: Vec<String>,
    ) -> ProvenanceEntry {
        make_entry(task_id, phase, agent, tool, input, output, upstream_ids)
    }

    #[test]
    fn test_append_and_length() {
        let ledger = ProvenanceLedger::new(100);
        let entry = make_test_entry(
            "task1",
            "exec",
            "agent1",
            "component",
            &json!({"file": "x"}),
            &json!({"result": "ok"}),
            vec![],
        );
        ledger.append(entry);
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn test_entries_for_task() {
        let ledger = ProvenanceLedger::new(100);
        ledger.append(make_test_entry(
            "task1",
            "exec",
            "agent1",
            "comp",
            &json!({}),
            &json!({}),
            vec![],
        ));
        ledger.append(make_test_entry(
            "task2",
            "exec",
            "agent1",
            "comp",
            &json!({}),
            &json!({}),
            vec![],
        ));
        assert_eq!(ledger.entries_for_task("task1").len(), 1);
        assert_eq!(ledger.entries_for_task("task3").len(), 0);
    }

    #[test]
    fn test_ledger_digest_is_consistent() {
        let ledger = ProvenanceLedger::new(100);
        ledger.append(make_test_entry(
            "t1",
            "exec",
            "a1",
            "comp",
            &json!({}),
            &json!({}),
            vec![],
        ));
        let d1 = ledger.ledger_digest();
        let d2 = ledger.ledger_digest();
        assert_eq!(d1, d2, "ledger_digest should be deterministic");
    }

    #[test]
    fn test_append_bounded_capacity() {
        let ledger = ProvenanceLedger::new(5);
        for i in 0..10 {
            ledger.append(make_test_entry(
                &format!("t{i}"),
                "exec",
                "agent",
                "comp",
                &json!({}),
                &json!({}),
                vec![],
            ));
        }
        assert_eq!(ledger.len(), 5, "should be bounded by capacity");
    }

    #[test]
    fn test_multiple_components_tracked() {
        let ledger = ProvenanceLedger::new(100);
        ledger.append(make_test_entry(
            "t1",
            "read",
            "a1",
            "reader",
            &json!({"file": "x.rs"}),
            &json!({"lines": 100}),
            vec![],
        ));
        ledger.append(make_test_entry(
            "t1",
            "write",
            "a1",
            "writer",
            &json!({"file": "x.rs"}),
            &json!({"lines": 120}),
            vec![],
        ));
        let entries = ledger.entries_for_task("t1");
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.tool == "reader"));
        assert!(entries.iter().any(|e| e.tool == "writer"));
    }

    #[test]
    fn test_entries_by_phase() {
        let ledger = ProvenanceLedger::new(100);
        ledger.append(make_test_entry(
            "t1",
            "read",
            "a1",
            "tool1",
            &json!({}),
            &json!({}),
            vec![],
        ));
        ledger.append(make_test_entry(
            "t2",
            "write",
            "a1",
            "tool1",
            &json!({}),
            &json!({}),
            vec![],
        ));
        ledger.append(make_test_entry(
            "t3",
            "read",
            "a2",
            "tool2",
            &json!({}),
            &json!({}),
            vec![],
        ));
        assert_eq!(ledger.entries_by_phase("read").len(), 2);
        assert_eq!(ledger.entries_by_phase("write").len(), 1);
        assert_eq!(ledger.entries_by_phase("delete").len(), 0);
    }

    #[test]
    fn test_entries_by_tool() {
        let ledger = ProvenanceLedger::new(100);
        ledger.append(make_test_entry(
            "t1",
            "exec",
            "a1",
            "scanner",
            &json!({}),
            &json!({}),
            vec![],
        ));
        ledger.append(make_test_entry(
            "t2",
            "exec",
            "a1",
            "scanner",
            &json!({}),
            &json!({}),
            vec![],
        ));
        ledger.append(make_test_entry(
            "t3",
            "exec",
            "a2",
            "formatter",
            &json!({}),
            &json!({}),
            vec![],
        ));
        assert_eq!(ledger.entries_by_tool("scanner").len(), 2);
        assert_eq!(ledger.entries_by_tool("formatter").len(), 1);
        assert_eq!(ledger.entries_by_tool("unknown").len(), 0);
    }

    #[test]
    fn test_entries_between() {
        let ledger = ProvenanceLedger::new(100);
        // Use a fixed timestamp by overriding the ledger's entries directly
        // Append normally first, then modify timestamps for deterministic testing
        ledger.append(make_test_entry(
            "t1",
            "exec",
            "a1",
            "tool1",
            &json!({}),
            &json!({}),
            vec![],
        ));
        ledger.append(make_test_entry(
            "t2",
            "exec",
            "a1",
            "tool1",
            &json!({}),
            &json!({}),
            vec![],
        ));

        // Get timestamps from the actual entries (they're both ~now)
        let entries = ledger.entries_for_task("t1");
        let ts = entries[0].timestamp_ms;

        // Both entries should be within a window around now
        let found = ledger.entries_between(ts - 1000, ts + 1000);
        assert_eq!(found.len(), 2, "both entries should be in the time window");

        let found_none = ledger.entries_between(0, ts.saturating_sub(100_000));
        assert_eq!(found_none.len(), 0, "no entries should match a past window");
    }

    #[test]
    fn test_clear() {
        let ledger = ProvenanceLedger::new(100);
        ledger.append(make_test_entry(
            "t1",
            "exec",
            "a1",
            "comp",
            &json!({}),
            &json!({}),
            vec![],
        ));
        assert_eq!(ledger.len(), 1);
        ledger.clear();
        assert_eq!(ledger.len(), 0);
        assert!(ledger.is_empty());
    }

    #[test]
    fn test_digest_different_inputs_differ() {
        // The static digest() should produce different hashes for different values
        let d1 = ProvenanceLedger::digest(&json!({"a": 1}));
        let d2 = ProvenanceLedger::digest(&json!({"a": 2}));
        assert_ne!(d1, d2, "different inputs should produce different digests");
    }

    #[test]
    fn test_digest_same_input_consistent() {
        let d1 = ProvenanceLedger::digest(&json!({"hello": "world"}));
        let d2 = ProvenanceLedger::digest(&json!({"hello": "world"}));
        assert_eq!(d1, d2, "same inputs should produce identical digests");
    }

    #[test]
    fn test_ledger_digest_changes_after_append() {
        let ledger = ProvenanceLedger::new(100);
        let d0 = ledger.ledger_digest();
        ledger.append(make_test_entry(
            "t1",
            "exec",
            "a1",
            "comp",
            &json!({}),
            &json!({}),
            vec![],
        ));
        let d1 = ledger.ledger_digest();
        assert_ne!(d0, d1, "digest should change after appending an entry");
    }

    #[test]
    fn test_default_capacity() {
        let ledger = ProvenanceLedger::default();
        assert_eq!(ledger.len(), 0);
        // Insert many entries to verify default capacity (2000) works
        for i in 0..2500 {
            ledger.append(make_test_entry(
                &format!("t{i}"),
                "exec",
                "agent",
                "comp",
                &json!({}),
                &json!({}),
                vec![],
            ));
        }
        assert_eq!(ledger.len(), 2000);
    }

    #[test]
    fn test_empty_ledger_properties() {
        let ledger = ProvenanceLedger::new(10);
        assert!(ledger.is_empty());
        assert_eq!(ledger.len(), 0);
        assert_eq!(ledger.entries_for_task("anything").len(), 0);
        assert_eq!(ledger.entries_by_phase("anything").len(), 0);
        assert_eq!(ledger.entries_by_tool("anything").len(), 0);
        assert!(ledger.entries_between(0, u64::MAX).is_empty());
        // Digest should still work on empty ledger
        let digest = ledger.ledger_digest();
        assert!(!digest.is_empty());
    }
}
