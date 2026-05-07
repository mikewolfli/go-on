use crate::backend::BackendClient;
use crate::i18n::I18n;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub timestamp: u64,
    pub attachments: Vec<Attachment>,
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
    #[serde(default)]
    pub phase_records: Vec<PhaseRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AiStatus {
    Idle,
    Thinking,
    Error,
}

/// Result returned from an async chat call.
struct PendingResponse {
    content: String,
    thinking: String,
    error: Option<String>,
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
    pending_rx: mpsc::Receiver<PendingResponse>,
    pending_tx: mpsc::Sender<PendingResponse>,
}

impl ChatView {
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
            phase_records: Vec::new(),
        }
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
        let path = Self::sessions_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "Failed to create chat session directory {}: {e}",
                    parent.display()
                );
                return;
            }
        }
        match serde_json::to_string_pretty(&self.sessions) {
            Ok(content) => {
                if let Err(e) = std::fs::write(&path, content) {
                    eprintln!("Failed to write chat sessions to {}: {e}", path.display());
                }
            }
            Err(e) => {
                eprintln!("Failed to serialize chat sessions: {e}");
            }
        }
    }

    pub fn new() -> Self {
        let mut sessions = Self::load_sessions_from_disk();
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
            pending_rx,
            pending_tx,
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

    fn new_session(&mut self) {
        let count = self.sessions.len() + 1;
        self.sessions.push(Self::default_session(
            count - 1,
            self.selected_phase.clone(),
            self.selected_mode.clone(),
        ));
        self.active_session = self.sessions.len() - 1;
        self.ai_status = AiStatus::Idle;
        self.attachments.clear();
        self.save_sessions_to_disk();
    }

    /// Send a message asynchronously via the backend.
    pub fn send_message(&mut self, backend: &BackendClient, ctx: &egui::Context) {
        let msg = self.input.trim().to_string();
        if msg.is_empty() || self.sending {
            return;
        }
        let mode = self.selected_mode.clone();
        let phase = self.selected_phase.clone();

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
        let outbound_msg = format!("{msg}{attachment_summary}");

        // Add user message immediately
        self.session().messages.push(Message {
            role: "user".to_string(),
            content: msg.clone(),
            timestamp: now,
            attachments: atts,
            thinking: String::new(),
            show_thinking_msg: false,
        });
        self.save_sessions_to_disk();
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

        // Spawn a non-blocking tokio task that calls the real backend
        let tx = self.pending_tx.clone();
        let backend_clone = backend.clone();
        let ctx_clone = ctx.clone();
        tokio::spawn(async move {
            let resp = backend_clone.chat(&outbound_msg, &mode, &phase).await;
            let _ = tx.send(match resp {
                Ok((content, thinking)) => PendingResponse {
                    content,
                    thinking,
                    error: None,
                },
                Err(e) => PendingResponse {
                    content: String::new(),
                    thinking: String::new(),
                    error: Some(e),
                },
            });
            ctx_clone.request_repaint();
        });
    }

    /// Drain any pending async responses and update the session / `ai_status`.
    fn process_pending(&mut self) {
        while let Ok(pending) = self.pending_rx.try_recv() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // Check for internal control messages
            if let Some(phases_str) = pending.content.strip_prefix("__phases__:") {
                self.phases = phases_str.split(',').map(String::from).collect();
                continue;
            }
            if let Some(editor_content) = pending.content.strip_prefix("__editor__:") {
                self.input = editor_content.to_string();
                continue;
            }

            if let Some(err) = &pending.error {
                self.error = format!("Chat error: {err}");
                // Mark latest running phase as error
                if let Some(record) = self.session().phase_records.iter_mut().rev().find(|r| r.status == "running") {
                    record.status = "error".to_string();
                }
                self.ai_status = AiStatus::Error;
            } else {
                self.session().messages.push(Message {
                    role: "assistant".to_string(),
                    content: pending.content,
                    thinking: pending.thinking,
                    timestamp: now,
                    attachments: Vec::new(),
                    show_thinking_msg: false,
                });
                // Update the latest running phase record to "completed"
                if let Some(record) = self.session().phase_records.iter_mut().rev().find(|r| r.status == "running") {
                    record.status = "completed".to_string();
                }
                self.ai_status = AiStatus::Idle;

                // Auto-name the session from first user message if still default
                {
                    let first_user_content = self
                        .session()
                        .messages
                        .iter()
                        .find(|m| m.role == "user")
                        .map(|m| m.content.clone());
                    if let Some(content) = first_user_content {
                        let is_default = self.session().name == "New Chat"
                            || self.session().name.starts_with("Chat ");
                        if is_default {
                            let truncated: String = content.chars().take(25).collect();
                            self.session().name = truncated;
                        }
                    }
                }

                self.save_sessions_to_disk();
            }
            self.sending = false;
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
    ) {
        // Process any pending async responses
        self.process_pending();

        // Fetch available phases from backend if not yet loaded
        if !self.phases_loaded {
            self.phases_loaded = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                if let Ok(baseline) = backend_clone.config_baseline().await {
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
                        let msg = format!("__phases__:{}", list.join(","));
                        let _ = tx.send(PendingResponse {
                            content: msg,
                            thinking: String::new(),
                            error: None,
                        });
                    }
                }
                ctx_clone.request_repaint();
            });
        }

        // ── Layout: left sidebar (200px) + right content ──────────────
        egui::SidePanel::left("chat_sessions")
            .resizable(true)
            .default_width(200.0)
            .min_width(140.0)
            .show_inside(ui, |ui| {
                self.show_sidebar(ui, i18n);
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            // ── Top: conversation messages ──────────────────────────
            let avail = ui.available_height();
            let top_height = avail - 160.0; // leave room for input area
            egui::ScrollArea::vertical()
                .max_height(top_height.max(100.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.show_messages(ui, i18n);
                });

            ui.separator();
            ui.add_space(4.0);

            // ── Mode selector row ──────────────────────────────────
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(240, 242, 245))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10i8, 6i8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(i18n.t("chat.mode"))
                                .color(egui::Color32::from_rgb(34, 34, 34))
                                .strong(),
                        );
                        ui.add_space(6.0);
                        egui::ComboBox::from_id_salt("mode_sel")
                            .selected_text(i18n.t(&format!("mode.{}", self.selected_mode)))
                            .show_ui(ui, |ui| {
                                let modes = ["ask", "plan", "edit", "safeguard", "full_auto"];
                                for val in &modes {
                                    ui.selectable_value(
                                        &mut self.selected_mode,
                                        val.to_string(),
                                        i18n.t(&format!("mode.{val}")),
                                    );
                                }
                            });
                    });
                });
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
            }
            if metadata_changed {
                self.save_sessions_to_disk();
            }

            ui.add_space(4.0);

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
            let resp = egui::Frame::new()
                .fill(egui::Color32::from_rgb(255, 255, 255))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(6i8, 4i8))
                .show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut self.input)
                        .hint_text(i18n.t("chat.input"))
                        .desired_width(ui.available_width())
                        .desired_rows(1)
                        .frame(false))
                })
                .inner;

            // ── Button row ────────────────────────────────────────────
            ui.horizontal(|ui| {
                if ui.button("\u{1f4ce}").on_hover_text(i18n.t("chat.attach")).clicked() {
                    if let Some(files) = rfd::FileDialog::new().pick_files() {
                        for f in files {
                            let n = f.file_name().and_then(|s| s.to_str()).unwrap_or("file").to_string();
                            self.attachments.push(Attachment { name: n, mime: Self::guess_mime(&f), data: f.display().to_string() });
                        }
                        self.error.clear();
                    }
                }
                if ui.button("\u{1f4dd}").on_hover_text("Editor").clicked() {
                    let p = std::env::temp_dir().join("go_on_chat_input.txt");
                    let _ = std::fs::write(&p, &self.input);
                    for e in &["zed","code","gedit","vim","nano"] {
                        if std::process::Command::new(e).arg(&p).spawn().is_ok() { break; }
                    }
                }
                // Fill remaining space, then Send button on the right
                let (icon, col) = match self.ai_status {
                    AiStatus::Idle => ("Send", egui::Color32::from_rgb(40, 120, 220)),
                    AiStatus::Thinking => ("...", egui::Color32::from_rgb(200, 160, 60)),
                    AiStatus::Error => ("Retry", egui::Color32::RED),
                };
                let snd = egui::Button::new(format!("\u{25b6} {}", icon))
                    .fill(col)
                    .min_size(egui::vec2(80.0, 28.0));
                if ui.add_enabled(!self.sending, snd).clicked() {
                    self.send_message(backend, ctx);
                }
            });

            // ── Enter to send ─────────────────────────────────────────
            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !ui.input(|i| i.modifiers.shift) {
                self.send_message(backend, ctx);
            }

            // Show error if present
            if !self.error.is_empty() {
                ui.colored_label(egui::Color32::RED, &self.error);
            }
        });
    }

    // ── Sidebar: session list ───────────────────────────────────
    fn show_sidebar(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(i18n.t("chat.title"));
                if ui
                    .button("＋")
                    .on_hover_text(i18n.t("chat.title"))
                    .clicked()
                {
                    self.new_session();
                }
            });
            ui.separator();
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .max_height(ui.available_height().max(100.0))
                .show(ui, |ui| {
                    let mut to_remove: Option<usize> = None;
                    for (idx, session) in self.sessions.iter().enumerate() {
                        let selected = idx == self.active_session;
                        let bg = if selected {
                            egui::Color32::from_rgb(40, 100, 200)
                        } else {
                            egui::Color32::DARK_GRAY
                        };

                        egui::Frame::NONE
                            .fill(bg)
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(egui::Margin::same(6i8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.set_min_width(160.0);
                                    if ui.selectable_label(selected, &session.name).clicked() {
                                        self.active_session = idx;
                                        self.selected_mode = session.mode.clone();
                                        self.selected_phase = session.phase.clone();
                                        self.ai_status = AiStatus::Idle;
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
                                    i18n.t(&format!("mode.{}", session.mode)),
                                    i18n.t(&format!("phase.{}", session.phase)),
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
                            }
                            self.save_sessions_to_disk();
                        }
                    }
                });
        });
    }

    // ── Messages area (Cherry Studio style) ─────────────────────
    fn show_messages(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        let msgs = self.messages().to_vec();
        if msgs.is_empty() {
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

        for msg in &msgs {
            let is_user = msg.role == "user";

            // ── Avatar + Bubble row ──────────────────────────────
            // ── Message row ───────────────────────────────────────
            let bw = (ui.available_width() * 0.65).max(120.0);
            if is_user {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        Self::avatar_circle(ui, 28.0, egui::Color32::from_rgb(100, 180, 80), "U");
                        ui.add_space(6.0);
                        ui.allocate_ui(egui::vec2(bw, ui.available_height()), |ui| {
                            egui::Frame::new()
                                .fill(egui::Color32::from_rgb(0, 106, 255))
                                .corner_radius(egui::CornerRadius { nw: 12, ne: 4, sw: 12, se: 12 })
                                .inner_margin(egui::Margin::symmetric(12i8, 8i8))
                                .show(ui, |ui| {
                                    self.message_bubble_content(ui, msg, egui::Color32::WHITE, i18n);
                                });
                        });
                    });
                });
            } else {
                ui.horizontal(|ui| {
                    Self::avatar_circle(ui, 28.0, egui::Color32::from_rgb(0, 106, 255), "A");
                    ui.add_space(6.0);
                    ui.allocate_ui(egui::vec2(bw, ui.available_height()), |ui| {
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(240, 241, 245))
                            .corner_radius(egui::CornerRadius { nw: 4, ne: 12, sw: 12, se: 12 })
                            .inner_margin(egui::Margin::symmetric(12i8, 8i8))
                            .show(ui, |ui| {
                                self.message_bubble_content(ui, msg, egui::Color32::from_rgb(28, 28, 32), i18n);
                            });
                    });
                });
            }
            ui.add_space(4.0);
        }
    }

    /// Draw a small colored avatar circle with initials
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

    /// Render the content inside a message bubble
    fn message_bubble_content(&mut self, ui: &mut egui::Ui, msg: &Message, text_color: egui::Color32, i18n: &I18n) {
        // Timestamp + copy row
        ui.horizontal(|ui| {
            let ts_color = egui::Color32::from_rgb(160, 162, 170);
            let time_str = format_absolute_time(msg.timestamp);
            ui.colored_label(ts_color, time_str);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let copy_btn = egui::Button::new("\u{1f4cb}")
                    .min_size(egui::vec2(18.0, 14.0))
                    .fill(egui::Color32::from_rgba_premultiplied(0, 0, 0, 0));
                if ui.add(copy_btn).on_hover_text("Copy").clicked() {
                    ui.ctx().copy_text(msg.content.clone());
                }
            });
        });

        // Attachments
        for att in &msg.attachments {
            let icon = if att.mime.starts_with("image/") { "\u{1f5bc}" } else { "\u{1f4ce}" };
            ui.label(egui::RichText::new(format!("{} {}", icon, att.name)).color(text_color));
        }

        // Think toggle
        if msg.role == "assistant" && !msg.thinking.is_empty() {
            let toggle = if msg.show_thinking_msg {
                ui.button("\u{25b2} Think")
            } else {
                ui.button("\u{25bc} Think")
            };
            if toggle.clicked() {
                let session_msgs = &mut self.session().messages;
                if let Some(m) = session_msgs.iter_mut().find(|m| m.timestamp == msg.timestamp) {
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
                    ui.colored_label(egui::Color32::from_rgb(180, 130, 30), "\u{1f4ad} Thinking");
                    let think_resp = ui.label(egui::RichText::new(&msg.thinking).color(egui::Color32::from_rgb(80, 60, 20)));
                    think_resp.context_menu(|ui| {
                        if ui.button("\u{1f4cb} Copy").clicked() {
                            ui.ctx().copy_text(msg.thinking.clone());
                            ui.close_menu();
                        }
                    });
                });
        }

        // Main content
        let content_resp = ui.label(egui::RichText::new(&msg.content).color(text_color));
        content_resp.context_menu(|ui| {
            if ui.button("\u{1f4cb} Copy").clicked() {
                ui.ctx().copy_text(msg.content.clone());
                ui.close_menu();
            }
        });
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
