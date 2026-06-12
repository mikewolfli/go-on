//! S12: Provenance Ledger
//!
//! Appends an immutable provenance record for every tool call, showing the
//! data lineage chain: input → tool → output → consumer.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub use crate::shared::provenance_helpers::ProvenanceEntry;

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
        crate::shared::provenance_helpers::digest(value)
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

    /// Record a high-level provenance entry for a chat request execution.
    ///
    /// This is the canonical entry point for recording provenance in the
    /// chat request path. It creates a single `ProvenanceEntry` summarising
    /// the overall execution outcome of a chat request.
    ///
    /// # Arguments
    ///
    /// * `task_id`         – Unique trace/request identifier.
    /// * `task_description`– Natural-language description of the user's request.
    /// * `agent`           – The agent that handled the request.
    /// * `success`         – Whether the request completed without error.
    /// * `duration_ms`     – Total elapsed time for the request in milliseconds.
    pub async fn record_provenance(
        &self,
        task_id: &str,
        task_description: &str,
        agent: &str,
        success: bool,
        duration_ms: u64,
    ) -> Result<(), anyhow::Error> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let entry = ProvenanceEntry {
            id: format!("prov-{}-{}", task_id, now_ms),
            task_id: task_id.to_string(),
            phase: "chat_execution".to_string(),
            agent: agent.to_string(),
            tool: "chat".to_string(),
            input_digest: Self::digest(&serde_json::json!({
                "description": task_description
            })),
            output_digest: Self::digest(&serde_json::json!({
                "success": success,
                "duration_ms": duration_ms,
            })),
            upstream_ids: Vec::new(),
            timestamp_ms: now_ms,
            rationale: Some(format!(
                "Chat execution by agent '{}': {}",
                agent,
                if success { "success" } else { "failed" }
            )),
            metadata: serde_json::json!({
                "success": success,
                "duration_ms": duration_ms,
                "task_description": task_description,
            }),
        };

        self.append(entry);
        Ok(())
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

#[allow(dead_code)] // F-GAP-49 — reserved provenance wiring
fn now_ms() -> u64 {
    crate::shared::timestamps::now_ts_ms() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::provenance_helpers::make_entry;
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
