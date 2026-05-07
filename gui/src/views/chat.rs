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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub data: String, // base64 or path
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub messages: Vec<Message>,
    pub created_at: u64,
    pub workflow_type: String,
    pub phase: String,
    pub mode: String,
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
            sessions.push(Self::default_session(
                0,
                "coding".to_string(),
                "ask".to_string(),
            ));
        }
        let initial_phase = sessions
            .first()
            .map(|s| s.phase.clone())
            .unwrap_or_else(|| "coding".to_string());
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
                    "coding".to_string(),
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
        });
        self.save_sessions_to_disk();
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
                Ok(content) => PendingResponse {
                    content,
                    error: None,
                },
                Err(e) => PendingResponse {
                    content: String::new(),
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

            if let Some(err) = &pending.error {
                self.error = format!("Chat error: {err}");
                self.ai_status = AiStatus::Error;
            } else {
                self.session().messages.push(Message {
                    role: "assistant".to_string(),
                    content: pending.content,
                    timestamp: now,
                    attachments: Vec::new(),
                });
                self.ai_status = AiStatus::Idle;
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

            // ── Phase + Mode selector row ──────────────────────────
            ui.horizontal(|ui| {
                ui.label(i18n.t("chat.phase"));
                egui::ComboBox::from_id_salt("phase_sel")
                    .selected_text(i18n.t(&format!("phase.{}", self.selected_phase)))
                    .show_ui(ui, |ui| {
                        let phases = ["coding", "review", "debug", "test", "deploy"];
                        for p in &phases {
                            ui.selectable_value(
                                &mut self.selected_phase,
                                p.to_string(),
                                i18n.t(&format!("phase.{p}")),
                            );
                        }
                    });
                ui.add_space(12.0);
                ui.label(i18n.t("chat.mode"));
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

            ui.horizontal(|ui| {
                // Attachment button
                if ui
                    .button("📎")
                    .on_hover_text(i18n.t("chat.attach"))
                    .clicked()
                {
                    if let Some(files) = rfd::FileDialog::new().pick_files() {
                        for file in files {
                            let name = file
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("attachment")
                                .to_string();
                            let mime = Self::guess_mime(&file);
                            let data = file.display().to_string();
                            self.attachments.push(Attachment { name, mime, data });
                        }
                        self.error.clear();
                    }
                }

                // Text input
                let inp_w = ui.available_width() - 100.0;
                let inp = egui::TextEdit::singleline(&mut self.input)
                    .hint_text(i18n.t("chat.input"))
                    .desired_width(inp_w.max(80.0));
                let resp = ui.add(inp);

                // Send button with dynamic icon based on AI status
                let (icon, color) = match self.ai_status {
                    AiStatus::Idle => ("▶", egui::Color32::from_rgb(40, 120, 220)),
                    AiStatus::Thinking => ("⏳", egui::Color32::from_rgb(200, 160, 60)),
                    AiStatus::Error => ("⚠", egui::Color32::RED),
                };
                let btn = egui::Button::new(icon)
                    .fill(color)
                    .min_size(egui::vec2(40.0, 28.0));

                if ui.add_enabled(!self.sending, btn).clicked()
                    || (!self.sending
                        && resp.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    self.send_message(backend, ctx);
                }
            });

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

    // ── Messages area ───────────────────────────────────────────
    fn show_messages(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        let msgs = self.messages().to_vec();
        if msgs.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.label(i18n.t("chat.noMessages"));
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(100, 120, 140), i18n.t("chat.hint"));
            });
            return;
        }

        for msg in &msgs {
            let is_user = msg.role == "user";
            let align = if is_user {
                egui::Layout::right_to_left(egui::Align::TOP)
            } else {
                egui::Layout::left_to_right(egui::Align::TOP)
            };

            ui.with_layout(align, |ui| {
                let max_w = ui.available_width() * 0.75;
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::symmetric(12i8, 8i8))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w);

                        // Role label + timestamp
                        ui.horizontal(|ui| {
                            let (color, label) = match msg.role.as_str() {
                                "user" => {
                                    (egui::Color32::from_rgb(59, 130, 246), i18n.t("chat.you"))
                                }
                                "assistant" => (
                                    egui::Color32::from_rgb(16, 185, 129),
                                    i18n.t("chat.assistant"),
                                ),
                                _ => (egui::Color32::GRAY, i18n.t("chat.system")),
                            };
                            ui.colored_label(color, label);
                            let ts = msg.timestamp;
                            let time_str = format_time(ts, i18n);
                            ui.colored_label(egui::Color32::from_rgb(120, 130, 140), time_str);
                        });

                        // Attachments
                        for att in &msg.attachments {
                            let icon = if att.mime.starts_with("image/") {
                                "🖼️"
                            } else {
                                "📎"
                            };
                            ui.label(format!("{} {}", icon, att.name));
                        }

                        // Message content
                        ui.label(&msg.content);
                    });
            });
            ui.add_space(8.0);
        }
    }
}

fn format_time(ts: u64, i18n: &I18n) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let diff = now.saturating_sub(ts);
    if diff < 60 {
        i18n.t("time.secondsAgo").replace("{}", &diff.to_string())
    } else if diff < 3600 {
        i18n.t("time.minutesAgo")
            .replace("{}", &(diff / 60).to_string())
    } else if diff < 86400 {
        i18n.t("time.hoursAgo")
            .replace("{}", &(diff / 3600).to_string())
    } else {
        i18n.t("time.daysAgo")
            .replace("{}", &(diff / 86400).to_string())
    }
}
