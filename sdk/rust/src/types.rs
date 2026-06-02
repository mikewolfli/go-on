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

// ---------------------------------------------------------------------------
// BLUE56-E05: Missing key types for complete SDK coverage
// ---------------------------------------------------------------------------

/// Record of a tool call made by an agent (who called what tool with which args).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Name of the tool that was called.
    pub tool_name: String,
    /// Arguments passed to the tool.
    pub arguments: serde_json::Value,
    /// The agent that made the call.
    pub agent_name: String,
    /// Optional result of the tool execution.
    pub result: Option<serde_json::Value>,
    /// Duration of the tool call in milliseconds.
    pub duration_ms: u64,
}

/// Multimodal input types (images, documents, audio) for rich chat requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MultimodalInput {
    /// Plain text content.
    Text {
        /// The text content.
        text: String,
    },
    /// Image content (base64-encoded or URL).
    Image {
        /// Image URL or base64 data URI.
        image_url: String,
        /// Optional detail level ("auto", "low", "high").
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Document content (PDF, DOCX, etc.).
    Document {
        /// File data as base64.
        data: String,
        /// MIME type of the document.
        mime_type: String,
        /// Optional filename.
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
    /// Audio content.
    Audio {
        /// Audio data as base64.
        data: String,
        /// Audio format (e.g. "wav", "mp3").
        format: String,
    },
}

/// A single chunk in an SSE streaming response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// The token text content.
    pub token: String,
    /// Whether this is the final chunk.
    #[serde(default)]
    pub done: bool,
    /// Optional reasoning content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Optional tool calls included in this chunk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Chunk index in the stream.
    pub index: usize,
    /// Total characters sent so far.
    pub total_chars: usize,
}

/// Metadata about an available agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Unique agent name/ID.
    pub name: String,
    /// Agent type (e.g. "copilot", "custom").
    pub agent_type: String,
    /// Human-readable description.
    pub description: String,
    /// Available model names this agent can use.
    #[serde(default)]
    pub models: Vec<String>,
    /// Capability tags (e.g. "coding", "review", "general").
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Whether this agent is currently healthy/available.
    #[serde(default = "default_true")]
    pub healthy: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessStatusResponse {
    pub harness: Value,
}
