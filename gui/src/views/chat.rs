use crate::backend::BackendClient;
use crate::i18n::I18n;
use serde::{Deserialize, Serialize};
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
    Generating,
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
    pub show_file_dialog: bool,
    pending_rx: mpsc::Receiver<PendingResponse>,
    pending_tx: mpsc::Sender<PendingResponse>,
}

impl ChatView {
    pub fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let default_session = Session {
            id: "session_1".to_string(),
            name: "New Chat".to_string(),
            messages: Vec::new(),
            created_at: now,
            workflow_type: "chat".to_string(),
            phase: "coding".to_string(),
            mode: "ask".to_string(),
        };

        let (pending_tx, pending_rx) = mpsc::channel();

        Self {
            sessions: vec![default_session],
            active_session: 0,
            input: String::new(),
            sending: false,
            error: String::new(),
            ai_status: AiStatus::Idle,
            selected_phase: "coding".to_string(),
            selected_mode: "ask".to_string(),
            attachments: Vec::new(),
            show_file_dialog: false,
            pending_rx,
            pending_tx,
        }
    }

    fn session(&mut self) -> &mut Session {
        &mut self.sessions[self.active_session]
    }

    fn messages(&self) -> &[Message] {
        &self.sessions[self.active_session].messages
    }

    fn new_session(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let count = self.sessions.len() + 1;
        self.sessions.push(Session {
            id: format!("session_{}", count),
            name: format!("Chat {}", count),
            messages: Vec::new(),
            created_at: now,
            workflow_type: "chat".to_string(),
            phase: self.selected_phase.clone(),
            mode: self.selected_mode.clone(),
        });
        self.active_session = self.sessions.len() - 1;
        self.ai_status = AiStatus::Idle;
        self.attachments.clear();
    }

    /// Send a message asynchronously via the backend.
    pub fn send_message(&mut self, backend: &BackendClient, ctx: &egui::Context) {
        let msg = self.input.trim().to_string();
        if msg.is_empty() || self.sending {
            return;
        }

        self.input.clear();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let atts = std::mem::take(&mut self.attachments);

        // Add user message immediately
        self.session().messages.push(Message {
            role: "user".to_string(),
            content: msg.clone(),
            timestamp: now,
            attachments: atts,
        });
        self.ai_status = AiStatus::Thinking;
        self.sending = true;
        self.error.clear();

        // Spawn a non-blocking tokio task that calls the real backend
        let tx = self.pending_tx.clone();
        let backend_clone = backend.clone();
        let ctx_clone = ctx.clone();
        tokio::spawn(async move {
            let resp = backend_clone.chat(&msg).await;
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

    /// Drain any pending async responses and update the session / ai_status.
    fn process_pending(&mut self) {
        while let Ok(pending) = self.pending_rx.try_recv() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
                .as_secs();

            if let Some(err) = &pending.error {
                self.error = format!("Chat error: {}", err);
                self.ai_status = AiStatus::Error;
            } else {
                self.session().messages.push(Message {
                    role: "assistant".to_string(),
                    content: pending.content,
                    timestamp: now,
                    attachments: Vec::new(),
                });
                self.ai_status = AiStatus::Generating;
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

        // If we were showing Generating (one frame), fall back to Idle
        if self.ai_status == AiStatus::Generating {
            self.ai_status = AiStatus::Idle;
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
                                i18n.t(&format!("phase.{}", p)),
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
                                i18n.t(&format!("mode.{}", val)),
                            );
                        }
                    });
            });
            self.session().mode = self.selected_mode.clone();
            self.session().phase = self.selected_phase.clone();

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
                if ui.button("📎").clicked() {
                    self.show_file_dialog = !self.show_file_dialog;
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
                    AiStatus::Generating => ("⚡", egui::Color32::from_rgb(16, 185, 129)),
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
                if ui.button("＋").clicked() {
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
                                        self.ai_status = AiStatus::Idle;
                                    }
                                    // Right-click context or delete button
                                    if ui.button("✕").clicked() {
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
                            if self.active_session >= self.sessions.len() {
                                self.active_session = self.sessions.len() - 1;
                            }
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
                            let time_str = format_time(ts);
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

fn format_time(ts: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
        .as_secs();
    let diff = now.saturating_sub(ts);
    if diff < 60 {
        format!("{}s ago", diff)
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}
