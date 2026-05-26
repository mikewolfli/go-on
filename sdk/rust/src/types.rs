//! Request and response types for the go-on Rust SDK.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Chat types (streaming support)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
    #[serde(default)]
    pub modules: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceStatusResponse {
    pub ok: bool,
    pub governance: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthProbesResponse {
    pub modules: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsResponse {
    pub metrics: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakerStatusResponse {
    pub breakers: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointListResponse {
    pub checkpoints: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlanResponse {
    pub plan: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningSummaryResponse {
    pub summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorStatusResponse {
    pub selector: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostStatusResponse {
    pub cost: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigBaselineResponse {
    pub baseline: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessStatusResponse {
    pub harness: Value,
}
