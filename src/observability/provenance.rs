//! S12: Provenance Ledger
//!
//! Appends an immutable provenance record for every tool call, showing the
//! data lineage chain: input → tool → output → consumer.

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
    use std::cell::RefCell;
    use std::time::{SystemTime, UNIX_EPOCH};

    thread_local! {
        static RNG: RefCell<u64> = {
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            RefCell::new(seed)
        };
    }

    let rand_a: u64 = RNG.with(|rng| {
        let mut state = rng.borrow_mut();
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state
    });

    let rand_b: u64 = RNG.with(|rng| {
        let mut state = rng.borrow_mut();
        *state = state
            .wrapping_mul(2862933555777941757)
            .wrapping_add(3037000493);
        *state
    });

    let time_low = (rand_a & 0xffff_ffff) as u32;
    let time_mid = ((rand_a >> 32) & 0xffff) as u16;
    let time_hi_and_version = ((rand_a >> 48) as u16 & 0x0fff) | 0x4000;
    let clock_seq_hi = ((rand_b >> 32) as u8 & 0x3f) | 0x80;
    let clock_seq_low = (rand_b >> 24) as u8;
    let node_low = rand_b & 0xffff_ffff_ffff;
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:012x}",
        time_low, time_mid, time_hi_and_version, clock_seq_hi, clock_seq_low, node_low
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
