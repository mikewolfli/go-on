use crate::backend::BackendClient;
use crate::i18n::I18n;
use crate::views::autotune::AutoTuneView;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const CHAT_DISABLE_MARKDOWN_RENDER: bool = false;
const CHAT_STAGE6_ENABLE_MODE_ROW: bool = true;
const CHAT_STAGE6_ENABLE_EXTRA_BUTTONS: bool = true;

use super::types::{
    AiStatus, Attachment, GenerationState, Message, ModelPerfStats, PendingResponse, PhaseRecord,
    PromptTemplate, Session,
};
mod render;
mod runtime;
mod storage;
mod ui;

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
    // Session rename (double-click on session name)
    rename_session_idx: Option<usize>,
    rename_session_buf: String,
    // Message edit
    edit_msg_idx: Option<usize>,
    edit_msg_buf: String,
    // Feature 4: stop button
    stop_requested: bool,
    generation_states: Vec<GenerationState>,
    next_generation_id: u64,
    // Feature 5: token display with improved accuracy
    last_token_estimate: usize,
    input_token_estimate: usize,
    output_token_estimate: usize,
    /// Enable markdown rendering (default: true)
    enable_markdown: bool,
    /// Show token details (default: true)
    show_token_details: bool,
    /// Model performance stats cache
    model_stats: std::collections::HashMap<String, ModelPerfStats>,
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
    // Save serialization guards (AtomicBool ensures no concurrent file writes)
    session_save_in_flight: Arc<AtomicBool>,
    template_save_in_flight: Arc<AtomicBool>,
    // Feature 6: multi-model
    selected_model: String,
    selected_models: Vec<String>,
    available_models: Vec<String>,
    models_loaded: bool,
    last_selected_agent: String,
    stream_chunk_flush_interval: std::time::Duration,
    stream_repaint_interval: std::time::Duration,
    max_pending_events_per_frame: usize,
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

        out.trim().to_string()
    }

    /// Improved token counting algorithm
    /// Uses a more accurate estimation based on character and word count
    fn estimate_tokens_improved(text: &str) -> usize {
        // Clean text by removing markdown and extra whitespace
        let clean_text = Self::markdown_to_plain_text(text);

        // Token estimation formula (OpenAI compatible)
        // Approximately: 1 token per 4 characters for English text
        // Plus 1 token per word for better accuracy
        let char_count = clean_text.chars().count();
        let word_count = clean_text.split_whitespace().count();

        // Weighted average: 40% from chars, 60% from words
        let from_chars = (char_count as f64 / 4.0).ceil() as usize;
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

    /// Handle paste events from the egui input system.
    /// Detects pasted file paths (common on Linux with Ctrl+Shift+V) and data:image/ URLs.
    /// Returns any attachments that were created from paste events.
    fn handle_paste_events(&mut self, ui: &mut egui::Ui) -> Vec<Attachment> {
        let pasted = ui.input_mut(|i| {
            let mut result = Vec::new();
            i.events.retain(|e| {
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

        if !pasted.is_empty() {
            // We can't call ctx.request_repaint() here because we don't have ctx.
            // The caller will handle this.
        }

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
            conversation_id: None,
            branch_id: None,
        }
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
            rename_session_idx: None,
            rename_session_buf: String::new(),
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
            // Save guards
            session_save_in_flight: Arc::new(AtomicBool::new(false)),
            template_save_in_flight: Arc::new(AtomicBool::new(false)),
            // Feature 6
            selected_model: initial_model,
            selected_models: initial_models,
            available_models: vec!["auto".to_string()],
            models_loaded: false,
            last_selected_agent: String::new(),
            stream_chunk_flush_interval: std::time::Duration::from_millis(33),
            stream_repaint_interval: std::time::Duration::from_millis(33),
            max_pending_events_per_frame: 256,
            // Enhanced features (Phase 2)
            enable_markdown: true,
            show_token_details: true,
            model_stats: std::collections::HashMap::new(),
        }
    }

    fn apply_stability_settings(
        &mut self,
        repaint_ms: u64,
        flush_ms: u64,
        max_pending_events_per_frame: usize,
    ) {
        self.stream_repaint_interval = std::time::Duration::from_millis(repaint_ms.clamp(16, 200));
        self.stream_chunk_flush_interval =
            std::time::Duration::from_millis(flush_ms.clamp(16, 200));
        self.max_pending_events_per_frame = max_pending_events_per_frame.clamp(16, 4096);
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

    fn generation_meta(&self, generation_id: u64) -> Option<(usize, String, std::time::Instant)> {
        self.generation_states
            .iter()
            .find(|state| state.id == generation_id)
            .map(|state| (state.msg_idx, state.model.clone(), state.started_at))
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
        self.rename_session_idx = None;
        self.rename_session_buf.clear();
        self.selected_models = vec![self.selected_model.clone()];
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
    }

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
            phases_load_scheduled: false,
            pending_rx,
            pending_tx,
            rename_session_idx: None,
            rename_session_buf: String::new(),
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
            session_save_in_flight: Arc::new(AtomicBool::new(false)),
            template_save_in_flight: Arc::new(AtomicBool::new(false)),
            selected_model: "auto".to_string(),
            selected_models: vec!["auto".to_string()],
            available_models: vec!["auto".to_string()],
            models_loaded: false,
            last_selected_agent: String::new(),
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
