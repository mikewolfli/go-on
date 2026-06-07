use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio::task::JoinHandle;

/// Maximum number of messages to keep per session to prevent unbounded memory growth.
pub const MAX_MESSAGES: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub timestamp: u64,
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub comparison_id: u64,
    #[serde(default)]
    pub input_tokens: usize,
    #[serde(default)]
    pub output_tokens: usize,
    #[serde(default)]
    pub total_tokens: usize,
    #[serde(default)]
    pub thinking: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseRecord {
    pub phase: String,
    pub agent: String,
    pub status: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub messages: Vec<Message>,
    pub created_at: u64,
    #[serde(default)]
    pub workflow_type: String,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub phase_records: Vec<PhaseRecord>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub branch_id: Option<String>,
}

impl Session {
    /// Push a message, enforcing the maximum message limit.
    /// If the limit is exceeded, the oldest messages are removed.
    pub fn push_message(&mut self, msg: Message) {
        self.messages.push(msg);
        if self.messages.len() > crate::views::chat::types::MAX_MESSAGES {
            let excess = self.messages.len() - crate::views::chat::types::MAX_MESSAGES;
            self.messages.drain(0..excess);
        }
    }
}

fn default_model() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: String,
    pub name: String,
    pub command: String,
    pub content: String,
}

pub struct GenerationState {
    pub id: u64,
    pub msg_idx: usize,
    pub model: String,
    pub started_at: Instant,
    pub handle: JoinHandle<()>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AiStatus {
    Idle,
    Thinking,
    Error,
}

pub enum PendingResponse {
    ChatCompleted {
        generation_id: u64,
        content: String,
        thinking: String,
        agent: String,
        /// The model that was actually used (e.g. Copilot auto-select resolution).
        model: Option<String>,
        conversation_id: Option<String>,
        branch_id: Option<String>,
    },
    StreamChunk {
        generation_id: u64,
        token: String,
        reasoning: String,
    },
    TokenEconomy {
        generation_id: u64,
        input_tokens: usize,
        output_tokens: usize,
        total_tokens: usize,
    },
    Error {
        generation_id: Option<u64>,
        message: String,
    },
    Phases(Vec<String>),
    /// Agent name → list of model IDs. Use a HashMap to preserve agent-grouping.
    Models(std::collections::HashMap<String, Vec<String>>),
    UiMessage(String),
    ExternalEditorResult(String),
}

/// A pre-rendered markdown segment that can be displayed without re-parsing.
/// Produced on a background thread via spawn_blocking to avoid UI thread blocking.
#[derive(Debug, Clone)]
pub enum MarkdownSegment {
    /// Plain text with optional styling
    Text(String, MarkdownStyle),
    /// Code block with language and content
    CodeBlock(String, String),
    /// Inline code
    InlineCode(String),
    /// Thematic break (horizontal rule)
    ThematicBreak,
    /// Heading with level and text
    Heading(u8, String),
    /// List item with prefix ("• " or "1. " etc.)
    ListItem(String, Vec<MarkdownSegment>),
    /// Blockquote containing segments
    BlockQuote(Vec<MarkdownSegment>),
    /// Link with URL and label
    Link(String, String),
    /// Image with URL and alt text
    Image(String, String),
    /// Soft or hard line break
    LineBreak,
    /// Raw text (no markdown interpretation, plain label)
    Raw(String),
}

/// Styling attributes for markdown text segments
#[derive(Debug, Clone, Default)]
pub struct MarkdownStyle {
    pub bold: bool,
    pub italic: bool,
    pub font_size: f32,
}

/// Cache entry for pre-rendered markdown content.
#[derive(Debug, Clone)]
pub struct CachedMarkdownRender {
    pub segments: Vec<MarkdownSegment>,
}

/// Model performance statistics for caching and analysis
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelPerfStats {
    pub response_time_ms: u64,
    pub token_count: usize,
    pub success_count: u32,
    pub error_count: u32,
    pub avg_tokens_per_minute: f64,
}
