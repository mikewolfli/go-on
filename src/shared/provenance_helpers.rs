//! Shared provenance types and helpers.
//!
//! Extracted from `observability::provenance` to break the circular
//! dependency: acp → observability → intelligence → acp.
//!
//! Contains the pure-data types (`ProvenanceEntry`), the builder
//! (`ProvenanceEntryBuilder`), and standalone factory functions
//! (`make_entry`, `make_entry_with_rationale`) that are used by
//! `intelligence::capability_bus` and `acp` modules.

use serde::{Deserialize, Serialize};

/// A single provenance entry — pure data, no logic.
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

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Helper to create a provenance entry.
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
        input_digest: digest(input),
        output_digest: digest(output),
        upstream_ids,
        timestamp_ms: crate::shared::timestamps::now_ts_ms() as u64,
        rationale: None,
        metadata: serde_json::Value::Object(Default::default()),
    }
}

/// Compute a SHA-256 hex digest of a JSON value.
///
/// This is the standalone version used by both `acp` and `intelligence`
/// without needing to import `ProvenanceLedger`.
pub fn digest(value: &serde_json::Value) -> String {
    let s = value.to_string();
    crate::shared::sha256_hex(s.as_bytes())
}
