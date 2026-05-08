use crate::backend::BackendClient;
use crate::i18n::I18n;
use crate::views::autotune::AutoTuneView;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

const CHAT_PERF_WINDOW: usize = 120;
const CHAT_PERF_SUMMARY_INTERVAL: u64 = 60;
const CHAT_DISABLE_MARKDOWN_RENDER: bool = true;
const CHAT_SAFE_MODE: bool = true;
// Isolation stages for step-by-step freeze diagnosis:
// 1=minimal list only, 2=+input widget, 3=+Enter send, 4=+show_messages,
// 5=+sidebar, 6=full original layout (safe mode bypassed).
const CHAT_ISOLATION_STAGE: u8 = 6;
// Stage-6 probe: metadata sync can trigger frequent disk writes if values flap each frame.
const CHAT_STAGE6_ENABLE_METADATA_SYNC: bool = true;
const CHAT_STAGE6_ENABLE_SEARCH_ROW: bool = true;
const CHAT_STAGE6_ENABLE_MODE_ROW: bool = true;
const CHAT_STAGE6_ENABLE_EXTRA_BUTTONS: bool = true;
const CHAT_STAGE6_ENABLE_MODEL_PICKER_WINDOW: bool = true;
const CHAT_STAGE6_FORCE_SAFE_LAYOUT_SKELETON: bool = true;

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
    #[serde(skip)]
    pub show_thinking_msg: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub data: String, // base64 or path
}

/// A record of an AI phase execution in the workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseRecord {
    pub phase: String,
    pub agent: String,
    pub status: String, // "running", "completed", "error"
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
    #[serde(default = "default_models")]
    pub models: Vec<String>,
    #[serde(default)]
    pub phase_records: Vec<PhaseRecord>,
}

fn default_model() -> String {
    "auto".to_string()
}

fn default_models() -> Vec<String> {
    vec!["auto".to_string()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromptTemplate {
    id: String,
    name: String,
    command: String,
    content: String,
}

struct GenerationState {
    id: u64,
    msg_idx: usize,
    handle: JoinHandle<()>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AiStatus {
    Idle,
    Thinking,
    Error,
}

enum PendingResponse {
    ChatCompleted {
        generation_id: u64,
        content: String,
        thinking: String,
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
    Models(Vec<String>),
}

pub struct ChatView {
    pub sessions: Vec<Session>,
    pub active_session: usize,
    pub input: String,
    pub sending: bool,
    pub error: String,
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
    pending_tx: mpsc::Sender<PendingResponse>,
    // Feature 3: edit/retry/delete
    edit_msg_idx: Option<usize>,
    edit_msg_buf: String,
    // Feature 4: stop button
    stop_requested: bool,
    generation_states: Vec<GenerationState>,
    next_generation_id: u64,
    // Feature 5: token display
    last_token_estimate: usize,
    input_token_estimate: usize,
    output_token_estimate: usize,
    // Feature 7: quick prompts
    show_prompts: bool,
    show_model_picker: bool,
    prompt_templates: Vec<PromptTemplate>,
    selected_template_idx: Option<usize>,
    template_name_buf: String,
    template_command_buf: String,
    template_content_buf: String,
    template_search_query: String,
    templates_bootstrapped: bool,
    // Feature 9: search (sessions + messages)
    session_search_query: String,
    message_search_query: String,
    // Feature 6: multi-model
    selected_model: String,
    selected_models: Vec<String>,
    available_models: Vec<String>,
    models_loaded: bool,
    input_ready: bool,
    perf_total_samples: VecDeque<u128>,
    perf_sidebar_samples: VecDeque<u128>,
    perf_messages_samples: VecDeque<u128>,
    perf_composer_samples: VecDeque<u128>,
    perf_frame_counter: u64,
    debug_log_bootstrapped: bool,
    // Message pagination
    messages_page: usize,
    const_messages_per_page: usize,
}

impl ChatView {
    fn chat_debug_enabled() -> bool {
        std::env::var("GOON_GUI_CHAT_DEBUG")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "on" | "ON"))
            .unwrap_or(false)
    }

    fn chat_debug_log(msg: &str) {
        eprintln!("{}", msg);
        let path = std::env::temp_dir().join("go-on-chat-debug.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{}", msg);
        }
    }

    fn push_perf_sample(samples: &mut VecDeque<u128>, value: u128) {
        if samples.len() >= CHAT_PERF_WINDOW {
            samples.pop_front();
        }
        samples.push_back(value);
    }

    fn perf_avg(samples: &VecDeque<u128>) -> u128 {
        if samples.is_empty() {
            return 0;
        }
        samples.iter().sum::<u128>() / samples.len() as u128
    }

    fn perf_p95(samples: &VecDeque<u128>) -> u128 {
        if samples.is_empty() {
            return 0;
        }
        let mut sorted: Vec<u128> = samples.iter().copied().collect();
        sorted.sort_unstable();
        let idx = ((sorted.len() - 1) * 95) / 100;
        sorted[idx]
    }

    fn perf_max(samples: &VecDeque<u128>) -> u128 {
        samples.iter().copied().max().unwrap_or(0)
    }

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

        out.trim().to_string()
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
        name == "New Chat"
            || name.starts_with("Chat ")
            || name == localized
            || name.starts_with(&format!("{} ", localized))
    }

    fn refresh_default_session_names(&mut self, i18n: &I18n) {
        for (idx, session) in self.sessions.iter_mut().enumerate() {
            if session.messages.is_empty() && Self::is_default_session_name(&session.name, i18n) {
                session.name = Self::localized_default_session_name(idx, i18n);
            }
        }
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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Session {
            id: format!("session_{}", index + 1),
            name: if index == 0 {
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
            models: vec!["auto".to_string()],
            phase_records: Vec::new(),
        }
    }

    fn templates_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
            dirs.config_dir().join("chat_prompt_templates.json")
        } else {
            PathBuf::from("chat_prompt_templates.json")
        }
    }

    fn load_templates_from_disk() -> Vec<PromptTemplate> {
        let path = Self::templates_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                serde_json::from_str::<Vec<PromptTemplate>>(&content).unwrap_or_default()
            }
            Err(_) => Vec::new(),
        }
    }

    fn save_templates_to_disk(&self) {
        let templates = self.prompt_templates.clone();
        let path = Self::templates_path();
        tokio::spawn(async move {
            if let Some(parent) = path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    eprintln!(
                        "Failed to create chat template directory {}: {e}",
                        parent.display()
                    );
                    return;
                }
            }
            match serde_json::to_string_pretty(&templates) {
                Ok(content) => {
                    if let Err(e) = tokio::fs::write(&path, content).await {
                        eprintln!("Failed to write chat templates to {}: {e}", path.display());
                    }
                }
                Err(e) => {
                    eprintln!("Failed to serialize chat templates: {e}");
                }
            }
        });
    }

    fn sessions_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
            dirs.config_dir().join("chat_sessions.json")
        } else {
            PathBuf::from("chat_sessions.json")
        }
    }

    fn load_sessions_from_disk() -> Vec<Session> {
        let path = Self::sessions_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str::<Vec<Session>>(&content).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    fn save_sessions_to_disk(&self) {
        // Clone data for the background task to avoid blocking the UI thread.
        let sessions = self.sessions.clone();
        let path = Self::sessions_path();
        tokio::spawn(async move {
            if let Some(parent) = path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    eprintln!(
                        "Failed to create chat session directory {}: {e}",
                        parent.display()
                    );
                    return;
                }
            }
            // Serialize on the async task (off UI thread).
            match serde_json::to_string_pretty(&sessions) {
                Ok(content) => {
                    if let Err(e) = tokio::fs::write(&path, content).await {
                        eprintln!("Failed to write chat sessions to {}: {e}", path.display());
                    }
                }
                Err(e) => {
                    eprintln!("Failed to serialize chat sessions: {e}");
                }
            }
        });
    }

    pub fn new() -> Self {
        let mut sessions = Self::load_sessions_from_disk();
        let templates = Self::load_templates_from_disk();
        if sessions.is_empty() {
            sessions.push(Self::default_session(0, String::new(), "ask".to_string()));
        }
        let initial_phase = sessions
            .first()
            .map(|s| s.phase.clone())
            .unwrap_or_default();
        let initial_mode = sessions
            .first()
            .map(|s| s.mode.clone())
            .unwrap_or_else(|| "ask".to_string());
        let initial_model = sessions
            .first()
            .map(|s| s.model.clone())
            .unwrap_or_else(|| "auto".to_string());
        let initial_models = sessions
            .first()
            .map(|s| {
                if s.models.is_empty() {
                    vec![s.model.clone()]
                } else {
                    s.models.clone()
                }
            })
            .unwrap_or_else(|| vec!["auto".to_string()]);

        let (pending_tx, pending_rx) = mpsc::channel();

        Self {
            sessions,
            active_session: 0,
            input: String::new(),
            sending: false,
            error: String::new(),
            ai_status: AiStatus::Idle,
            selected_phase: initial_phase,
            selected_mode: initial_mode,
            attachments: Vec::new(),
            phases: Vec::new(),
            phases_loaded: false,
            phases_load_scheduled: false,
            pending_rx,
            pending_tx,
            // Feature 3
            edit_msg_idx: None,
            edit_msg_buf: String::new(),
            // Feature 4
            stop_requested: false,
            generation_states: Vec::new(),
            next_generation_id: 1,
            // Feature 5
            last_token_estimate: 0,
            input_token_estimate: 0,
            output_token_estimate: 0,
            // Feature 7
            show_prompts: false,
            show_model_picker: false,
            prompt_templates: templates,
            selected_template_idx: None,
            template_name_buf: String::new(),
            template_command_buf: String::new(),
            template_content_buf: String::new(),
            template_search_query: String::new(),
            templates_bootstrapped: false,
            // Feature 9
            session_search_query: String::new(),
            message_search_query: String::new(),
            // Feature 6
            selected_model: initial_model,
            selected_models: initial_models,
            available_models: vec!["auto".to_string()],
            models_loaded: false,
            input_ready: false,
            perf_total_samples: VecDeque::with_capacity(CHAT_PERF_WINDOW),
            perf_sidebar_samples: VecDeque::with_capacity(CHAT_PERF_WINDOW),
            perf_messages_samples: VecDeque::with_capacity(CHAT_PERF_WINDOW),
            perf_composer_samples: VecDeque::with_capacity(CHAT_PERF_WINDOW),
            perf_frame_counter: 0,
            debug_log_bootstrapped: false,
            // Pagination
            messages_page: 0,
            const_messages_per_page: 3,
        }
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
                    "ask".to_string(),
                ));
            }
            self.active_session = self.sessions.len() - 1;
            self.active_session
        };
        &mut self.sessions[idx]
    }

    fn messages(&self) -> &[Message] {
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

    fn normalize_models(models: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for model in models.iter().map(|m| m.trim()).filter(|m| !m.is_empty()) {
            if seen.insert(model.to_string()) {
                result.push(model.to_string());
            }
        }
        if result.len() > 1 {
            result.retain(|m| m != "auto");
        }
        if result.is_empty() {
            result.push("auto".to_string());
        }
        result
    }

    fn sync_model_selection(&mut self) {
        // Avoid allocating a new Vec every frame when nothing needs normalizing.
        let normalized = Self::normalize_models(&self.selected_models);
        if normalized != self.selected_models {
            self.selected_models = normalized;
        }
        let first = self
            .selected_models
            .first()
            .cloned()
            .unwrap_or_else(|| "auto".to_string());
        if first != self.selected_model {
            self.selected_model = first;
        }
    }

    fn selected_models_summary(&self, i18n: &I18n) -> String {
        match self.selected_models.len() {
            0 => "auto".to_string(),
            1 => self.selected_models[0].clone(),
            count => format!(
                "{} ({count}): {}",
                i18n.t("chat.multiModelEnabled"),
                self.selected_models.join(", ")
            ),
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

    fn remove_generation(&mut self, generation_id: u64) {
        self.generation_states
            .retain(|state| state.id != generation_id);
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
            }
        }
    }

    fn normalize_command(command: &str) -> String {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            String::new()
        } else if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            format!("/{trimmed}")
        }
    }

    fn expand_prompt_command(&self, raw_input: &str) -> String {
        let trimmed = raw_input.trim();
        if !trimmed.starts_with('/') {
            return trimmed.to_string();
        }

        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or_default();
        let arguments = parts.next().unwrap_or_default().trim();
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
        self.selected_models = vec![self.selected_model.clone()];
        self.save_sessions_to_disk();
    }

    // Feature 4: stop sending
    pub fn stop_sending(&mut self) {
        self.stop_requested = true;
        for state in self.generation_states.drain(..) {
            state.handle.abort();
        }
        self.sending = false;
        self.ai_status = AiStatus::Idle;
        if let Some(record) = self
            .session()
            .phase_records
            .iter_mut()
            .rev()
            .find(|r| r.status == "running")
        {
            record.status = "stopped".to_string();
        }
    }

    /// Send a message asynchronously via the backend.
    pub fn send_message(
        &mut self,
        backend: &BackendClient,
        ctx: &egui::Context,
        autotune_chain_enabled: bool,
    ) {
        let msg = self.input.trim().to_string();
        if msg.is_empty() || self.sending {
            return;
        }
        let expanded_msg = self.expand_prompt_command(&msg);
        let mode = self.selected_mode.clone();
        let phase = self.selected_phase.clone();
        let base_url = backend.base_url().to_string();
        let selected_models = Self::normalize_models(&self.selected_models);
        let autotune_extra = if autotune_chain_enabled {
            Some(AutoTuneView::load_runtime_options())
        } else {
            None
        };

        self.input.clear();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let atts = std::mem::take(&mut self.attachments);
        let attachment_summary = if atts.is_empty() {
            String::new()
        } else {
            let details = atts
                .iter()
                .map(|a| format!("- {} ({}) {}", a.name, a.mime, a.data))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\n[Attachments]\n{details}")
        };
        let outbound_msg = format!("{expanded_msg}{attachment_summary}");

        // Add user message immediately
        self.session().messages.push(Message {
            role: "user".to_string(),
            content: expanded_msg.clone(),
            timestamp: now,
            attachments: atts,
            model: String::new(),
            comparison_id: 0,
            input_tokens: expanded_msg.chars().count() / 4,
            output_tokens: 0,
            total_tokens: 0,
            thinking: String::new(),
            show_thinking_msg: false,
        });
        self.save_sessions_to_disk();

        self.last_token_estimate = 0;
        self.input_token_estimate = expanded_msg.chars().count() / 4;
        self.output_token_estimate = 0;

        // Add a "running" phase record
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let running_phase = if phase.is_empty() { "think" } else { &phase };
        self.session().phase_records.push(PhaseRecord {
            phase: running_phase.to_string(),
            agent: String::new(),
            status: "running".to_string(),
            timestamp: now_ts,
        });

        self.ai_status = AiStatus::Thinking;
        self.sending = true;
        self.error.clear();
        self.stop_requested = false;

        self.selected_models = selected_models.clone();
        self.sync_model_selection();

        let comparison_id = now;
        for model_name in selected_models {
            let generation_id = self.next_generation_id();
            let input_tokens = self.input_token_estimate;
            self.session().messages.push(Message {
                role: "assistant".to_string(),
                content: String::new(),
                timestamp: now,
                attachments: Vec::new(),
                model: model_name.clone(),
                comparison_id,
                input_tokens,
                output_tokens: 0,
                total_tokens: 0,
                thinking: String::new(),
                show_thinking_msg: false,
            });
            let msg_idx = self.session().messages.len().saturating_sub(1);

            let tx = self.pending_tx.clone();
            let backend_clone = backend.clone();
            let ctx_clone = ctx.clone();
            let mode_clone = mode.clone();
            let phase_clone = phase.clone();
            let msg_clone = expanded_msg.clone();
            let outbound_clone = outbound_msg.clone();
            let model_clone = model_name.clone();
            let base_url_clone = base_url.clone();
            let autotune_extra_clone = autotune_extra.clone();
            let handle = tokio::spawn(async move {
                let phase_val = if phase_clone.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(phase_clone.clone())
                };

                let mut body = serde_json::json!({
                    "messages": [{"role": "user", "content": outbound_clone}],
                    "mode": mode_clone,
                    "phase": phase_val,
                });

                if !model_clone.trim().is_empty() && model_clone != "auto" {
                    body["options"] = serde_json::json!({
                        "model": model_clone,
                    });
                }

                if let Some(extra) = autotune_extra_clone.clone() {
                    if body.get("options").is_none() {
                        body["options"] = serde_json::json!({});
                    }
                    body["options"]["extra"] = extra;
                }

                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(180))
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new());

                let stream_resp = client
                    .post(format!(
                        "{}/chat/stream",
                        base_url_clone.trim_end_matches('/')
                    ))
                    .json(&body)
                    .send()
                    .await;

                match stream_resp {
                    Ok(resp) => {
                        let mut resp = if let Err(err) = resp.error_for_status_ref() {
                            let fallback = backend_clone
                                .chat_with_options(
                                    &msg_clone,
                                    &mode_clone,
                                    &phase_clone,
                                    Some(&model_clone),
                                    autotune_extra_clone.clone(),
                                )
                                .await
                                .map(|(content, thinking)| PendingResponse::ChatCompleted {
                                    generation_id,
                                    content,
                                    thinking,
                                })
                                .unwrap_or_else(|e| PendingResponse::Error {
                                    generation_id: Some(generation_id),
                                    message: format!(
                                        "stream status error: {err}; fallback failed: {e}"
                                    ),
                                });
                            let _ = tx.send(fallback);
                            ctx_clone.request_repaint();
                            return;
                        } else {
                            resp
                        };

                        let mut sse_buffer = String::new();
                        let mut final_content: Option<String> = None;
                        let mut final_thinking: Option<String> = None;

                        loop {
                            let chunk = match resp.chunk().await {
                                Ok(Some(c)) => c,
                                Ok(None) => break,
                                Err(e) => {
                                    let _ = tx.send(PendingResponse::Error {
                                        generation_id: Some(generation_id),
                                        message: format!("stream read error: {e}"),
                                    });
                                    ctx_clone.request_repaint();
                                    return;
                                }
                            };
                            let part = String::from_utf8_lossy(&chunk);
                            sse_buffer.push_str(&part);

                            while let Some(split_at) = sse_buffer.find("\n\n") {
                                let frame = sse_buffer[..split_at].to_string();
                                sse_buffer = sse_buffer[split_at + 2..].to_string();

                                let mut event_name = String::new();
                                let mut data_payload = String::new();
                                for line in frame.lines() {
                                    if let Some(rest) = line.strip_prefix("event:") {
                                        event_name = rest.trim().to_string();
                                    } else if let Some(rest) = line.strip_prefix("data:") {
                                        if !data_payload.is_empty() {
                                            data_payload.push('\n');
                                        }
                                        data_payload.push_str(rest.trim());
                                    }
                                }

                                if data_payload.is_empty() {
                                    continue;
                                }

                                let data: Value = match serde_json::from_str(&data_payload) {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };

                                match event_name.as_str() {
                                    "chunk" => {
                                        let token = data
                                            .get("token")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        let reasoning = data
                                            .get("reasoning")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        if !token.is_empty() || !reasoning.is_empty() {
                                            let _ = tx.send(PendingResponse::StreamChunk {
                                                generation_id,
                                                token,
                                                reasoning,
                                            });
                                        }
                                    }
                                    "telemetry" => {
                                        if let Some(te) = data.get("token_economy") {
                                            let input_tokens = te
                                                .get("input_tokens")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0)
                                                as usize;
                                            let output_tokens = te
                                                .get("output_tokens")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0)
                                                as usize;
                                            let total_tokens = te
                                                .get("total_tokens")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0)
                                                as usize;
                                            let _ = tx.send(PendingResponse::TokenEconomy {
                                                generation_id,
                                                input_tokens,
                                                output_tokens,
                                                total_tokens,
                                            });
                                        }
                                    }
                                    "result" => {
                                        final_content = data
                                            .get("response")
                                            .and_then(|v| v.as_str())
                                            .map(ToOwned::to_owned);
                                        final_thinking = data
                                            .get("thinking")
                                            .and_then(|v| v.as_str())
                                            .map(ToOwned::to_owned);
                                    }
                                    "error" => {
                                        let message = data
                                            .get("message")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown stream error")
                                            .to_string();
                                        let _ = tx.send(PendingResponse::Error {
                                            generation_id: Some(generation_id),
                                            message,
                                        });
                                        ctx_clone.request_repaint();
                                        return;
                                    }
                                    _ => {}
                                }
                            }
                            ctx_clone.request_repaint();
                        }

                        let _ = tx.send(PendingResponse::ChatCompleted {
                            generation_id,
                            content: final_content.unwrap_or_default(),
                            thinking: final_thinking.unwrap_or_default(),
                        });
                    }
                    Err(err) => {
                        let fallback = backend_clone
                            .chat(&msg_clone, &mode_clone, &phase_clone, Some(&model_clone))
                            .await
                            .map(|(content, thinking)| PendingResponse::ChatCompleted {
                                generation_id,
                                content,
                                thinking,
                            })
                            .unwrap_or_else(|e| PendingResponse::Error {
                                generation_id: Some(generation_id),
                                message: format!(
                                    "stream request error: {err}; fallback failed: {e}"
                                ),
                            });
                        let _ = tx.send(fallback);
                    }
                }
                ctx_clone.request_repaint();
            });
            self.generation_states.push(GenerationState {
                id: generation_id,
                msg_idx,
                handle,
            });
        }
        self.save_sessions_to_disk();
    }

    /// Drain any pending async responses and update the session / `ai_status`.
    fn process_pending(&mut self, i18n: &I18n) {
        while let Ok(pending) = self.pending_rx.try_recv() {
            match pending {
                PendingResponse::Phases(list) => {
                    self.phases = list;
                    self.phases_loaded = true;
                }
                PendingResponse::Models(list) => {
                    self.available_models = if list.is_empty() {
                        vec!["auto".to_string()]
                    } else {
                        list
                    };
                    if !self
                        .available_models
                        .iter()
                        .any(|m| m == &self.selected_model)
                    {
                        self.selected_model = "auto".to_string();
                    }
                }
                PendingResponse::StreamChunk {
                    generation_id,
                    token,
                    reasoning,
                } => {
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
                            if let Some(m) = session.messages.get_mut(idx) {
                                if !token.is_empty() {
                                    m.content.push_str(&token);
                                }
                                if !reasoning.is_empty() {
                                    m.thinking.push_str(&reasoning);
                                }
                            }
                        }
                    }
                }
                PendingResponse::TokenEconomy {
                    generation_id,
                    input_tokens,
                    output_tokens,
                    total_tokens,
                } => {
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
                            if let Some(m) = session.messages.get_mut(idx) {
                                m.input_tokens = input_tokens;
                                m.output_tokens = output_tokens;
                                m.total_tokens = total_tokens;
                            }
                        }
                    }
                    self.input_token_estimate = input_tokens;
                    self.output_token_estimate = output_tokens;
                    self.last_token_estimate = total_tokens;
                }
                PendingResponse::ChatCompleted {
                    generation_id,
                    content,
                    thinking,
                } => {
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
                            if let Some(m) = session.messages.get_mut(idx) {
                                if !content.is_empty() {
                                    m.content = content;
                                }
                                if !thinking.is_empty() {
                                    m.thinking = thinking;
                                }
                                if self.last_token_estimate == 0 {
                                    self.output_token_estimate = m.content.chars().count() / 4;
                                    self.last_token_estimate =
                                        self.input_token_estimate + self.output_token_estimate;
                                }
                            }
                        }
                    }

                    if let Some(record) = self
                        .session()
                        .phase_records
                        .iter_mut()
                        .rev()
                        .find(|r| r.status == "running")
                    {
                        record.status = "completed".to_string();
                    }

                    // Auto-name the session from first user message if still default
                    let first_user_content = self
                        .session()
                        .messages
                        .iter()
                        .find(|m| m.role == "user")
                        .map(|m| m.content.clone());
                    if let Some(content) = first_user_content {
                        let is_default = Self::is_default_session_name(&self.session().name, i18n);
                        if is_default {
                            let truncated: String = content.chars().take(25).collect();
                            self.session().name = truncated;
                        }
                    }

                    self.remove_generation(generation_id);
                    self.stop_requested = false;
                    self.save_sessions_to_disk();
                }
                PendingResponse::Error {
                    generation_id,
                    message,
                } => {
                    self.error = i18n.t("chat.chatError").replace("{message}", &message);
                    if let Some(record) = self
                        .session()
                        .phase_records
                        .iter_mut()
                        .rev()
                        .find(|r| r.status == "running")
                    {
                        record.status = "error".to_string();
                    }

                    // Drop empty placeholder assistant message on failure.
                    if let Some(idx) = generation_id.and_then(|id| self.generation_msg_idx(id)) {
                        let should_remove = self
                            .sessions
                            .get(self.active_session)
                            .map(|session| {
                                idx < session.messages.len()
                                    && session.messages[idx].content.is_empty()
                            })
                            .unwrap_or(false);
                        if should_remove {
                            self.remove_message_at(idx);
                        }
                    }

                    if let Some(id) = generation_id {
                        self.remove_generation(id);
                    }
                    self.sending = !self.generation_states.is_empty();
                    self.ai_status = AiStatus::Error;
                    self.stop_requested = false;
                }
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
        autotune_chain_enabled: bool,
    ) {
        // Debug: timer to detect hangs
        let _start = Instant::now();
        let debug_enabled = Self::chat_debug_enabled();
        if debug_enabled && !self.debug_log_bootstrapped {
            self.debug_log_bootstrapped = true;
            let path = std::env::temp_dir().join("go-on-chat-debug.log");
            Self::chat_debug_log(&format!(
                "[CHAT_DEBUG_BOOT] logging enabled; file={}",
                path.display()
            ));
        }

        // Log entry
        Self::chat_debug_log("[CHAT_SHOW] Entry");

        // Process any pending async responses (non-blocking)
        self.process_pending(i18n);
        Self::chat_debug_log(&format!(
            "[CHAT_SHOW] process_pending done: {}ms",
            _start.elapsed().as_millis()
        ));

        // Bail out early if processing_pending took too long
        if _start.elapsed().as_millis() > 100 {
            eprintln!(
                "[CHAT_DEBUG] process_pending took {}ms",
                _start.elapsed().as_millis()
            );
        }

        // Lazy initialization of templates and name refresh
        // Capture before bootstrap so we can detect the very first run.
        let is_first_init = !self.templates_bootstrapped;
        if is_first_init {
            self.bootstrap_default_templates(i18n);
            // Refresh localized session names once at startup.
            self.refresh_default_session_names(i18n);
        }
        self.sync_model_selection();

        // Delayed loading: Schedule backend queries after first render to avoid UI freeze
        // Only schedule once to prevent repeated triggers
        if !self.phases_load_scheduled && !self.phases_loaded {
            self.phases_load_scheduled = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();

            tokio::spawn(async move {
                // Wait 100ms to let UI render first
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                // Add timeout to prevent hanging
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.config_baseline(),
                )
                .await
                {
                    Ok(Ok(baseline)) => {
                        let phases = baseline
                            .get("config")
                            .and_then(|c| c.get("flow"))
                            .and_then(|f| f.get("phases"))
                            .and_then(|p| p.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect::<Vec<_>>()
                            });
                        if let Some(list) = phases {
                            let _ = tx.send(PendingResponse::Phases(list));
                            ctx_clone.request_repaint();
                        }
                    }
                    _ => {
                        eprintln!("Warning: Failed to load phases from backend (timeout or error)");
                    }
                }
            });
        }

        if !self.models_loaded {
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();

            tokio::spawn(async move {
                // Wait 150ms (slightly after phases) to stagger requests
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;

                // Add timeout to prevent hanging
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.fetch_models(),
                )
                .await
                {
                    Ok(models) => {
                        let mut options = vec!["auto".to_string()];
                        let mut ids: Vec<String> = models
                            .into_values()
                            .flat_map(|ids| ids.into_iter())
                            .collect();
                        ids.sort();
                        ids.dedup();
                        options.extend(ids);
                        let _ = tx.send(PendingResponse::Models(options));
                        ctx_clone.request_repaint();
                    }
                    Err(_) => {
                        eprintln!("Warning: Failed to load models from backend (timeout)");
                    }
                }
            });

            self.models_loaded = true;
        }

        let use_safe_mode = (CHAT_SAFE_MODE && CHAT_ISOLATION_STAGE < 6)
            || (CHAT_ISOLATION_STAGE == 6 && CHAT_STAGE6_FORCE_SAFE_LAYOUT_SKELETON);
        if use_safe_mode {
            let (sidebar_ms, messages_ms, composer_ms) =
                self.show_safe_chat_layout(ui, i18n, backend, ctx, autotune_chain_enabled);

            if debug_enabled {
                let total_ms = _start.elapsed().as_millis();
                Self::push_perf_sample(&mut self.perf_total_samples, total_ms);
                Self::push_perf_sample(&mut self.perf_sidebar_samples, sidebar_ms);
                Self::push_perf_sample(&mut self.perf_messages_samples, messages_ms);
                Self::push_perf_sample(&mut self.perf_composer_samples, composer_ms);
                self.perf_frame_counter = self.perf_frame_counter.saturating_add(1);

                if total_ms > 40 || messages_ms > 30 || composer_ms > 25 || sidebar_ms > 20 {
                    Self::chat_debug_log(&format!(
                        "[CHAT_PERF_SAFE] total={}ms sidebar={}ms messages={}ms composer={}ms sessions={} msgs={}",
                        total_ms,
                        sidebar_ms,
                        messages_ms,
                        composer_ms,
                        self.sessions.len(),
                        self.messages().len()
                    ));
                }
            }
            return;
        }

        // ── Layout: left sidebar (200px) + right content ──────────────
        let mut sidebar_ms: u128 = 0;
        let mut messages_ms: u128 = 0;
        let mut composer_ms: u128 = 0;

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let t_sidebar = Instant::now();
                ui.set_min_width(160.0);
                self.show_sidebar(ui, i18n);
                sidebar_ms = t_sidebar.elapsed().as_millis();
            });
            ui.separator();
            ui.vertical(|ui| {
                let dark_mode = ui.visuals().dark_mode;
                let panel_bg = if dark_mode {
                    egui::Color32::from_rgb(36, 38, 44)
                } else {
                    egui::Color32::from_rgb(240, 242, 245)
                };
                let panel_text = if dark_mode {
                    egui::Color32::from_rgb(220, 224, 234)
                } else {
                    egui::Color32::from_rgb(34, 34, 34)
                };

                // Feature 9: message search in current session.
                if CHAT_STAGE6_ENABLE_SEARCH_ROW {
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("chat.search"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.message_search_query)
                                .hint_text(i18n.t("chat.searchMessages"))
                                .desired_width(ui.available_width()),
                        );
                    });
                    ui.add_space(4.0);
                }

                // ── Top: conversation messages ──────────────────────────
                let avail = ui.available_height();
                // Keep enough room for mode row + attachments/input + button row.
                // Without this cap, ScrollArea can consume nearly all height and push controls off-screen.
                let reserved_bottom = 280.0;
                let top_height = (avail - reserved_bottom).max(120.0);
                let t_messages = Instant::now();
                egui::ScrollArea::vertical()
                    .max_height(top_height.max(100.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.show_messages(ui, i18n);
                    });
                messages_ms = t_messages.elapsed().as_millis();

                ui.separator();
                ui.add_space(4.0);

                // ── Mode selector row ──────────────────────────────────
                if CHAT_STAGE6_ENABLE_MODE_ROW {
                    egui::Frame::new()
                        .fill(panel_bg)
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(10i8, 6i8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(i18n.t("chat.mode"))
                                        .color(panel_text)
                                        .strong(),
                                );
                                ui.add_space(6.0);
                                egui::ComboBox::from_id_salt("mode_sel")
                                    .selected_text(i18n.t(&format!("mode.{}", self.selected_mode)))
                                    .show_ui(ui, |ui| {
                                        let modes =
                                            ["ask", "plan", "edit", "safeguard", "full_auto"];
                                        for val in &modes {
                                            ui.selectable_value(
                                                &mut self.selected_mode,
                                                val.to_string(),
                                                i18n.t(&format!("mode.{val}")),
                                            );
                                        }
                                    });

                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(i18n.t("chat.model")).color(panel_text),
                                );
                                if ui
                                    .button(i18n.t("chat.chooseModels"))
                                    .on_hover_text(i18n.t("chat.multiModelHint"))
                                    .clicked()
                                {
                                    self.show_model_picker = true;
                                }

                                // Feature 6: show active model name
                                ui.add_space(12.0);
                                ui.label(
                                    egui::RichText::new(self.selected_models_summary(i18n))
                                        .color(panel_text)
                                        .size(12.0),
                                );
                            });
                        });
                }
                if CHAT_STAGE6_ENABLE_METADATA_SYNC {
                    let mut metadata_changed = false;
                    if self.active_session < self.sessions.len() {
                        let session = &mut self.sessions[self.active_session];
                        if session.mode != self.selected_mode {
                            session.mode = self.selected_mode.clone();
                            metadata_changed = true;
                        }
                        if session.phase != self.selected_phase {
                            session.phase = self.selected_phase.clone();
                            metadata_changed = true;
                        }
                        if session.model != self.selected_model {
                            session.model = self.selected_model.clone();
                            metadata_changed = true;
                        }
                        if session.models != self.selected_models {
                            session.models = self.selected_models.clone();
                            metadata_changed = true;
                        }
                    }
                    if metadata_changed {
                        self.save_sessions_to_disk();
                    }
                }

                ui.add_space(4.0);

                if !self.input_ready {
                    self.input_ready = true;
                    ui.ctx().request_repaint();
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.label("Building input...");
                    });
                    return;
                }

                let t_composer = Instant::now();

                // ── Input area with attachments ────────────────────────
                if !self.attachments.is_empty() {
                    ui.horizontal(|ui| {
                        for att in &self.attachments {
                            let icon = if att.mime.starts_with("image/") {
                                "🖼️"
                            } else {
                                "📎"
                            };
                            ui.label(format!("{} {}", icon, att.name));
                        }
                        if ui.button("✕").clicked() {
                            self.attachments.clear();
                        }
                    });
                }
                // ── Input box ────────────────────────────────────────────
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut self.input)
                        .hint_text(i18n.t("chat.input"))
                        .desired_rows(3)
                        .desired_width(ui.available_width()),
                );
                // ── Button row (keep for testing) ─────────────────────────
                ui.horizontal(|ui| {
                    if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS
                        && ui
                            .button("\u{1f4ce}")
                            .on_hover_text(i18n.t("chat.attach"))
                            .clicked()
                    {
                        if let Some(files) = rfd::FileDialog::new().pick_files() {
                            for f in files {
                                let n = f
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("file")
                                    .to_string();
                                self.attachments.push(Attachment {
                                    name: n,
                                    mime: Self::guess_mime(&f),
                                    data: f.display().to_string(),
                                });
                            }
                            self.error.clear();
                        }
                    }
                    if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS
                        && ui
                            .button("\u{1f4dd}")
                            .on_hover_text(i18n.t("chat.externalEditor"))
                            .clicked()
                    {
                        let p = std::env::temp_dir().join("go_on_chat_input.txt");
                        let _ = std::fs::write(&p, &self.input);
                        for e in &["zed", "code", "gedit", "vim", "nano"] {
                            if std::process::Command::new(e).arg(&p).spawn().is_ok() {
                                break;
                            }
                        }
                    }
                    // Feature 7: quick prompts button
                    if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS
                        && ui
                            .button("\u{1f4a1}")
                            .on_hover_text(i18n.t("chat.promptTemplates"))
                            .clicked()
                    {
                        self.show_prompts = !self.show_prompts;
                    }
                    // Feature 7: show prompt dropdown
                    if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS && self.show_prompts {
                        egui::Window::new(i18n.t("chat.promptTemplates"))
                            .id(egui::Id::new("quick_prompts_window"))
                            .collapsible(false)
                            .resizable(true)
                            .default_width(520.0)
                            .anchor(egui::Align2::LEFT_TOP, egui::vec2(0.0, 0.0))
                            .show(ui.ctx(), |ui| {
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.template_search_query)
                                            .hint_text(i18n.t("chat.searchTemplates"))
                                            .desired_width(220.0),
                                    );
                                    if ui.button(i18n.t("chat.templateNew")).clicked() {
                                        self.selected_template_idx = None;
                                        self.template_name_buf.clear();
                                        self.template_command_buf.clear();
                                        self.template_content_buf.clear();
                                    }
                                });
                                ui.separator();
                                ui.columns(2, |columns| {
                                    columns[0].vertical(|ui| {
                                        let query = self.template_search_query.to_ascii_lowercase();
                                        let mut pick_idx = None;
                                        for (idx, template) in
                                            self.prompt_templates.iter().enumerate()
                                        {
                                            if !query.is_empty()
                                                && !template
                                                    .name
                                                    .to_ascii_lowercase()
                                                    .contains(&query)
                                                && !template
                                                    .command
                                                    .to_ascii_lowercase()
                                                    .contains(&query)
                                            {
                                                continue;
                                            }
                                            let label =
                                                format!("{}  {}", template.command, template.name);
                                            if ui
                                                .selectable_label(
                                                    self.selected_template_idx == Some(idx),
                                                    label,
                                                )
                                                .clicked()
                                            {
                                                pick_idx = Some(idx);
                                            }
                                        }
                                        if let Some(idx) = pick_idx {
                                            self.load_template_into_editor(idx);
                                        }
                                    });

                                    columns[1].vertical(|ui| {
                                        ui.label(i18n.t("chat.templateName"));
                                        ui.text_edit_singleline(&mut self.template_name_buf);
                                        ui.label(i18n.t("chat.templateCommand"));
                                        ui.text_edit_singleline(&mut self.template_command_buf);
                                        ui.label(i18n.t("chat.templateBody"));
                                        ui.add(
                                            egui::TextEdit::multiline(
                                                &mut self.template_content_buf,
                                            )
                                            .desired_rows(10)
                                            .desired_width(ui.available_width()),
                                        );
                                        ui.label(i18n.t("chat.templatePlaceholderHint"));
                                        ui.horizontal(|ui| {
                                            if ui.button(i18n.t("chat.templateInsert")).clicked() {
                                                self.input = self.template_content_buf.clone();
                                                self.show_prompts = false;
                                            }
                                            if ui.button(i18n.t("chat.templateSave")).clicked() {
                                                let name = self.template_name_buf.trim();
                                                let command = Self::normalize_command(
                                                    &self.template_command_buf,
                                                );
                                                let content = self.template_content_buf.trim();
                                                if name.is_empty()
                                                    || command.is_empty()
                                                    || content.is_empty()
                                                {
                                                    self.error = i18n
                                                        .t("chat.templateValidation")
                                                        .to_string();
                                                } else if self
                                                    .prompt_templates
                                                    .iter()
                                                    .enumerate()
                                                    .any(|(idx, t)| {
                                                        t.command == command
                                                            && Some(idx)
                                                                != self.selected_template_idx
                                                    })
                                                {
                                                    self.error = i18n
                                                        .t("chat.templateDuplicate")
                                                        .to_string();
                                                } else {
                                                    let template = PromptTemplate {
                                                        id: self
                                                            .selected_template_idx
                                                            .and_then(|idx| {
                                                                self.prompt_templates
                                                                    .get(idx)
                                                                    .map(|t| t.id.clone())
                                                            })
                                                            .unwrap_or_else(|| {
                                                                format!(
                                                                    "tpl_{}",
                                                                    self.prompt_templates.len() + 1
                                                                )
                                                            }),
                                                        name: name.to_string(),
                                                        command,
                                                        content: content.to_string(),
                                                    };
                                                    if let Some(idx) = self.selected_template_idx {
                                                        self.prompt_templates[idx] = template;
                                                    } else {
                                                        self.prompt_templates.push(template);
                                                        self.selected_template_idx =
                                                            Some(self.prompt_templates.len() - 1);
                                                    }
                                                    self.save_templates_to_disk();
                                                    self.error.clear();
                                                }
                                            }
                                            if ui.button(i18n.t("chat.templateDelete")).clicked() {
                                                if let Some(idx) = self.selected_template_idx.take()
                                                {
                                                    if idx < self.prompt_templates.len() {
                                                        self.prompt_templates.remove(idx);
                                                        self.save_templates_to_disk();
                                                        self.template_name_buf.clear();
                                                        self.template_command_buf.clear();
                                                        self.template_content_buf.clear();
                                                    }
                                                }
                                            }
                                        });
                                    });
                                });
                                if ui.button(i18n.t("chat.close")).clicked() {
                                    self.show_prompts = false;
                                }
                            });
                    }
                    // Fill remaining space, then Send/Stop button on the right
                    ui.add_space(8.0);
                    let send_hint_key = if cfg!(target_os = "linux") {
                        "chat.sendShortcutHintLinux"
                    } else {
                        "chat.sendShortcutHint"
                    };
                    ui.label(egui::RichText::new(i18n.t(send_hint_key)).small().weak());

                    // Feature 4: stop button replaces send when thinking
                    if self.sending && self.ai_status == AiStatus::Thinking {
                        let stop_btn =
                            egui::Button::new(format!("\u{23f9} {}", i18n.t("chat.stop")))
                                .fill(egui::Color32::RED)
                                .min_size(egui::vec2(80.0, 28.0));
                        if ui.add(stop_btn).clicked() {
                            self.stop_sending();
                        }
                    } else {
                        let (icon, col) = match self.ai_status {
                            AiStatus::Idle => (
                                i18n.t("chat.send").to_string(),
                                egui::Color32::from_rgb(40, 120, 220),
                            ),
                            AiStatus::Thinking => {
                                ("...".to_string(), egui::Color32::from_rgb(200, 160, 60))
                            }
                            AiStatus::Error => {
                                (i18n.t("chat.retry").to_string(), egui::Color32::RED)
                            }
                        };
                        let snd = egui::Button::new(format!("\u{25b6} {}", icon))
                            .fill(col)
                            .min_size(egui::vec2(80.0, 28.0));
                        if ui.add_enabled(!self.sending, snd).clicked() {
                            self.send_message(backend, ctx, autotune_chain_enabled);
                        }
                    }
                });

                if CHAT_STAGE6_ENABLE_MODEL_PICKER_WINDOW && self.show_model_picker {
                    egui::Window::new(i18n.t("chat.chooseModels"))
                        .id(egui::Id::new("chat_model_picker_window"))
                        .collapsible(false)
                        .resizable(true)
                        .default_width(360.0)
                        .show(ui.ctx(), |ui| {
                            ui.label(i18n.t("chat.multiModelHint"));
                            ui.separator();
                            let available_models = self.available_models.clone();
                            for model in &available_models {
                                let mut checked = self.selected_models.iter().any(|m| m == model);
                                if ui.checkbox(&mut checked, model).changed() {
                                    if checked {
                                        self.selected_models.push(model.clone());
                                    } else {
                                        self.selected_models.retain(|m| m != model);
                                    }
                                    self.sync_model_selection();
                                }
                            }
                            ui.separator();
                            ui.horizontal(|ui| {
                                if ui.button(i18n.t("chat.modelAutoOnly")).clicked() {
                                    self.selected_models = vec!["auto".to_string()];
                                    self.sync_model_selection();
                                }
                                if ui.button(i18n.t("chat.close")).clicked() {
                                    self.show_model_picker = false;
                                }
                            });
                        });
                }

                // ── Enter to send ─────────────────────────────────────────
                let should_send_with_enter = ui.input(|i| {
                    if !resp.has_focus() || !i.key_pressed(egui::Key::Enter) || i.modifiers.shift {
                        return false;
                    }
                    #[cfg(target_os = "linux")]
                    {
                        // On Linux/fcitx5, plain Enter is used by IME candidate selection.
                        // Require Ctrl/Command+Enter for sending to avoid accidental submits.
                        i.modifiers.ctrl || i.modifiers.command
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        true
                    }
                });
                if should_send_with_enter {
                    self.send_message(backend, ctx, autotune_chain_enabled);
                }

                // Show error if present
                if !self.error.is_empty() {
                    ui.colored_label(egui::Color32::RED, &self.error);
                }

                composer_ms = t_composer.elapsed().as_millis();
            });
        });

        if debug_enabled {
            let total_ms = _start.elapsed().as_millis();
            Self::push_perf_sample(&mut self.perf_total_samples, total_ms);
            Self::push_perf_sample(&mut self.perf_sidebar_samples, sidebar_ms);
            Self::push_perf_sample(&mut self.perf_messages_samples, messages_ms);
            Self::push_perf_sample(&mut self.perf_composer_samples, composer_ms);
            self.perf_frame_counter = self.perf_frame_counter.saturating_add(1);

            if total_ms > 40 || messages_ms > 30 || composer_ms > 25 || sidebar_ms > 20 {
                Self::chat_debug_log(&format!(
                    "[CHAT_PERF] total={}ms sidebar={}ms messages={}ms composer={}ms sessions={} msgs={}",
                    total_ms,
                    sidebar_ms,
                    messages_ms,
                    composer_ms,
                    self.sessions.len(),
                    self.messages().len()
                ));
            }

            if self.perf_frame_counter % CHAT_PERF_SUMMARY_INTERVAL == 0 {
                Self::chat_debug_log(&format!(
                    "[CHAT_PERF_SUMMARY] window={} avg(total/sidebar/messages/composer)={}/{}/{}/{}ms p95(total/messages)={}/{}ms max(total/messages)={}/{}ms",
                    self.perf_total_samples.len(),
                    Self::perf_avg(&self.perf_total_samples),
                    Self::perf_avg(&self.perf_sidebar_samples),
                    Self::perf_avg(&self.perf_messages_samples),
                    Self::perf_avg(&self.perf_composer_samples),
                    Self::perf_p95(&self.perf_total_samples),
                    Self::perf_p95(&self.perf_messages_samples),
                    Self::perf_max(&self.perf_total_samples),
                    Self::perf_max(&self.perf_messages_samples),
                ));
            }
        }
    }

    fn show_safe_chat_layout(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
        autotune_chain_enabled: bool,
    ) -> (u128, u128, u128) {
        let mut sidebar_ms: u128 = 0;
        let mut messages_ms: u128 = 0;
        let mut composer_ms: u128 = 0;

        let enable_input_widget = CHAT_ISOLATION_STAGE >= 2;
        let enable_enter_send = CHAT_ISOLATION_STAGE >= 3;
        let enable_show_messages = CHAT_ISOLATION_STAGE >= 4;
        let enable_sidebar = CHAT_ISOLATION_STAGE >= 5;

        let total_w = ui.available_width();
        let total_h = ui.available_height().min(4096.0); // guard against infinity
        let sidebar_w = (total_w / 3.0).max(160.0);
        // separator takes ~1px + 8px margin
        let content_w = (total_w - sidebar_w - 9.0).max(200.0);
        // Reserve ~130px for input area (multiline 3 rows + button bar + separator + spacing)
        let msg_area_h = (total_h - 130.0).max(150.0);

        ui.horizontal(|ui| {
            ui.allocate_ui(egui::vec2(sidebar_w, total_h), |ui| {
                if enable_sidebar {
                    let t_sidebar = Instant::now();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.show_sidebar(ui, i18n);
                        });
                    sidebar_ms = t_sidebar.elapsed().as_millis();
                } else {
                    ui.label("Sidebar disabled");
                }
            });

            ui.separator();

            // Right panel: use bottom_up layout so composer (input+buttons)
            // is always pinned at the bottom and messages fill the rest.
            ui.allocate_ui(egui::vec2(content_w, total_h), |ui| {
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    let t_composer = Instant::now();

                    // ── Render bottom items first (they appear at the bottom) ──

                    // Model picker window (floating, no layout impact)
                    if CHAT_STAGE6_ENABLE_MODEL_PICKER_WINDOW && self.show_model_picker {
                        egui::Window::new(i18n.t("chat.chooseModels"))
                            .id(egui::Id::new("chat_model_picker_window_safe"))
                            .collapsible(false)
                            .resizable(true)
                            .default_width(360.0)
                            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                            .show(ui.ctx(), |ui| {
                                ui.label(i18n.t("chat.multiModelHint"));
                                ui.separator();
                                let available_models = self.available_models.clone();
                                for model in &available_models {
                                    let mut checked =
                                        self.selected_models.iter().any(|m| m == model);
                                    if ui.checkbox(&mut checked, model).changed() {
                                        if checked {
                                            self.selected_models.push(model.clone());
                                        } else {
                                            self.selected_models.retain(|m| m != model);
                                        }
                                        self.sync_model_selection();
                                    }
                                }
                                ui.separator();
                                ui.horizontal(|ui| {
                                    if ui.button(i18n.t("chat.modelAutoOnly")).clicked() {
                                        self.selected_models = vec!["auto".to_string()];
                                        self.sync_model_selection();
                                    }
                                    if ui.button(i18n.t("chat.close")).clicked() {
                                        self.show_model_picker = false;
                                    }
                                });
                            });
                    }

                    // Error label (very bottom)
                    if !self.error.is_empty() {
                        ui.colored_label(egui::Color32::RED, &self.error);
                    }

                    // Send / Stop button row
                    ui.horizontal(|ui| {
                        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS {
                            if ui
                                .button("📎")
                                .on_hover_text(i18n.t("chat.attach"))
                                .clicked()
                            {
                                if let Some(files) = rfd::FileDialog::new().pick_files() {
                                    for f in files {
                                        let n = f
                                            .file_name()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or("file")
                                            .to_string();
                                        self.attachments.push(Attachment {
                                            name: n,
                                            mime: Self::guess_mime(&f),
                                            data: f.display().to_string(),
                                        });
                                    }
                                    self.error.clear();
                                }
                            }
                            if ui
                                .button("📝")
                                .on_hover_text(i18n.t("chat.externalEditor"))
                                .clicked()
                            {
                                let p = std::env::temp_dir().join("go_on_chat_input.txt");
                                let _ = std::fs::write(&p, &self.input);
                                for e in &["zed", "code", "gedit", "vim", "nano"] {
                                    if std::process::Command::new(e).arg(&p).spawn().is_ok() {
                                        break;
                                    }
                                }
                            }
                            if ui
                                .button("💡")
                                .on_hover_text(i18n.t("chat.promptTemplates"))
                                .clicked()
                            {
                                self.show_prompts = !self.show_prompts;
                            }
                        }

                        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS
                            && ui
                                .button(i18n.t("chat.chooseModels"))
                                .on_hover_text(i18n.t("chat.multiModelHint"))
                                .clicked()
                        {
                            self.show_model_picker = true;
                        }

                        if self.sending && self.ai_status == AiStatus::Thinking {
                            if ui
                                .add(
                                    egui::Button::new(format!("⏹ {}", i18n.t("chat.stop")))
                                        .fill(egui::Color32::RED),
                                )
                                .clicked()
                            {
                                self.stop_sending();
                            }
                        } else if ui
                            .add_enabled(
                                !self.sending,
                                egui::Button::new(format!("▶ {}", i18n.t("chat.send")))
                                    .fill(egui::Color32::from_rgb(40, 120, 220)),
                            )
                            .clicked()
                        {
                            self.send_message(backend, ctx, autotune_chain_enabled);
                        }
                    });

                    // Attachments row
                    if !self.attachments.is_empty() {
                        ui.horizontal(|ui| {
                            for att in &self.attachments {
                                let icon = if att.mime.starts_with("image/") {
                                    "🖼️"
                                } else {
                                    "📎"
                                };
                                ui.label(format!("{} {}", icon, att.name));
                            }
                            if ui.button("✕").clicked() {
                                self.attachments.clear();
                            }
                        });
                    }

                    // Text input
                    let mut input_has_focus = false;
                    if enable_input_widget {
                        let input_resp = ui.add(
                            egui::TextEdit::multiline(&mut self.input)
                                .hint_text(i18n.t("chat.input"))
                                .desired_rows(3)
                                .desired_width(content_w),
                        );
                        input_has_focus = input_resp.has_focus();
                        // Enter-to-send (check immediately after input while has_focus is valid)
                        if enable_enter_send
                            && ui.input(|i| {
                                input_has_focus
                                    && i.key_pressed(egui::Key::Enter)
                                    && !i.modifiers.shift
                            })
                        {
                            self.send_message(backend, ctx, autotune_chain_enabled);
                        }
                    }

                    composer_ms = t_composer.elapsed().as_millis();

                    ui.separator();

                    // ── Messages fill all remaining height (rendered last = top) ──
                    let t_messages = Instant::now();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if enable_show_messages {
                                self.show_messages(ui, i18n);
                            } else {
                                const MAX_SHOWN: usize = 20;
                                let msgs = self.messages();
                                let start = msgs.len().saturating_sub(MAX_SHOWN);
                                for msg in msgs.iter().skip(start) {
                                    let role = if msg.role == "user" { "U" } else { "A" };
                                    let mut text = if CHAT_DISABLE_MARKDOWN_RENDER {
                                        Self::markdown_to_plain_text(&msg.content)
                                    } else {
                                        msg.content.clone()
                                    };
                                    if text.chars().count() > 240 {
                                        text = text.chars().take(240).collect::<String>() + "...";
                                    }
                                    ui.label(format!("[{}] {}", role, text));
                                }
                            }
                        });
                    messages_ms = t_messages.elapsed().as_millis();
                }); // end bottom_up layout
            }); // end allocate_ui (right panel)
        }); // end ui.horizontal

        (sidebar_ms, messages_ms, composer_ms)
    }

    // ── Sidebar: session list ───────────────────────────────────
    fn show_sidebar(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(i18n.t("chat.title"));
                // Feature 8: export button
                if ui
                    .button("\u{1f4e4}")
                    .on_hover_text(i18n.t("chat.export"))
                    .clicked()
                {
                    let msgs = self.messages();
                    let mut md = String::new();
                    md.push_str(&format!("# {}\n\n", i18n.t("chat.exportTitle")));
                    let exported_at = format_absolute_time(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                    md.push_str(&format!(
                        "_{}_\n\n",
                        i18n.t("chat.exportedAt").replace("{time}", &exported_at)
                    ));
                    for msg in msgs {
                        let role_label = if msg.role == "user" {
                            format!("**{}**", i18n.t("chat.exportRoleYou"))
                        } else {
                            format!("**{}**", i18n.t("chat.exportRoleAssistant"))
                        };
                        md.push_str(&format!(
                            "{} ({})\n\n",
                            role_label,
                            format_absolute_time(msg.timestamp)
                        ));
                        if !msg.model.is_empty() {
                            md.push_str(&format!(
                                "_{}_\n\n",
                                i18n.t("chat.exportModel").replace("{model}", &msg.model)
                            ));
                        }
                        md.push_str(&format!("{}\n\n", msg.content));
                        if !msg.thinking.is_empty() {
                            md.push_str(&format!(
                                "> {}\n\n",
                                i18n.t("chat.exportThinking")
                                    .replace("{thinking}", &msg.thinking)
                            ));
                        }
                    }
                    let default_name = self
                        .sessions
                        .get(self.active_session)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "chat-export".to_string())
                        .replace('/', "-");
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name(&format!("{default_name}.md"))
                        .save_file()
                    {
                        match std::fs::write(&path, md) {
                            Ok(()) => {
                                self.error = i18n
                                    .t("chat.exportSuccess")
                                    .replace("{path}", &path.display().to_string());
                            }
                            Err(e) => {
                                self.error = i18n
                                    .t("chat.exportFailed")
                                    .replace("{error}", &e.to_string());
                            }
                        }
                    }
                }
                if ui
                    .button("＋")
                    .on_hover_text(i18n.t("chat.newSession"))
                    .clicked()
                {
                    self.new_session();
                    self.refresh_default_session_names(i18n);
                }
            });
            // Feature 9: search field
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.session_search_query)
                    .hint_text(i18n.t("chat.searchSessions"))
                    .desired_width(ui.available_width()),
            );
            ui.separator();
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .max_height(ui.available_height().max(100.0))
                .show(ui, |ui| {
                    let mut to_remove: Option<usize> = None;
                    // Feature 9: filter by search query
                    let filtered_sessions: Vec<(usize, String, String, String, Vec<String>)> =
                        if self.session_search_query.is_empty() {
                            self.sessions
                                .iter()
                                .enumerate()
                                .map(|(idx, s)| {
                                    (
                                        idx,
                                        s.name.clone(),
                                        s.mode.clone(),
                                        s.phase.clone(),
                                        s.models.clone(),
                                    )
                                })
                                .collect()
                        } else {
                            let q = self.session_search_query.to_lowercase();
                            self.sessions
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| s.name.to_lowercase().contains(&q))
                                .map(|(idx, s)| {
                                    (
                                        idx,
                                        s.name.clone(),
                                        s.mode.clone(),
                                        s.phase.clone(),
                                        s.models.clone(),
                                    )
                                })
                                .collect()
                        };
                    for (idx, session_name, session_mode, session_phase, session_models) in
                        filtered_sessions
                    {
                        let selected = idx == self.active_session;
                        let dark_mode = ui.visuals().dark_mode;
                        let bg = if selected {
                            if dark_mode {
                                egui::Color32::from_rgb(52, 96, 170)
                            } else {
                                egui::Color32::from_rgb(40, 100, 200)
                            }
                        } else {
                            if dark_mode {
                                egui::Color32::from_rgb(40, 42, 48)
                            } else {
                                egui::Color32::from_rgb(86, 90, 98)
                            }
                        };

                        egui::Frame::NONE
                            .fill(bg)
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(egui::Margin::same(6i8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.set_min_width(160.0);
                                    // Feature 9: highlight matching text
                                    if self.session_search_query.is_empty() {
                                        if ui.selectable_label(selected, &session_name).clicked() {
                                            self.active_session = idx;
                                            self.selected_mode = session_mode.clone();
                                            self.selected_phase = session_phase.clone();
                                            self.selected_models = if session_models.is_empty() {
                                                vec!["auto".to_string()]
                                            } else {
                                                session_models.clone()
                                            };
                                            self.sync_model_selection();
                                            self.ai_status = AiStatus::Idle;
                                            self.edit_msg_idx = None;
                                            self.edit_msg_buf.clear();
                                        }
                                    } else {
                                        // Highlight matched text
                                        let label = ui.selectable_label(selected, "").clicked();
                                        let _resp = ui.label(
                                            egui::RichText::new(&session_name)
                                                .color(egui::Color32::WHITE),
                                        );
                                        // Reuse the click from the selectable_label
                                        if label {
                                            self.active_session = idx;
                                            self.selected_mode = session_mode.clone();
                                            self.selected_phase = session_phase.clone();
                                            self.selected_models = if session_models.is_empty() {
                                                vec!["auto".to_string()]
                                            } else {
                                                session_models.clone()
                                            };
                                            self.sync_model_selection();
                                            self.ai_status = AiStatus::Idle;
                                            self.edit_msg_idx = None;
                                            self.edit_msg_buf.clear();
                                        }
                                        // Highlight using painter - simpler approach
                                        let q = self.session_search_query.to_lowercase();
                                        if let Some(_start) = session_name.to_lowercase().find(&q) {
                                            let painter = ui.painter();
                                            // Highlight the entire label area as a colored rect
                                            let min_rect = ui.min_rect();
                                            painter.rect_filled(
                                                min_rect,
                                                2.0,
                                                egui::Color32::from_rgba_premultiplied(
                                                    255, 255, 0, 60,
                                                ),
                                            );
                                        }
                                    }
                                    // Right-click context or delete button
                                    if ui.button("✕").on_hover_text(i18n.t("chat.clear")).clicked()
                                    {
                                        to_remove = Some(idx);
                                    }
                                });
                                // Show mode/phase indicator
                                ui.label(format!(
                                    "{} | {}",
                                    i18n.t(&format!("mode.{}", session_mode)),
                                    i18n.t(&format!("phase.{}", session_phase)),
                                ))
                                .highlight();
                            });
                        ui.add_space(2.0);
                    }

                    if let Some(idx) = to_remove {
                        if self.sessions.len() > 1 {
                            self.sessions.remove(idx);
                            if idx < self.active_session {
                                self.active_session -= 1;
                            } else if self.active_session >= self.sessions.len() {
                                self.active_session = self.sessions.len() - 1;
                            }
                            if self.active_session < self.sessions.len() {
                                self.selected_mode =
                                    self.sessions[self.active_session].mode.clone();
                                self.selected_phase =
                                    self.sessions[self.active_session].phase.clone();
                                self.selected_model =
                                    self.sessions[self.active_session].model.clone();
                                self.selected_models =
                                    if self.sessions[self.active_session].models.is_empty() {
                                        vec![self.selected_model.clone()]
                                    } else {
                                        self.sessions[self.active_session].models.clone()
                                    };
                                self.sync_model_selection();
                            }
                            self.save_sessions_to_disk();
                        }
                    }
                });
        });
    }

    // ── Messages area (Cherry Studio style) ─────────────────────
    fn show_messages(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        let render_start = std::time::Instant::now();

        // Immediate bootstrap on function entry
        if !self.debug_log_bootstrapped {
            self.debug_log_bootstrapped = true;
            Self::chat_debug_log("[CHAT_DEBUG_ENTER] show_messages() called");
        }

        let msgs = self.messages().to_vec();
        let total_msgs = msgs.len();

        if total_msgs == 0 {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(i18n.t("chat.noMessages"))
                        .color(egui::Color32::from_rgb(140, 142, 150))
                        .size(16.0),
                );
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(180, 182, 188), i18n.t("chat.hint"));
            });
            return;
        }

        // Calculate pagination
        let pages = (total_msgs + self.const_messages_per_page - 1) / self.const_messages_per_page;
        if self.messages_page >= pages {
            self.messages_page = if pages > 0 { pages - 1 } else { 0 };
        }

        let start_idx = self.messages_page * self.const_messages_per_page;
        let end_idx = (start_idx + self.const_messages_per_page).min(total_msgs);
        let msgs_to_show = &msgs[start_idx..end_idx];

        // Pagination controls
        ui.horizontal(|ui| {
            if ui.button("◀ Prev").clicked() && self.messages_page > 0 {
                self.messages_page -= 1;
            }
            ui.label(format!(
                "Page {} / {} ({}-{})",
                self.messages_page + 1,
                pages,
                start_idx + 1,
                end_idx
            ));
            if ui.button("Next ▶").clicked() && self.messages_page + 1 < pages {
                self.messages_page += 1;
            }
        });
        ui.separator();

        // Render only current page messages
        let dark_mode = ui.visuals().dark_mode;
        for msg in msgs_to_show.iter() {
            let is_user = msg.role == "user";
            let bubble_color = if is_user {
                if dark_mode {
                    egui::Color32::from_rgb(32, 112, 210)
                } else {
                    egui::Color32::from_rgb(10, 106, 255)
                }
            } else {
                if dark_mode {
                    egui::Color32::from_rgb(42, 44, 50)
                } else {
                    egui::Color32::from_rgb(240, 241, 245)
                }
            };
            let text_color = if is_user {
                egui::Color32::WHITE
            } else {
                if dark_mode {
                    egui::Color32::from_rgb(232, 236, 244)
                } else {
                    egui::Color32::from_rgb(28, 28, 32)
                }
            };

            if is_user {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    ui.label(egui::RichText::new("👤").size(20.0));
                    const MAX_DISPLAY: usize = 500;
                    let display_text = if CHAT_DISABLE_MARKDOWN_RENDER {
                        Self::markdown_to_plain_text(&msg.content)
                    } else {
                        msg.content.clone()
                    };
                    let preview = if display_text.len() > MAX_DISPLAY {
                        let safe_str: String = display_text.chars().take(MAX_DISPLAY).collect();
                        format!("{}...", safe_str)
                    } else {
                        display_text
                    };
                    egui::Frame::new()
                        .fill(bubble_color)
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(preview).color(text_color));
                        });
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("🤖").size(20.0));
                    ui.allocate_ui(
                        egui::vec2(ui.available_width() - 10.0, ui.available_height()),
                        |ui| {
                            egui::Frame::new()
                                .fill(bubble_color)
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                                .show(ui, |ui| {
                                    const MAX_DISPLAY: usize = 500;
                                    let display_text = if CHAT_DISABLE_MARKDOWN_RENDER {
                                        Self::markdown_to_plain_text(&msg.content)
                                    } else {
                                        msg.content.clone()
                                    };
                                    let preview = if display_text.len() > MAX_DISPLAY {
                                        let safe_str: String =
                                            display_text.chars().take(MAX_DISPLAY).collect();
                                        format!("{}...", safe_str)
                                    } else {
                                        display_text
                                    };
                                    ui.label(egui::RichText::new(preview).color(text_color));
                                });
                        },
                    );
                });
            }
            ui.add_space(4.0);
        }

        // Feature 5: show aggregate token estimate below last AI message
        if self.last_token_estimate > 0 {
            let msgs = self.messages();
            if let Some(last) = msgs.last() {
                if last.role == "assistant" {
                    ui.horizontal(|ui| {
                        ui.add_space(36.0);
                        ui.colored_label(
                            egui::Color32::from_rgb(140, 142, 150),
                            format!(
                                "\u{26a1} {}",
                                i18n.t("chat.tokenSummary")
                                    .replace("{input}", &self.input_token_estimate.to_string())
                                    .replace("{output}", &self.output_token_estimate.to_string())
                                    .replace("{total}", &self.last_token_estimate.to_string())
                            ),
                        );
                    });
                }
            }
        }

        // Log performance
        let total_ms = render_start.elapsed().as_millis();
        Self::chat_debug_log(&format!(
            "[CHAT_PERF_PAGINATED] total={}ms page={}/{} messages_shown={}",
            total_ms,
            self.messages_page + 1,
            pages,
            msgs_to_show.len()
        ));
    }

    /// Draw a small colored avatar circle with initials, returning the interaction response
    fn avatar_circle_with_actions(
        ui: &mut egui::Ui,
        size: f32,
        color: egui::Color32,
        label: &str,
        _msg_idx: &usize,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
        let painter = ui.painter();
        painter.circle_filled(rect.center(), size / 2.0, color);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        resp
    }

    /// Draw a small colored avatar circle with initials
    #[allow(dead_code)]
    fn avatar_circle(ui: &mut egui::Ui, size: f32, color: egui::Color32, label: &str) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let painter = ui.painter();
        painter.circle_filled(rect.center(), size / 2.0, color);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
    }

    fn render_markdown(
        ui: &mut egui::Ui,
        text: &str,
        copy_code_hint: &str,
        text_color: egui::Color32,
    ) {
        if CHAT_DISABLE_MARKDOWN_RENDER {
            ui.label(egui::RichText::new(Self::markdown_to_plain_text(text)).color(text_color));
            return;
        }

        const MAX_MARKDOWN_CHARS: usize = 5_000;
        if text.len() > MAX_MARKDOWN_CHARS {
            let preview: String = text.chars().take(MAX_MARKDOWN_CHARS).collect();
            ui.colored_label(
                egui::Color32::from_rgb(220, 170, 80),
                format!(
                    "⚠️ Large message ({} chars) truncated for UI safety",
                    text.len()
                ),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(preview).color(text_color));
            return;
        }

        // Simple markdown renderer: handles code blocks (```...```) and plain text.
        // Avoids comrak which can cause UI hangs with certain inputs.
        let mut remaining = text;
        loop {
            if let Some(start) = remaining.find("```") {
                let before = &remaining[..start];
                if !before.trim().is_empty() {
                    for para in before.trim().split("\n\n") {
                        let p = para.trim();
                        if !p.is_empty() {
                            ui.label(egui::RichText::new(p).color(text_color));
                        }
                    }
                }
                remaining = &remaining[start + 3..];
                let endline = remaining.find('\n').unwrap_or(remaining.len());
                let _lang = remaining[..endline].trim().to_string();
                remaining = &remaining[endline.min(remaining.len())..];
                if remaining.starts_with('\n') {
                    remaining = &remaining[1..];
                }
                if let Some(end) = remaining.find("```") {
                    let code = &remaining[..end].trim_end();
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(40, 44, 52))
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.colored_label(egui::Color32::from_rgb(150, 152, 160), "code");
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        if ui
                                            .button("\u{1f4cb}")
                                            .on_hover_text(copy_code_hint)
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(code.to_string());
                                        }
                                    },
                                );
                            });
                            ui.add(egui::Label::new(
                                egui::RichText::new(code.to_string())
                                    .font(egui::FontId::monospace(13.0))
                                    .color(egui::Color32::from_rgb(200, 204, 212)),
                            ));
                        });
                    remaining = &remaining[end + 3..];
                } else {
                    // No closing ``` found — treat the rest as plain text
                    let rest = remaining.trim();
                    if !rest.is_empty() {
                        ui.label(egui::RichText::new(rest).color(text_color));
                    }
                    break;
                }
            } else {
                if !remaining.trim().is_empty() {
                    for para in remaining.trim().split("\n\n") {
                        let p = para.trim();
                        if !p.is_empty() {
                            // Handle inline code with backticks
                            let parts: Vec<&str> = p.split('`').collect();
                            ui.horizontal_wrapped(|ui| {
                                for (i, part) in parts.iter().enumerate() {
                                    if i % 2 == 0 && !part.trim().is_empty() {
                                        ui.label(
                                            egui::RichText::new(part.trim()).color(text_color),
                                        );
                                    } else if !part.trim().is_empty() {
                                        ui.label(
                                            egui::RichText::new(part.trim())
                                                .color(egui::Color32::from_rgb(220, 80, 80))
                                                .family(egui::FontFamily::Monospace),
                                        );
                                    }
                                }
                            });
                            ui.add_space(2.0);
                        }
                    }
                }
                break;
            }
        }
    }

    /// Draw a small colored avatar circle with initials
    #[cfg(test)]
    #[allow(dead_code)]
    /// Render the content inside a message bubble
    fn message_bubble_content(
        &mut self,
        ui: &mut egui::Ui,
        msg: &Message,
        text_color: egui::Color32,
        i18n: &I18n,
    ) {
        // Timestamp + copy row
        ui.horizontal(|ui| {
            let ts_color = egui::Color32::from_rgb(160, 162, 170);
            let time_str = format_absolute_time(msg.timestamp);
            ui.colored_label(ts_color, time_str);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let copy_btn = egui::Button::new("\u{1f4cb}")
                    .min_size(egui::vec2(18.0, 14.0))
                    .fill(egui::Color32::from_rgba_premultiplied(0, 0, 0, 0));
                if ui
                    .add(copy_btn)
                    .on_hover_text(i18n.t("chat.copy"))
                    .clicked()
                {
                    ui.ctx().copy_text(msg.content.clone());
                }
            });
        });

        // Attachments
        for att in &msg.attachments {
            let icon = if att.mime.starts_with("image/") {
                "\u{1f5bc}"
            } else {
                "\u{1f4ce}"
            };
            ui.label(egui::RichText::new(format!("{} {}", icon, att.name)).color(text_color));
        }

        // Think toggle
        if msg.role == "assistant" && !msg.thinking.is_empty() {
            let toggle = if msg.show_thinking_msg {
                ui.button(format!("\u{25b2} {}", i18n.t("chat.thinkingLabel")))
            } else {
                ui.button(format!("\u{25bc} {}", i18n.t("chat.thinkingLabel")))
            };
            if toggle.clicked() {
                let session_msgs = &mut self.session().messages;
                if let Some(m) = session_msgs
                    .iter_mut()
                    .find(|m| m.timestamp == msg.timestamp)
                {
                    m.show_thinking_msg = !m.show_thinking_msg;
                }
            }
        }

        // Thinking text
        if msg.show_thinking_msg && !msg.thinking.is_empty() {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(250, 242, 220))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                .show(ui, |ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 130, 30),
                        i18n.t("chat.thinkingLabel"),
                    );
                    let think_resp = ui.label(
                        egui::RichText::new(&msg.thinking)
                            .color(egui::Color32::from_rgb(80, 60, 20)),
                    );
                    think_resp.context_menu(|ui| {
                        if ui
                            .button(format!("\u{1f4cb} {}", i18n.t("chat.copy")))
                            .clicked()
                        {
                            ui.ctx().copy_text(msg.thinking.clone());
                            ui.close_menu();
                        }
                    });
                });
        }

        // Main content - now handled inline in show_messages via render_markdown
        // Keep this method for backward compatibility / other callers
        let content_resp = ui.label(egui::RichText::new(&msg.content).color(text_color));
        content_resp.context_menu(|ui| {
            if ui
                .button(format!("\u{1f4cb} {}", i18n.t("chat.copy")))
                .clicked()
            {
                ui.ctx().copy_text(msg.content.clone());
                ui.close_menu();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    fn test_chat_view() -> ChatView {
        let (pending_tx, pending_rx) = mpsc::channel();
        ChatView {
            sessions: vec![Session {
                id: "session_1".to_string(),
                name: "New Chat".to_string(),
                messages: Vec::new(),
                created_at: 0,
                workflow_type: "chat".to_string(),
                phase: String::new(),
                mode: "ask".to_string(),
                model: "auto".to_string(),
                models: vec!["auto".to_string()],
                phase_records: Vec::new(),
            }],
            active_session: 0,
            input: String::new(),
            sending: false,
            error: String::new(),
            ai_status: AiStatus::Idle,
            selected_phase: String::new(),
            selected_mode: "ask".to_string(),
            attachments: Vec::new(),
            phases: Vec::new(),
            phases_loaded: false,
            pending_rx,
            pending_tx,
            edit_msg_idx: None,
            edit_msg_buf: String::new(),
            stop_requested: false,
            generation_states: Vec::new(),
            next_generation_id: 1,
            last_token_estimate: 0,
            input_token_estimate: 0,
            output_token_estimate: 0,
            show_prompts: false,
            show_model_picker: false,
            prompt_templates: Vec::new(),
            selected_template_idx: None,
            template_name_buf: String::new(),
            template_command_buf: String::new(),
            template_content_buf: String::new(),
            template_search_query: String::new(),
            templates_bootstrapped: false,
            session_search_query: String::new(),
            message_search_query: String::new(),
            selected_model: "auto".to_string(),
            selected_models: vec!["auto".to_string()],
            available_models: vec!["auto".to_string()],
            models_loaded: false,
            input_ready: false,
            perf_total_samples: VecDeque::with_capacity(CHAT_PERF_WINDOW),
            perf_sidebar_samples: VecDeque::with_capacity(CHAT_PERF_WINDOW),
            perf_messages_samples: VecDeque::with_capacity(CHAT_PERF_WINDOW),
            perf_composer_samples: VecDeque::with_capacity(CHAT_PERF_WINDOW),
            perf_frame_counter: 0,
            debug_log_bootstrapped: false,
        }
    }

    #[test]
    fn normalize_models_dedupes_and_drops_auto_for_multi_select() {
        let normalized = ChatView::normalize_models(&[
            "auto".to_string(),
            "gpt-4.1".to_string(),
            "gpt-4.1".to_string(),
            "claude-sonnet".to_string(),
        ]);

        assert_eq!(normalized, vec!["gpt-4.1", "claude-sonnet"]);
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
            mode: "ask".to_string(),
            model: "auto".to_string(),
            models: vec!["auto".to_string()],
            phase_records: Vec::new(),
        });
        let i18n = I18n::new(Lang::ZhCn);

        view.refresh_default_session_names(&i18n);

        assert_eq!(view.sessions[0].name, "新对话");
        assert_eq!(view.sessions[1].name, "新对话 2");
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

/// Format timestamp as absolute date+time in local timezone (e.g. "2025-05-07 14:30")
fn format_absolute_time(ts: u64) -> String {
    // Use chrono for proper local timezone handling
    let naive = chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| dt.naive_local())
        .unwrap_or_else(|| {
            chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            )
        });
    naive.format("%Y-%m-%d %H:%M").to_string()
}
