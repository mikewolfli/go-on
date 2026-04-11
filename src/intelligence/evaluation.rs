//! Trace event model used by ACP request/chat runtime paths.
//!
//! This module intentionally keeps only the data model that is actively used on
//! the main chain. Unwired evaluation-suite scaffolding has been removed to
//! avoid dead-code drift and false-closure complexity.

use serde::{Deserialize, Serialize};

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
