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
    /// Lifecycle snapshot — present on both `GET /health` (ServerStatus) and
    /// `runtime.health` (JSON-RPC) responses.
    #[serde(default)]
    pub lifecycle: Option<Value>,
    /// Version string — present only on `runtime.health` (JSON-RPC);
    /// `GET /health` (ServerStatus) does not emit this field.
    #[serde(default)]
    pub version: Option<String>,
    /// Request statistics — present only on `runtime.health` (JSON-RPC);
    /// `GET /health` (ServerStatus) does not emit this field.
    #[serde(default)]
    pub stats: Option<Value>,
    /// Maintenance snapshot — present on both endpoints.
    #[serde(default)]
    pub maintenance: Option<Value>,
    /// Monotonic server timestamp (ms) — present on both endpoints.
    #[serde(default)]
    pub timestamp: Option<i64>,
    /// Metrics snapshot — present only on `GET /health` (ServerStatus);
    /// `runtime.health` exposes `stats` / `review_gate` / `timeouts` instead.
    #[serde(default)]
    pub metrics: Option<Value>,
    /// Module health probes — present only on `runtime.health` (JSON-RPC);
    /// `GET /health` (ServerStatus) does not emit this field.
    #[serde(default)]
    pub modules: Option<Value>,
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

// ---------------------------------------------------------------------------
// ACP Session Protocol types
// ---------------------------------------------------------------------------

/// Request payload for `session/new`.
///
/// Field names match the backend contract exactly: the backend reads
/// `mode`, `cwd`, `work_dirs` (snake_case) and `additionalDirectories`
/// (camelCase) from `protocol_pack/session.rs::session_new_payload`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcpSessionNewRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub work_dirs: Vec<String>,
    #[serde(
        rename = "additionalDirectories",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub additional_directories: Vec<String>,
}

/// Request payload for `session/prompt`.
///
/// The backend reads `sessionId`, `prompt` (content blocks), `mode`, `cwd`
/// and `additionalDirectories` (`session_state_for_prompt` + `acp_prompt_to_text`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpSessionPromptRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub prompt: Vec<PromptContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(
        rename = "additionalDirectories",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub additional_directories: Vec<String>,
}

/// A single content block inside a `session/prompt` prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptContentBlock {
    #[serde(rename = "type")]
    pub kind: PromptContentBlockType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<PromptResourceBlock>,
}

/// The `type` discriminator of a prompt content block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum PromptContentBlockType {
    Text,
    Resource,
    ResourceLink,
    Image,
    Audio,
}

/// Embedded resource payload inside a resource-type prompt block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResourceBlock {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Response payload for `session/list`.
///
/// The backend emits a minimal summary per session: `[{"id": sid}]`
/// (`session_list_payload` in `protocol_pack/session.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpSessionListResponse {
    pub sessions: Vec<AcpSessionSummary>,
    // Wire key is `nextCursor` (camelCase): the backend `ListSessionsResponse`
    // (src/schema/agent.rs) is annotated `#[serde(rename_all = "camelCase")]`,
    // so the snake_case Rust field needs the rename below. The backend handler
    // currently always sends `next_cursor: None`, so the field is usually absent.
    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Minimal summary of an active ACP session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpSessionSummary {
    pub id: String,
}

// ---------------------------------------------------------------------------
// Tools types (tools/list, tools/call)
// ---------------------------------------------------------------------------

/// Descriptor for a tool exposed via `tools/list`.
///
/// The backend emits the input schema under the snake_case key
/// `input_schema` (see `tools_pack.rs::build_mcp_tool_descriptors`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}
