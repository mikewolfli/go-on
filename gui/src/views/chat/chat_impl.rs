use crate::backend::{AbortController, BackendClient, StreamProcessor, TokenProgress};
use crate::i18n::I18n;
use crate::views::autotune::AutoTuneView;
use crate::views::risk_decision::RiskDecisionDraft;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc;
use std::sync::Arc;

// Compile-time feature flags — not configurable at runtime.
// Change these and rebuild to toggle the corresponding feature.
const CHAT_DISABLE_MARKDOWN_RENDER: bool = false;
const CHAT_STAGE6_ENABLE_MODE_ROW: bool = true;
const CHAT_STAGE6_ENABLE_EXTRA_BUTTONS: bool = true;
const MAX_CONCURRENT_GENERATIONS: usize = 4;

#[derive(Clone, Copy)]
pub struct ChatUiRuntimeConfig {
    pub repaint_interval_ms: u64,
    pub stream_chunk_flush_ms: u64,
    pub max_pending_events_per_frame: usize,
    /// Minimum interval (ms) between flushing buffered tokens to the UI.
    /// 16ms → ~60fps; higher values reduce repaint frequency.
    pub stream_token_flush_ms: u64,
}

use super::types::{
    AiStatus, Attachment, GenerationState, Message, ModelPerfStats, PendingResponse, PhaseRecord,
    PromptTemplate, Session,
};
mod render;
mod runtime;
mod storage;
mod ui;
use crate::views::ui_state::GlobalUiState;

pub struct ChatView {
    pub sessions: Vec<Session>,
    pub active_session: usize,
    pub input: String,
    pub sending: bool,
    pub last_sending: bool,
    pub error: String,
    pub success_message: Option<String>,
    pub ai_status: AiStatus,
    pub selected_phase: String,
    pub selected_mode: String,
    pub attachments: Vec<Attachment>,
    /// Available phases fetched from backend
    phases: Vec<String>,
    /// Whether phases have been fetched
    phases_loaded: bool,
    /// Whether phases loading has been scheduled
    phases_load_scheduled: bool,
    pending_rx: mpsc::Receiver<PendingResponse>,
    pending_tx: mpsc::SyncSender<PendingResponse>,
    // Session rename (double-click on session name)
    rename_session_idx: Option<usize>,
    rename_session_buf: String,
    // Track which message's thinking content is expanded
    show_thinking_idx: Option<usize>,
    // Global "Show/Hide All Thinking" toggle
    pub show_all_thinking: bool,
    // Sub-agent panels
    pub show_all_sub_agents: bool,
    pub show_sub_agent_idx: Option<usize>,
    // Command panels
    pub show_all_commands: bool,
    pub show_command_idx: Option<usize>,
    // Message edit
    edit_msg_idx: Option<usize>,
    edit_msg_buf: String,
    // Feature 4: stop button
    stop_requested: bool,
    generation_states: Vec<GenerationState>,
    next_generation_id: u64,
    /// Track concurrent generations to prevent resource exhaustion
    active_generations: Arc<AtomicU64>,
    // Feature 5: token display with improved accuracy
    last_token_estimate: usize,
    input_token_estimate: usize,
    output_token_estimate: usize,
    /// Enable markdown rendering (default: true)
    pub enable_markdown: bool,
    /// Show token details (default: true)
    pub show_token_details: bool,
    /// Model performance stats cache
    pub model_stats: std::collections::HashMap<String, ModelPerfStats>,
    // Feature 7: quick prompts
    pub show_prompts: bool,
    pub show_model_picker: bool,
    prompt_templates: Vec<PromptTemplate>,
    next_template_id: u64,
    selected_template_idx: Option<usize>,
    template_name_buf: String,
    template_command_buf: String,
    template_content_buf: String,
    pub template_search_query: String,
    templates_bootstrapped: bool,
    /// Command templates from the PromptsView, used as fallback for `/` commands.
    pub prompts_command_templates: Vec<crate::views::prompts::CommandTemplate>,
    /// Full prompt category collection for the category browser.
    pub prompt_collection: Vec<crate::views::prompts::PromptCategory>,
    /// Currently selected category ID in the prompt browser.
    prompt_selected_category: Option<String>,
    /// Risk decision helper panel visibility and fields.
    show_risk_decision: bool,
    risk_is_high: bool,
    risk_review_required: bool,
    risk_strategy: String,
    risk_reasons: String,
    // Feature 9: search (sessions + messages)
    pub session_search_query: String,
    // Save serialization guards (AtomicBool ensures no concurrent file writes)
    session_save_in_flight: Arc<AtomicBool>,
    template_save_in_flight: Arc<AtomicBool>,
    // Monotonic save epochs for coalescing frequent save requests.
    session_save_epoch: Arc<AtomicU64>,
    template_save_epoch: Arc<AtomicU64>,
    // Feature 6: model selection
    selected_agent: String,
    selected_model: String,
    available_models: Vec<String>,
    /// Agent → [model_id, …] for the two-level picker
    available_agent_models: std::collections::HashMap<String, Vec<String>>,
    models_loaded: bool,
    /// Timestamp of last models fetch attempt (for retry throttle)
    last_models_fetch: std::time::Instant,
    last_selected_agent: String,
    stream_chunk_flush_interval: std::time::Duration,
    stream_repaint_interval: std::time::Duration,
    max_pending_events_per_frame: usize,
    stream_client: reqwest::Client,
    /// Per-message content hash cache: skips re-parsing unchanged messages.
    /// Key = message index, value = hash of content last rendered.
    rendered_content_hashes: Vec<u64>,
    /// Per-message "expand full text" toggle for truncated content.
    /// Key = message index, value = whether full text is shown.
    #[allow(dead_code)]
    expand_full_text: std::collections::HashSet<usize>,

    /// Shared abort controller for cancelling in-progress streaming generations.
    abort_controller: Option<AbortController>,
    /// Token-level progress tracking for the active generation.
    pub stream_progress: TokenProgress,
    /// SSE processor for the current streaming generation.
    stream_processor: Option<StreamProcessor>,

    /// Pending tool approval request from a sandbox denial.
    /// When set, the UI shows Approve/Deny buttons instead of a plain error.
    /// Tuple: (tool_name, last_user_message_index)
    pending_tool_approval: Option<(String, usize)>,
}

impl ChatView {
    fn markdown_to_plain_text(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut in_code_fence = false;

        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                in_code_fence = !in_code_fence;
                continue;
            }

            if in_code_fence {
                out.push_str(line);
                out.push('\n');
                continue;
            }

            let mut l = line.to_string();
            if l.starts_with('#') {
                l = l.trim_start_matches('#').trim_start().to_string();
            }
            if l.starts_with('>') {
                l = l.trim_start_matches('>').trim_start().to_string();
            }

            l = l.replace("**", "");
            l = l.replace("__", "");
            l = l.replace('`', "");
            l = l.replace("* ", "- ");

            out.push_str(&l);
            out.push('\n');
        }

        // Strip HTML tags for readability (e.g., <body>, <div>, etc.)
        let stripped = strip_html_tags(&out);
        stripped.trim().to_string()
    }

    /// Improved token counting algorithm
    /// Uses a more accurate estimation based on character and word count.
    /// Accounts for CJK (Chinese/Japanese/Korean) characters which are
    /// denser (~1.5 chars/token) than Latin script (~4 chars/token).
    fn estimate_tokens_improved(text: &str) -> usize {
        // Clean text by removing markdown and extra whitespace
        let clean_text = Self::markdown_to_plain_text(text);

        let char_count = clean_text.chars().count();
        let word_count = clean_text.split_whitespace().count();

        // Count CJK characters for adjusted token estimation.
        let cjk_count = clean_text
            .chars()
            .filter(|&c| {
                let code = c as u32;
                (0x4E00..=0x9FFF).contains(&code)      // CJK Unified Ideographs
                    || (0x3400..=0x4DBF).contains(&code) // CJK Extension A
                    || (0x2E80..=0x2FDF).contains(&code) // CJK Radicals
                    || (0x3040..=0x30FF).contains(&code) // Hiragana & Katakana
                    || (0xAC00..=0xD7AF).contains(&code) // Hangul Syllables
            })
            .count();

        // Blended divisor: CJK chars are ~1.5 tokens each, Latin ~4 chars each.
        let non_cjk = char_count.saturating_sub(cjk_count);
        let blended_divisor = if char_count > 0 {
            (cjk_count as f64 * 1.5 + non_cjk as f64 * 4.0) / char_count as f64
        } else {
            4.0
        };

        // Weighted average: 40% from chars, 60% from words
        let from_chars = (char_count as f64 / blended_divisor).ceil() as usize;
        let from_words = (word_count as f64 / 0.75).ceil() as usize;

        ((from_chars as f64 * 0.4 + from_words as f64 * 0.6).ceil() as usize).max(1)
    }

    /// Update model performance stats after message generation
    fn update_model_stats(&mut self, model: &str, tokens: usize, duration_ms: u64) {
        let stats = self.model_stats.entry(model.to_string()).or_default();
        stats.response_time_ms = duration_ms;
        stats.token_count = tokens;
        stats.success_count = stats.success_count.saturating_add(1);

        // Calculate tokens per minute
        if duration_ms > 0 {
            stats.avg_tokens_per_minute = (tokens as f64 / (duration_ms as f64 / 60000.0)).round();
        }
    }

    fn localized_default_session_name(index: usize, i18n: &I18n) -> String {
        if index == 0 {
            i18n.t("chat.newSession").to_string()
        } else {
            format!("{} {}", i18n.t("chat.newSession"), index + 1)
        }
    }

    fn is_default_session_name(name: &str, i18n: &I18n) -> bool {
        let localized = i18n.t("chat.newSession");
        name == i18n.t("chat.defaultSessionName")
                || name.starts_with(&*i18n.t("chat.defaultSessionPrefix"))
                || name == localized
                || name.starts_with(&format!("{} ", localized))
                // Fallback: English default names (for tests and backward compat)
                || name == "New Chat"
                // Match only "Chat <number>" (e.g. "Chat 3"), not user-named sessions like "Chat about project"
                || (name.starts_with("Chat ") && name[5..].parse::<u64>().is_ok())
                || name == "New session"
                || name.starts_with("New session ")
    }

    fn refresh_default_session_names(&mut self, i18n: &I18n) {
        for (idx, session) in self.sessions.iter_mut().enumerate() {
            if session.messages.is_empty() && Self::is_default_session_name(&session.name, i18n) {
                session.name = Self::localized_default_session_name(idx, i18n);
            }
        }
    }

    /// Handle paste and drop events from the egui input system.
    /// Detects pasted file paths (common on Linux with Ctrl+Shift+V), data:image/ URLs,
    /// and dropped files (drag-and-drop from file manager).
    /// Returns any attachments that were created from paste/drop events.
    fn handle_paste_events(&mut self, ui: &mut egui::Ui) -> Vec<Attachment> {
        // ── Handle file drop (drag-and-drop) via egui 0.31 raw input ──
        let dropped = ui.input(|i| i.raw.dropped_files.clone());
        for f in &dropped {
            if let Some(path) = &f.path {
                let mime = Self::guess_mime(path);
                if mime.starts_with("image/") || mime.starts_with("application/pdf") {
                    if let Ok(data) = std::fs::read(path) {
                        use base64::Engine;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                        self.attachments.push(Attachment {
                            name: path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("dropped")
                                .to_string(),
                            mime,
                            data: b64,
                        });
                    }
                }
            }
        }

        let pasted = ui.input_mut(|i| {
            let mut result = Vec::new();
            i.events.retain(|e| {
                // ── Handle paste events ──────────────────────────
                if let egui::Event::Paste(text) = e {
                    // Check if it looks like a file path (common on Linux)
                    let path = std::path::Path::new(text.as_str());
                    if path.exists() {
                        let mime = Self::guess_mime(path);
                        if mime.starts_with("image/") {
                            if let Ok(data) = std::fs::read(path) {
                                use base64::Engine;
                                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                                result.push(Attachment {
                                    name: path
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("pasted")
                                        .to_string(),
                                    mime,
                                    data: b64,
                                });
                                return false; // consume event
                            }
                        }
                    }
                    // If text looks like a base64 data URL for an image
                    if text.starts_with("data:image/") {
                        if let Some(comma_pos) = text.find(',') {
                            let mime_end = text.find(';').unwrap_or(comma_pos);
                            let mime = text[5..mime_end].to_string();
                            let b64 = text[comma_pos + 1..].to_string();
                            result.push(Attachment {
                                name: "pasted_image".to_string(),
                                mime,
                                data: b64,
                            });
                            return false;
                        }
                    }
                    // Regular text paste - don't consume, let it go to TextEdit
                }
                true
            });
            result
        });

        pasted
    }

    fn guess_mime(path: &Path) -> String {
        match path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("png") => "image/png".to_string(),
            Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
            Some("gif") => "image/gif".to_string(),
            Some("webp") => "image/webp".to_string(),
            Some("pdf") => "application/pdf".to_string(),
            Some("json") => "application/json".to_string(),
            Some("md") => "text/markdown".to_string(),
            Some("txt") => "text/plain".to_string(),
            _ => "application/octet-stream".to_string(),
        }
    }

    fn default_session(index: usize, phase: String, mode: String) -> Session {
        let now = crate::fs_util::epoch_secs();
        Session {
            id: format!("session_{}", index + 1),
            name: if index == 0 {
                // English defaults are fine here; refresh_default_session_names will localize later.
                "New Chat".to_string()
            } else {
                format!("Chat {}", index + 1)
            },
            messages: Vec::new(),
            created_at: now,
            workflow_type: "chat".to_string(),
            phase,
            mode,
            model: "auto".to_string(),
            phase_records: Vec::new(),
            conversation_id: None,
            branch_id: None,
        }
    }

    pub fn new() -> Self {
        let mut sessions = Self::load_sessions_from_disk();
        let templates = Self::load_templates_from_disk();
        if sessions.is_empty() {
            sessions.push(Self::default_session(0, String::new(), "edit".to_string()));
        }
        let initial_phase = sessions
            .first()
            .map(|s| s.phase.clone())
            .unwrap_or_default();
        let initial_model = sessions
            .first()
            .map(|s| s.model.clone())
            .unwrap_or_else(|| "auto".to_string());

        let (pending_tx, pending_rx) = mpsc::sync_channel(256);

        // Compute next_template_id from max existing template id
        let next_template_id = templates
            .iter()
            .filter_map(|t| {
                t.id.strip_prefix("tpl_")
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .max()
            .map(|id| id + 1)
            .unwrap_or(templates.len() as u64 + 1);

        // Load persistent UI state before constructing ChatView
        let ui_state = GlobalUiState::load();

        // Final mode: always use "edit" as default. Ignore saved ui_state or session
        // defaults so the initial mode stays as "edit" regardless of previous sessions.
        let saved_mode = "edit".to_string();

        // Deserialize model_stats from saved JSON if present
        let model_stats: std::collections::HashMap<String, ModelPerfStats> = ui_state
            .model_stats_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();

        // Restore input draft from last session, or use a default starter prompt
        let default_input = if !ui_state.input_draft.is_empty() {
            ui_state.input_draft.clone()
        } else {
            // Default starter prompt for first-time users
            String::new()
        };

        Self {
            sessions,
            active_session: 0,
            input: default_input,
            sending: false,
            last_sending: false,
            error: String::new(),
            success_message: None,
            ai_status: AiStatus::Idle,
            selected_phase: initial_phase,
            selected_mode: saved_mode,
            attachments: Vec::new(),
            phases: Vec::new(),
            phases_loaded: false,
            phases_load_scheduled: false,
            pending_rx,
            pending_tx,
            rename_session_idx: None,
            rename_session_buf: String::new(),
            show_thinking_idx: None,
            show_all_thinking: false,
            show_all_sub_agents: false,
            show_sub_agent_idx: None,
            show_all_commands: false,
            show_command_idx: None,
            edit_msg_idx: None,
            edit_msg_buf: String::new(),
            // Feature 4
            stop_requested: false,
            generation_states: Vec::new(),
            next_generation_id: 1,
            active_generations: Arc::new(AtomicU64::new(0)),
            // Feature 5
            last_token_estimate: 0,
            input_token_estimate: 0,
            output_token_estimate: 0,
            // Feature 7
            show_prompts: ui_state.show_prompts,
            show_model_picker: ui_state.show_model_picker,
            prompt_templates: templates,
            next_template_id,
            selected_template_idx: None,
            template_name_buf: String::new(),
            template_command_buf: String::new(),
            template_content_buf: String::new(),
            template_search_query: String::new(),
            templates_bootstrapped: false,
            prompts_command_templates: Vec::new(),
            prompt_collection: Vec::new(),
            prompt_selected_category: None,
            show_risk_decision: false,
            risk_is_high: false,
            risk_review_required: false,
            risk_strategy: String::new(),
            risk_reasons: String::new(),
            // Feature 9
            session_search_query: String::new(),
            // Save guards
            session_save_in_flight: Arc::new(AtomicBool::new(false)),
            template_save_in_flight: Arc::new(AtomicBool::new(false)),
            session_save_epoch: Arc::new(AtomicU64::new(0)),
            template_save_epoch: Arc::new(AtomicU64::new(0)),
            // Feature 6
            selected_agent: String::new(),
            selected_model: initial_model,
            available_models: vec!["auto".to_string()],
            available_agent_models: std::collections::HashMap::new(),
            models_loaded: false,
            last_models_fetch: std::time::Instant::now(),
            last_selected_agent: String::new(),
            stream_chunk_flush_interval: std::time::Duration::from_millis(33),
            stream_repaint_interval: std::time::Duration::from_millis(33),
            max_pending_events_per_frame: 256,
            // Enhanced features (Phase 2)
            enable_markdown: ui_state.enable_markdown,
            show_token_details: ui_state.show_token_details,
            model_stats,
            stream_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .read_timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_else(|_| {
                    reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(300))
                        .read_timeout(std::time::Duration::from_secs(60))
                        .build()
                        .unwrap_or_else(|_| reqwest::Client::new())
                }),
            rendered_content_hashes: Vec::new(),
            expand_full_text: std::collections::HashSet::new(),
            abort_controller: None,
            stream_progress: TokenProgress::default(),
            stream_processor: None,
            pending_tool_approval: None,
        }
    }

    fn apply_stability_settings(
        &mut self,
        repaint_ms: u64,
        flush_ms: u64,
        max_pending_events_per_frame: usize,
        token_flush_ms: u64,
    ) {
        self.stream_repaint_interval = std::time::Duration::from_millis(repaint_ms.clamp(16, 200));
        self.stream_chunk_flush_interval =
            std::time::Duration::from_millis(flush_ms.clamp(16, 200));
        self.max_pending_events_per_frame = max_pending_events_per_frame.clamp(16, 4096);
        // Token flush rate controls how frequently buffered tokens are
        // flushed to the UI for frame-rate-smooth rendering.
        // 16ms = ~60fps; clamped to [8, 200].
        self.stream_chunk_flush_interval = std::time::Duration::from_millis(
            self.stream_chunk_flush_interval
                .as_millis()
                .min(token_flush_ms.clamp(8, 200) as u128) as u64,
        );
    }

    fn session(&mut self) -> &mut Session {
        // Bounds check: if active_session is out of range, create a fallback session
        let idx = if self.active_session < self.sessions.len() {
            self.active_session
        } else {
            // Fallback to last session or create one
            if self.sessions.is_empty() {
                self.sessions.push(Self::default_session(
                    0,
                    "think".to_string(),
                    "edit".to_string(),
                ));
            }
            self.active_session = self.sessions.len() - 1;
            self.active_session
        };
        &mut self.sessions[idx]
    }

    pub fn messages(&self) -> &[Message] {
        if self.active_session < self.sessions.len() {
            &self.sessions[self.active_session].messages
        } else if !self.sessions.is_empty() {
            &self.sessions[self.sessions.len() - 1].messages
        } else {
            &[]
        }
    }

    fn bootstrap_default_templates(&mut self, i18n: &I18n) {
        if self.templates_bootstrapped {
            return;
        }
        self.templates_bootstrapped = true;
        if !self.prompt_templates.is_empty() {
            // Templates exist on disk — just update display names in case language changed.
            for tpl in &mut self.prompt_templates {
                match tpl.id.as_str() {
                    "explain" => {
                        tpl.name = i18n.t("chat.template.explain").to_string();
                        tpl.content = i18n.t("chat.template.explain.body").to_string();
                    }
                    "test" => {
                        tpl.name = i18n.t("chat.template.test").to_string();
                        tpl.content = i18n.t("chat.template.test.body").to_string();
                    }
                    "debug" => {
                        tpl.name = i18n.t("chat.template.debug").to_string();
                        tpl.content = i18n.t("chat.template.debug.body").to_string();
                    }
                    "refactor" => {
                        tpl.name = i18n.t("chat.template.refactor").to_string();
                        tpl.content = i18n.t("chat.template.refactor.body").to_string();
                    }
                    "summary" => {
                        tpl.name = i18n.t("chat.template.summary").to_string();
                        tpl.content = i18n.t("chat.template.summary.body").to_string();
                    }
                    "docs" => {
                        tpl.name = i18n.t("chat.template.docs").to_string();
                        tpl.content = i18n.t("chat.template.docs.body").to_string();
                    }
                    _ => {} // User-created templates keep their content
                }
            }
            return;
        }

        self.prompt_templates = vec![
            PromptTemplate {
                id: "explain".to_string(),
                name: i18n.t("chat.template.explain").to_string(),
                command: "/explain".to_string(),
                content: i18n.t("chat.template.explain.body").to_string(),
            },
            PromptTemplate {
                id: "test".to_string(),
                name: i18n.t("chat.template.test").to_string(),
                command: "/test".to_string(),
                content: i18n.t("chat.template.test.body").to_string(),
            },
            PromptTemplate {
                id: "debug".to_string(),
                name: i18n.t("chat.template.debug").to_string(),
                command: "/debug".to_string(),
                content: i18n.t("chat.template.debug.body").to_string(),
            },
            PromptTemplate {
                id: "refactor".to_string(),
                name: i18n.t("chat.template.refactor").to_string(),
                command: "/refactor".to_string(),
                content: i18n.t("chat.template.refactor.body").to_string(),
            },
            PromptTemplate {
                id: "summary".to_string(),
                name: i18n.t("chat.template.summary").to_string(),
                command: "/summary".to_string(),
                content: i18n.t("chat.template.summary.body").to_string(),
            },
            PromptTemplate {
                id: "docs".to_string(),
                name: i18n.t("chat.template.docs").to_string(),
                command: "/docs".to_string(),
                content: i18n.t("chat.template.docs.body").to_string(),
            },
        ];
        self.save_templates_to_disk();
    }

    /// The sentinel model name used when the copilot agent is selected.
    /// It signals that the VS Code / GitHub Copilot extension should auto-select
    /// the optimal model on the server side — Go-on does not hardcode or choose
    /// any specific model ID.
    const COPILOT_AUTO_MODEL: &'static str = "copilot/auto";

    fn sync_model_selection(&mut self) {
        // Ensure model name is trimmed
        self.selected_model = self.selected_model.trim().to_string();
        if self.selected_model.is_empty() {
            self.selected_model = "auto".to_string();
        }

        // ── Rule 1: If the selected agent is "copilot", force model to "copilot-auto" ──
        //   The GitHub Copilot service (not Go-on) decides which model to use.
        if self.selected_agent == "copilot" && self.selected_model != Self::COPILOT_AUTO_MODEL {
            self.selected_model = Self::COPILOT_AUTO_MODEL.to_string();
            if let Some(session) = self.sessions.get_mut(self.active_session) {
                session.model = self.selected_model.clone();
            }
            return;
        }

        // ── Rule 2: If the model is "copilot-auto", derive the agent ──
        if self.selected_model == Self::COPILOT_AUTO_MODEL {
            self.selected_agent = "copilot".to_string();
            if let Some(session) = self.sessions.get_mut(self.active_session) {
                session.model = self.selected_model.clone();
            }
            return;
        }

        // Sync to active session
        if let Some(session) = self.sessions.get_mut(self.active_session) {
            session.model = self.selected_model.clone();
        }

        // Derive selected_agent from available_agent_models
        if self.selected_model == "auto" {
            // Keep the current agent selection when model is auto — the user
            // may have explicitly picked an agent and expects it to stick.
            // Only clear if no agent is selected at all or the stored agent
            // is no longer known.
            if self.selected_agent.is_empty()
                || (!self
                    .available_agent_models
                    .contains_key(&self.selected_agent)
                    && self.selected_agent != "copilot")
            {
                self.selected_agent.clear();
            }
        } else {
            self.selected_agent.clear();
            for (agent, models) in &self.available_agent_models {
                if models.contains(&self.selected_model) {
                    self.selected_agent = agent.clone();
                    break;
                }
            }
        }

        // Validate current model is in available list or is "auto"
        let first = if self.selected_model == "auto"
            || self.selected_model == Self::COPILOT_AUTO_MODEL
            || self
                .available_models
                .iter()
                .any(|m| m == &self.selected_model)
        {
            self.selected_model.clone()
        } else {
            self.available_models
                .first()
                .cloned()
                .unwrap_or_else(|| "auto".to_string())
        };
        if first != self.selected_model {
            self.selected_model = first;
        }
    }

    fn next_generation_id(&mut self) -> u64 {
        let id = self.next_generation_id;
        self.next_generation_id += 1;
        id
    }

    fn generation_msg_idx(&self, generation_id: u64) -> Option<usize> {
        self.generation_states
            .iter()
            .find(|state| state.id == generation_id)
            .map(|state| state.msg_idx)
    }

    fn generation_meta(&self, generation_id: u64) -> Option<(usize, String, std::time::Instant)> {
        self.generation_states
            .iter()
            .find(|state| state.id == generation_id)
            .map(|state| (state.msg_idx, state.model.clone(), state.started_at))
    }

    fn remove_generation(&mut self, generation_id: u64) {
        // Abort the task before removing to prevent zombie tasks
        if let Some(idx) = self
            .generation_states
            .iter()
            .position(|state| state.id == generation_id)
        {
            self.generation_states[idx].handle.abort();
            self.generation_states.remove(idx);
        }
        // Note: active_generations counter is managed by ActiveGenerationGuard in runtime.rs
        // which automatically decrements when the tokio task exits. Do not double-decrement here.
        self.sending = !self.generation_states.is_empty();
        self.ai_status = if self.sending {
            AiStatus::Thinking
        } else if self.error.is_empty() {
            AiStatus::Idle
        } else {
            AiStatus::Error
        };
    }

    fn remove_message_at(&mut self, idx: usize) {
        if let Some(session) = self.sessions.get_mut(self.active_session) {
            if idx < session.messages.len() {
                session.messages.remove(idx);
                for state in &mut self.generation_states {
                    if state.msg_idx > idx {
                        state.msg_idx -= 1;
                    }
                }
                // Adjust show_thinking_idx if it points at or after the removed message
                if self.show_thinking_idx == Some(idx) {
                    self.show_thinking_idx = None;
                } else if self.show_thinking_idx.is_some_and(|i| i > idx) {
                    self.show_thinking_idx = self.show_thinking_idx.map(|i| i - 1);
                }
                // Adjust edit_msg_idx similarly
                if self.edit_msg_idx == Some(idx) {
                    self.edit_msg_idx = None;
                    self.edit_msg_buf.clear();
                } else if self.edit_msg_idx.is_some_and(|i| i > idx) {
                    self.edit_msg_idx = self.edit_msg_idx.map(|i| i - 1);
                }
            }
        }
    }

    fn normalize_command(command: &str) -> String {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        // Strip all leading slashes to handle `/cmd`, `//cmd`, `///cmd`, etc.
        let without_slashes = trimmed.trim_start_matches('/');
        if without_slashes.len() < trimmed.len() {
            // Had leading slashes — put back exactly one
            format!("/{without_slashes}")
        } else {
            // No leading slash — add one
            format!("/{trimmed}")
        }
    }

    #[cfg(test)]
    fn expand_prompt_command(&self, raw_input: &str) -> String {
        self.expand_prompt_command_with_fallback(raw_input, None)
    }

    /// Expand a `/` command using chat-local templates and optionally
    /// the PromptsView command templates as a fallback.
    fn expand_prompt_command_with_fallback(
        &self,
        raw_input: &str,
        prompts_commands: Option<&[crate::views::prompts::CommandTemplate]>,
    ) -> String {
        let trimmed = raw_input.trim();
        if !trimmed.starts_with('/') {
            return trimmed.to_string();
        }

        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or_default();
        let arguments = parts.next().unwrap_or_default().trim();

        // Check chat-local templates first
        if let Some(template) = self
            .prompt_templates
            .iter()
            .find(|template| template.command == command)
        {
            if template.content.contains("{{input}}") {
                return template.content.replace("{{input}}", arguments);
            }
            if arguments.is_empty() {
                return template.content.clone();
            }
            return format!("{}\n\n{}", template.content, arguments);
        }

        // Fallback to prompts view templates
        if let Some(cmds) = prompts_commands {
            if let Some(ct) = cmds.iter().find(|ct| ct.command == command) {
                if ct.content.contains("{{input}}") {
                    return ct.content.replace("{{input}}", arguments);
                }
                if arguments.is_empty() {
                    return ct.content.clone();
                }
                return format!("{}\n\n{}", ct.content, arguments);
            }
        }

        trimmed.to_string()
    }

    fn load_template_into_editor(&mut self, idx: usize) {
        if let Some(template) = self.prompt_templates.get(idx) {
            self.selected_template_idx = Some(idx);
            self.template_name_buf = template.name.clone();
            self.template_command_buf = template.command.clone();
            self.template_content_buf = template.content.clone();
        }
    }

    fn new_session(&mut self) {
        self.stop_sending();
        let count = self.sessions.len() + 1;
        self.sessions.push(Self::default_session(
            count - 1,
            self.selected_phase.clone(),
            self.selected_mode.clone(),
        ));
        if let Some(session) = self.sessions.last_mut() {
            session.model = self.selected_model.clone();
        }
        self.active_session = self.sessions.len() - 1;
        self.ai_status = AiStatus::Idle;
        self.attachments.clear();
        self.edit_msg_idx = None;
        self.edit_msg_buf.clear();
        self.show_thinking_idx = None;
        self.rename_session_idx = None;
        self.rename_session_buf.clear();
        self.last_selected_agent.clear();
        self.save_sessions_to_disk();
    }

    // Feature 4: stop sending
    /// Reset loaded state flags to force re-fetch of phases and models from backend.
    /// Used when backend URL changes.
    pub fn reset_loaded_state(&mut self) {
        self.phases_loaded = false;
        self.phases_load_scheduled = false;
        self.models_loaded = false;
        self.last_sending = false;
    }

    /// Save chat-specific fields into the global UI state for persistence.
    pub fn save_ui_state(&self, ui_state: &mut crate::views::ui_state::GlobalUiState) {
        ui_state.selected_mode = self.selected_mode.clone();
        ui_state.show_token_details = self.show_token_details;
        ui_state.enable_markdown = self.enable_markdown;
        ui_state.show_model_picker = self.show_model_picker;
        ui_state.show_prompts = self.show_prompts;
        if let Ok(stats_json) = serde_json::to_string(&self.model_stats) {
            ui_state.model_stats_json = Some(stats_json);
        }
        ui_state.active_session = self.active_session;
        ui_state.input_draft = self.input.clone();
        ui_state.session_search_query = self.session_search_query.clone();
        ui_state.template_search_query = self.template_search_query.clone();
    }

    pub fn risk_decision_draft(&self) -> RiskDecisionDraft {
        RiskDecisionDraft {
            is_high: self.risk_is_high,
            review_required: self.risk_review_required,
            strategy: self.risk_strategy.clone(),
            reasons: self.risk_reasons.clone(),
        }
    }

    pub fn apply_risk_decision_draft(&mut self, draft: &RiskDecisionDraft) {
        self.risk_is_high = draft.is_high;
        self.risk_review_required = draft.review_required;
        self.risk_strategy = draft.strategy.clone();
        self.risk_reasons = draft.reasons.clone();
    }

    fn set_phase_record_status(&mut self, status: &str) {
        if let Some(record) = self
            .session()
            .phase_records
            .iter_mut()
            .rev()
            .find(|r| r.status == "running")
        {
            record.status = status.to_string();
        }
    }

    pub fn stop_sending(&mut self) {
        self.stop_requested = true;
        // Signal abort to any in-progress stream via the abort controller
        if let Some(ref ctrl) = self.abort_controller {
            ctrl.abort();
        }
        // Abort all generation task handles
        for state in self.generation_states.drain(..) {
            state.handle.abort();
        }
        self.sending = false;
        self.ai_status = AiStatus::Idle;
        self.set_phase_record_status("stopped");
        self.stream_progress = TokenProgress::default();
        self.stream_processor = None;
    }
}

/// Remove HTML tags from a string, keeping the text content between them.
/// Used by markdown_to_plain_text for fallback plain text rendering.
fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ => {
                if !in_tag {
                    out.push(c);
                }
            }
        }
    }
    // Collapse multiple spaces into one
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_space = false;
    for c in out.chars() {
        if c == ' ' {
            if !prev_space {
                collapsed.push(' ');
                prev_space = true;
            }
        } else {
            collapsed.push(c);
            prev_space = false;
        }
    }
    collapsed
}

/// Format timestamp as absolute date+time in local timezone (e.g. "2025-05-07 14:30")
fn format_absolute_time(ts: u64) -> String {
    use chrono::{DateTime, Local, Utc};
    // Convert UTC seconds to localized time, with safe fallback
    match DateTime::<Utc>::from_timestamp(ts as i64, 0) {
        Some(utc) => {
            let local: DateTime<Local> = utc.with_timezone(&Local);
            local.format("%Y-%m-%d %H:%M").to_string()
        }
        None => ts.to_string(), // fallback: show raw timestamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    fn test_chat_view() -> ChatView {
        let (pending_tx, pending_rx) = mpsc::sync_channel(256);
        ChatView {
            sessions: vec![Session {
                id: "session_1".to_string(),
                name: "New Chat".to_string(),
                messages: Vec::new(),
                created_at: 0,
                workflow_type: "chat".to_string(),
                phase: String::new(),
                mode: "edit".to_string(),
                model: "auto".to_string(),
                phase_records: Vec::new(),
                conversation_id: None,
                branch_id: None,
            }],
            active_session: 0,
            input: String::new(),
            sending: false,
            last_sending: false,
            error: String::new(),
            success_message: None,
            ai_status: AiStatus::Idle,
            selected_phase: String::new(),
            selected_mode: "edit".to_string(),
            attachments: Vec::new(),
            phases: Vec::new(),
            phases_loaded: false,
            phases_load_scheduled: false,
            pending_rx,
            pending_tx,
            rename_session_idx: None,
            rename_session_buf: String::new(),
            show_thinking_idx: None,
            show_all_thinking: false,
            show_all_sub_agents: false,
            show_sub_agent_idx: None,
            show_all_commands: false,
            show_command_idx: None,
            edit_msg_idx: None,
            edit_msg_buf: String::new(),
            stop_requested: false,
            generation_states: Vec::new(),
            active_generations: Arc::new(AtomicU64::new(0)),
            next_generation_id: 1,
            last_token_estimate: 0,
            input_token_estimate: 0,
            output_token_estimate: 0,
            show_prompts: false,
            show_model_picker: false,
            prompt_templates: Vec::new(),
            next_template_id: 1,
            selected_template_idx: None,
            template_name_buf: String::new(),
            template_command_buf: String::new(),
            template_content_buf: String::new(),
            template_search_query: String::new(),
            templates_bootstrapped: false,
            prompts_command_templates: Vec::new(),
            prompt_collection: Vec::new(),
            prompt_selected_category: None,
            show_risk_decision: false,
            risk_is_high: false,
            risk_review_required: false,
            risk_strategy: String::new(),
            risk_reasons: String::new(),
            session_search_query: String::new(),
            session_save_in_flight: Arc::new(AtomicBool::new(false)),
            template_save_in_flight: Arc::new(AtomicBool::new(false)),
            session_save_epoch: Arc::new(AtomicU64::new(0)),
            template_save_epoch: Arc::new(AtomicU64::new(0)),
            selected_agent: String::new(),
            selected_model: "auto".to_string(),
            available_models: vec!["auto".to_string()],
            available_agent_models: std::collections::HashMap::new(),
            models_loaded: false,
            last_models_fetch: std::time::Instant::now(),
            last_selected_agent: String::new(),
            stream_chunk_flush_interval: std::time::Duration::from_millis(33),
            stream_repaint_interval: std::time::Duration::from_millis(33),
            max_pending_events_per_frame: 256,
            enable_markdown: true,
            show_token_details: true,
            model_stats: std::collections::HashMap::new(),
            stream_client: reqwest::Client::new(),
            rendered_content_hashes: Vec::new(),
            expand_full_text: std::collections::HashSet::new(),
            abort_controller: None,
            stream_progress: TokenProgress::default(),
            stream_processor: None,
            pending_tool_approval: None,
        }
    }

    #[test]
    fn expand_prompt_command_replaces_input_placeholder() {
        let mut view = test_chat_view();
        view.prompt_templates.push(PromptTemplate {
            id: "explain".to_string(),
            name: "Explain".to_string(),
            command: "/explain".to_string(),
            content: "Explain:\n{{input}}".to_string(),
        });

        assert_eq!(
            view.expand_prompt_command("/explain hello"),
            "Explain:\nhello"
        );
    }

    #[test]
    fn expand_prompt_command_appends_arguments_when_template_has_no_placeholder() {
        let mut view = test_chat_view();
        view.prompt_templates.push(PromptTemplate {
            id: "sum".to_string(),
            name: "Summary".to_string(),
            command: "/summary".to_string(),
            content: "Summarize the following".to_string(),
        });

        assert_eq!(
            view.expand_prompt_command("/summary release notes"),
            "Summarize the following\n\nrelease notes"
        );
    }

    #[test]
    fn refresh_default_session_names_localizes_empty_sessions() {
        let mut view = test_chat_view();
        view.sessions.push(Session {
            id: "session_2".to_string(),
            name: "Chat 2".to_string(),
            messages: Vec::new(),
            created_at: 0,
            workflow_type: "chat".to_string(),
            phase: String::new(),
            mode: "edit".to_string(),
            model: "auto".to_string(),
            phase_records: Vec::new(),
            conversation_id: None,
            branch_id: None,
        });
        let i18n = I18n::new(Lang::ZhCn);

        view.refresh_default_session_names(&i18n);

        assert_eq!(view.sessions[0].name, i18n.t("chat.newSession"));
        assert_eq!(
            view.sessions[1].name,
            format!("{} 2", i18n.t("chat.newSession"))
        );
    }

    #[test]
    fn refresh_default_session_names_preserves_named_sessions() {
        let mut view = test_chat_view();
        view.sessions[0].name = "Release review".to_string();
        let i18n = I18n::new(Lang::ZhCn);

        view.refresh_default_session_names(&i18n);

        assert_eq!(view.sessions[0].name, "Release review");
    }
}
