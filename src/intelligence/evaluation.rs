//! Trace event model shared by the ACP request/chat runtime paths.
//!
//! The former F-GAP-06 evaluation suite framework (BenchmarkSuite /
//! ReplayEngine / EvaluationScore / safety-scoring stack) was removed: it had
//! zero production callers — the only wiring was an `evaluation_suite` field
//! on `RegistryContext` that was constructed and never read. Safety analysis
//! of agent output lives in [`crate::intelligence::verification`] instead.

use serde::{Deserialize, Serialize};

/// A single trace event emitted along the ACP request/chat path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub timestamp: String,
    pub event_type: String,
    pub task_id: String,
    pub phase: String,
    pub agent: Option<String>,
    pub tool: Option<String>,
    pub status: String,
    pub inputs: serde_json::Value,
    pub outputs: Option<serde_json::Value>,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub pua_stage: Option<String>,
}
