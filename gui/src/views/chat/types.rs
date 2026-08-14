use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio::task::JoinHandle;

/// Maximum number of messages to keep per session to prevent unbounded memory growth.
pub const MAX_MESSAGES: usize = 1000;

/// A single segment of a message — either thinking or response content.
/// Segments are ordered chronologically so the renderer can display them interleaved,
/// matching Zed Chat's real-time thinking + response flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageSegment {
    /// Thinking/reasoning content (displayed with distinct background).
    Thinking(String),
    /// Response content (normal display).
    Content(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    /// Full response text (legacy field, kept for backward compatibility).
    /// New code should use `segments` instead.
    #[serde(default)]
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
    /// Legacy thinking field (kept for backward compatibility).
    #[serde(default)]
    pub thinking: String,
    #[serde(default)]
    pub sub_agent_records: Vec<SubAgentRecord>,
    #[serde(default)]
    pub command_records: Vec<CommandRecord>,
    /// Zed-style interleaved segments: Thinking/Content pairs in chronological order.
    #[serde(default)]
    pub segments: Vec<MessageSegment>,
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
        /// The mode that was actually used by the backend.
        /// When set, the GUI should sync session.mode to this value.
        actual_mode: Option<String>,
    },
    StreamChunk {
        generation_id: u64,
        token: String,
        reasoning: String,
    },
    /// Lightweight progress status from the backend (e.g. "Checking for
    /// prompt injection...", "Pre-fetching URLs..."). Shown in the AI
    /// thinking indicator — NOT in the message's thinking panel, which is
    /// reserved for the model's actual reasoning.
    AgentStatus {
        message: String,
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
    SubAgentEvent {
        generation_id: u64,
        agent: String,
        action: String,
        status: String,
        input: String,
        output: String,
    },
    CommandOutput {
        generation_id: u64,
        command: String,
        working_dir: String,
        exit_code: i32,
        stdout: String,
        stderr: String,
        duration_ms: u64,
    },
    /// A tool approval request received during streaming (edit/safeguard modes).
    /// When the backend emits a `chat.stream.tool_approval` event, this variant
    /// carries the tool name, its arguments, the current mode, and a risk score
    /// so the UI can prompt the user to approve or deny execution.
    ToolApprovalRequest {
        /// The generation that triggered the approval request.
        generation_id: u64,
        tool_name: String,
        /// JSON-serialised tool arguments provided by the agent.
        tool_args: serde_json::Value,
        /// Operation mode at time of approval request (edit, safeguard).
        mode: String,
        /// Risk score from the backend (0.0–1.0).
        risk_score: f64,
        message: String,
    },
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
    /// Raw text (no markdown interpretation, plain label)
    Raw(String),
    /// Inline math rendered as SVG (`$...$`)
    MathInline(String),
    /// Display math rendered as SVG (`$$...$$`)
    MathDisplay(String),
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

/// A record of a sub-agent execution run (Zed-style collapsible panel)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRecord {
    pub agent_name: String,
    pub action: String,
    pub status: String, // "running", "completed", "failed"
    pub input: String,
    pub output: String,
    pub tool_calls: Vec<SubAgentToolCall>,
    pub started_at: u64, // unix seconds
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentToolCall {
    pub tool_name: String,
    pub arguments: String, // JSON string
    pub result: String,
    pub duration_ms: u64,
}

/// A record of a command execution (cmd window)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRecord {
    pub command: String,
    pub working_dir: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
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

/// Frontend-side mode policy, mirroring the backend ModeRuntime for
/// client-side constraints (tool filtering, risk display, approval).
///
/// This struct is created by ChatView based on selected_mode and used
/// to apply mode-aware UI behavior without round-tripping to the backend.
#[derive(Debug, Clone)]
pub struct ModePolicy {
    /// Canonical mode name (ask, plan, edit, safeguard, full_auto)
    pub mode: String,
    /// Tools allowed in this mode
    pub allowed_tools: Vec<String>,
    /// Maximum tool calls per turn
    pub max_tool_calls: usize,
    /// Whether user approval is shown for tool calls
    pub show_tool_approval: bool,
    /// Whether risk assessment is displayed
    pub show_risk_display: bool,
    /// Whether sub-agent panels are auto-expanded
    pub expand_sub_agents: bool,
    /// Whether progress steps are shown
    pub show_progress_steps: bool,
}

impl ModePolicy {
    pub fn new(mode: &str) -> Self {
        /// All available tools for full execution modes.
        /// Mirrors the backend's all_execution_tools() list.
        fn execution_tools() -> Vec<String> {
            vec![
                // File tools
                "read_file".into(),
                "read_file_lines".into(),
                "write_file".into(),
                "apply_patch".into(),
                "file_move".into(),
                "file_delete".into(),
                "copy_path".into(),
                "create_directory".into(),
                "format_code".into(),
                "hash_file".into(),
                "file_watch".into(),
                // Search tools
                "search_files".into(),
                "grep".into(),
                "code_index_search".into(),
                "go_to_definition".into(),
                "find_references".into(),
                // Git / Diff
                "inspect_git_diff".into(),
                "diff".into(),
                "git".into(),
                // Build / Test / Lint
                "cargo_check".into(),
                "cargo_test".into(),
                "run_tests".into(),
                "run_build".into(),
                "lint_code".into(),
                "diagnostics".into(),
                // Shell / Execution
                "shell_exec".into(),
                "bash".into(),
                "execute_command".into(),
                // Directory
                "list_directory".into(),
                // Archive
                "archive_inspect".into(),
                "archive_extract".into(),
                "compress".into(),
                "decompress".into(),
                // Network
                "http_request".into(),
                "web_search".into(),
                "dns_lookup".into(),
                "ping".into(),
                "port_scan".into(),
                // Data
                "jsonl_read".into(),
                "jsonl_write".into(),
                "json_query".into(),
                "rss_read".into(),
                // Docker
                "docker_ps".into(),
                "docker_logs".into(),
                "docker_exec".into(),
                "docker_build".into(),
                "docker_push".into(),
                "docker_compose".into(),
                // Utility
                "date_time".into(),
                "environment_info".into(),
                "uuid_gen".into(),
                "random_token".into(),
                "encode_decode".into(),
                "template_render".into(),
                "code_metrics".into(),
                "security_scan".into(),
                "search_packages".into(),
                "add_dependency".into(),
                // Agent tools
                "spawn_agent".into(),
                "apply_code_action".into(),
                // Skill tools
                "skill_list".into(),
                "skill_execute".into(),
                "skill_create".into(),
                "skill_reload".into(),
            ]
        }

        /// Read-only tools for Plan mode and SafeGuard ReadOnly degradation.
        fn read_only_tools() -> Vec<String> {
            vec![
                "read_file".into(),
                "read_file_lines".into(),
                "search_files".into(),
                "grep".into(),
                "list_directory".into(),
                "inspect_git_diff".into(),
                "code_index_search".into(),
                "go_to_definition".into(),
                "find_references".into(),
                "diff".into(),
                "date_time".into(),
                "environment_info".into(),
                "json_query".into(),
                "archive_inspect".into(),
                "dns_lookup".into(),
                "ping".into(),
                "docker_ps".into(),
                "docker_logs".into(),
                "rss_read".into(),
                "jsonl_read".into(),
                "code_metrics".into(),
                "security_scan".into(),
                "skill_list".into(),
                "web_search".into(),
            ]
        }

        match mode {
            "ask" => Self {
                mode: "ask".to_string(),
                // Low-risk (ReadOnly) tools only
                allowed_tools: read_only_tools(),
                max_tool_calls: 5,
                show_tool_approval: false,
                show_risk_display: false,
                expand_sub_agents: false,
                show_progress_steps: false,
            },
            "plan" => Self {
                mode: "plan".to_string(),
                allowed_tools: read_only_tools(),
                max_tool_calls: 3,
                show_tool_approval: false,
                show_risk_display: true,
                expand_sub_agents: false,
                show_progress_steps: true,
            },
            "edit" => Self {
                mode: "edit".to_string(),
                allowed_tools: execution_tools(),
                max_tool_calls: 20,
                show_tool_approval: true,
                show_risk_display: true,
                expand_sub_agents: false,
                show_progress_steps: false,
            },
            "safeguard" => Self {
                mode: "safeguard".to_string(),
                allowed_tools: execution_tools(),
                max_tool_calls: 30,
                show_tool_approval: true,
                show_risk_display: true,
                expand_sub_agents: true,
                show_progress_steps: true,
            },
            "full_auto" => Self {
                mode: "full_auto".to_string(),
                allowed_tools: execution_tools(),
                max_tool_calls: 50,
                show_tool_approval: false,
                show_risk_display: true,
                expand_sub_agents: true,
                show_progress_steps: true,
            },
            _ => Self::new("edit"),
        }
    }

    /// Compute a simple risk score for display purposes (0.0–1.0).
    /// Mirrors the backend's compute_risk_score heuristic.
    /// Must stay in sync with `classify_tool_risk` in tool_governance_defaults.rs.
    pub fn compute_risk_score(&self, objective: &str) -> f64 {
        let lower = objective.to_lowercase();
        let mut score: f64 = 0.0;

        // High-risk keywords (word-boundary match, +0.25 each, max +0.75)
        let high = [
            "delete",
            "remove",
            "drop",
            "rm",
            "truncate",
            "rollback",
            "revert",
            "reset",
            "force",
            "uninstall",
            "shutdown",
        ];
        for kw in &high {
            if word_boundary_match(&lower, kw) {
                score += 0.25;
            }
        }
        score = score.min(0.75);

        // Medium-risk keywords (word-boundary match, +0.10 each, max +0.40)
        let medium = [
            "write", "edit", "modify", "update", "create", "patch", "rename", "move", "copy",
        ];
        for kw in &medium {
            if word_boundary_match(&lower, kw) {
                score += 0.10;
            }
        }
        let medium_overage = score - 0.75;
        if medium_overage > 0.0 {
            let medium_cap = 0.40;
            let medium_portion = medium_overage.min(medium_cap);
            score = 0.75 + medium_portion;
        }

        score.min(1.0)
    }
}

/// Check if `text` contains `word` as a complete word (word-boundary matching).
/// Splits on non-alphanumeric characters to avoid false positives like
/// `"undelete"` matching `delete` or `"dropdown"` matching `drop`.
fn word_boundary_match(text: &str, word: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric())
        .any(|w| w == word)
}
