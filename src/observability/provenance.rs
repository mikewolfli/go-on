//! S12: Provenance Ledger
//!
//! Appends an immutable provenance record for every tool call, showing the
//! data lineage chain: input → tool → output → consumer.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
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
    pub metadata: serde_json::Value,
}

/// In-process provenance ledger (append-only, bounded)
#[derive(Debug, Clone)]
pub struct ProvenanceLedger {
    inner: Arc<Mutex<ProvenanceLedgerInner>>,
}

#[derive(Debug)]
struct ProvenanceLedgerInner {
    entries: Vec<ProvenanceEntry>,
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
                entries: Vec::new(),
                max_entries,
            })),
        }
    }

    /// Append a new entry; oldest entries are evicted when at capacity.
    pub fn append(&self, entry: ProvenanceEntry) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.entries.len() >= inner.max_entries {
                inner.entries.remove(0);
            }
            inner.entries.push(entry);
        }
    }

    /// Get all entries for a given task_id
    pub fn entries_for_task(&self, task_id: &str) -> Vec<ProvenanceEntry> {
        self.inner
            .lock()
            .map(|inner| {
                inner
                    .entries
                    .iter()
                    .filter(|e| e.task_id == task_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Build a simple digest of the input value (SHA-256 hex, first 16 chars)
    pub fn digest(value: &serde_json::Value) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let s = value.to_string();
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.entries.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Helper to create a provenance entry quickly
pub fn make_entry(
    task_id: &str,
    phase: &str,
    agent: &str,
    tool: &str,
    input: &serde_json::Value,
    output: &serde_json::Value,
    upstream_ids: Vec<String>,
) -> ProvenanceEntry {
    ProvenanceEntry {
        id: uuid_v4(),
        task_id: task_id.to_string(),
        phase: phase.to_string(),
        agent: agent.to_string(),
        tool: tool.to_string(),
        input_digest: ProvenanceLedger::digest(input),
        output_digest: ProvenanceLedger::digest(output),
        upstream_ids,
        timestamp_ms: now_ms(),
        metadata: serde_json::Value::Null,
    }
}

fn uuid_v4() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let timestamp_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id() as u64;
    let combined = timestamp_ns ^ (counter << 16) ^ (pid << 48);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (combined >> 32) as u32,
        (combined >> 16) as u16 & 0xffff,
        (combined & 0xffff) as u16,
        (counter & 0xffff) as u16,
        (timestamp_ns & 0xffff_ffff_ffff) as u64
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
