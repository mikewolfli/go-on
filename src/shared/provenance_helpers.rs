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

fn now_ms() -> u64 {
    crate::shared::timestamps::now_ts_ms() as u64
}

/// Builder for constructing a [`ProvenanceEntry`] with a fluent API.
#[allow(dead_code)]
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

#[allow(dead_code)]
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
            input_digest: digest(&self.input),
            output_digest: digest(&self.output),
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
#[allow(dead_code, clippy::too_many_arguments)]
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

/// Compute a SHA-256 hex digest of a JSON value.
///
/// This is the standalone version used by both `acp` and `intelligence`
/// without needing to import `ProvenanceLedger`.
pub fn digest(value: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let s = value.to_string();
    let hash = Sha256::digest(s.as_bytes());
    hash.iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .concat()
}
