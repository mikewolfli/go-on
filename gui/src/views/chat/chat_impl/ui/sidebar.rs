//! Sidebar rendering sub-module for the Chat UI.
//!
//! Handles rendering of the session list sidebar with session management,
//! search/filter, and workflow generation.

use super::super::*;
use crate::backend::BackendClient;
use crate::i18n::I18n;
use crate::views::chat::types::{AiStatus, ModePolicy, PendingResponse};

pub fn show_sidebar(
    chat: &mut ChatView,
    ui: &mut egui::Ui,
    i18n: &I18n,
    backend: &BackendClient,
    ctx: &egui::Context,
) {
    egui::Frame::NONE.show(ui, |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(i18n.t("chat.title"));
                // Export button
                if ui
                    .button("📤")
                    .on_hover_text(i18n.t("chat.exportSession"))
                    .clicked()
                {
                    let msgs = chat.messages();
                    let mut md = String::new();
                    md.push_str(&format!("# {}\n\n", i18n.t("chat.exportTitle")));
                    let exported_at = format_absolute_time(crate::fs_util::epoch_secs());
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
                    let default_name = chat
                        .session_state
                        .sessions
                        .get(chat.session_state.active_session)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "chat-export".to_string())
                        .replace('/', "-");
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("{default_name}.md"))
                        .save_file()
                    {
                        match std::fs::write(&path, md) {
                            Ok(()) => {
                                chat.error = i18n
                                    .t("chat.exportSuccess")
                                    .replace("{path}", &path.display().to_string());
                            }
                            Err(e) => {
                                chat.error = i18n
                                    .t("chat.exportFailed")
                                    .replace("{error}", &e.to_string());
                            }
                        }
                    }
                }
                if ui
                    .button("📂")
                    .on_hover_text(i18n.t("chat.openConfigDir"))
                    .clicked()
                {
                    if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
                        let config_dir = dirs.config_dir();
                        #[cfg(target_os = "windows")]
                        if let Err(e) = std::process::Command::new("cmd")
                            .args(["/c", "start", "", &config_dir.display().to_string()])
                            .spawn()
                        {
                            eprintln!("Failed to open config directory: {e}");
                        }
                        #[cfg(target_os = "macos")]
                        if let Err(e) = std::process::Command::new("open").arg(config_dir).spawn() {
                            eprintln!("Failed to open config directory: {e}");
                        }
                        #[cfg(target_os = "linux")]
                        if let Err(e) = std::process::Command::new("xdg-open")
                            .arg(config_dir)
                            .spawn()
                        {
                            eprintln!("Failed to open config directory: {e}");
                        }
                        #[cfg(not(any(
                            target_os = "windows",
                            target_os = "macos",
                            target_os = "linux"
                        )))]
                        if let Err(e) = std::process::Command::new("xdg-open")
                            .arg(config_dir)
                            .spawn()
                        {
                            eprintln!("Failed to open config directory: {e}");
                        }
                    }
                }
                if ui
                    .button("＋")
                    .on_hover_text(i18n.t("chat.newSession"))
                    .clicked()
                {
                    chat.new_session();
                    chat.refresh_default_session_names(i18n);
                }
                if ui
                    .button("🗑")
                    .on_hover_text(i18n.t("chat.clearSession"))
                    .clicked()
                {
                    if let Some(session) = chat
                        .session_state
                        .sessions
                        .get_mut(chat.session_state.active_session)
                    {
                        session.messages.clear();
                        chat.save_sessions_to_disk();
                    }
                }
            });
            // Feature 9: search field
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::singleline(&mut chat.session_state.session_search_query)
                    .hint_text(i18n.t("chat.searchSessions"))
                    .desired_width(ui.available_width()),
            );
            ui.separator();
            ui.add_space(4.0);
            // Generate Workflow button
            if ui
                .button("🔄 ".to_string() + &i18n.t("chat.generateWorkflow"))
                .on_hover_text(i18n.t("chat.generateWorkflowHint"))
                .clicked()
            {
                // Collect all user messages from current session
                let msgs = chat.messages();
                let user_msgs: Vec<String> = msgs
                    .iter()
                    .filter(|m| m.role == "user")
                    .map(|m| m.content.clone())
                    .collect();

                if user_msgs.is_empty() {
                    chat.error = i18n.t("chat.noMessagesForWorkflow").to_string();
                } else {
                    // Build the task from conversation history
                    let task = user_msgs.join("\n---\n");
                    let backend_clone = backend.clone();
                    let tx = chat.stream_state.pending_tx.clone();
                    let ctx_clone = ctx.clone();
                    let success_tpl = i18n.t("chat.workflowGenerated").to_string();
                    let failed_tpl = i18n.t("chat.workflowGenError").to_string();
                    tokio::spawn(async move {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            backend_clone.execute_workflow(&task, None, None),
                        )
                        .await
                        {
                            Ok(Ok(value)) => {
                                let msg = if let Some(id) =
                                    value.get("run_id").and_then(|v| v.as_str())
                                {
                                    success_tpl.replace("{workflow}", id)
                                } else {
                                    success_tpl.replace("{workflow}", "OK")
                                };
                                if let Err(e) = tx.try_send(PendingResponse::UiMessage(msg)) {
                                    eprintln!("WARN: chat ui try_send failed: {:?}", e);
                                }
                            }
                            Ok(Err(e)) => {
                                let msg = failed_tpl.replace("{error}", &e);
                                if let Err(e) = tx.try_send(PendingResponse::UiMessage(msg)) {
                                    eprintln!("WARN: chat ui try_send failed: {:?}", e);
                                }
                            }
                            Err(_) => {
                                let msg = failed_tpl.replace("{error}", "timeout");
                                if let Err(e) = tx.try_send(PendingResponse::UiMessage(msg)) {
                                    eprintln!("WARN: chat ui try_send failed: {:?}", e);
                                }
                            }
                        }
                        ctx_clone.request_repaint();
                    });
                }
            }
            ui.separator();
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut to_remove: Option<usize> = None;
                    // Feature 9: filter by search query
                    let filtered_indices: Vec<usize> =
                        if chat.session_state.session_search_query.is_empty() {
                            chat.session_state
                                .sessions
                                .iter()
                                .enumerate()
                                .map(|(idx, _)| idx)
                                .collect()
                        } else {
                            let q = chat.session_state.session_search_query.to_lowercase();
                            chat.session_state
                                .sessions
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| s.name.to_lowercase().contains(&q))
                                .map(|(idx, _)| idx)
                                .collect()
                        };
                    for idx in filtered_indices {
                        let selected = idx == chat.session_state.active_session;

                        // Session row with rename support
                        let is_renaming = chat.session_state.rename_session_idx == Some(idx);
                        if is_renaming {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut chat.session_state.rename_session_buf,
                                    )
                                    .hint_text(i18n.t("chat.sessionNamePlaceholder"))
                                    .desired_width(140.0),
                                );
                                if ui.button(i18n.t("chat.save")).clicked() {
                                    let new_name =
                                        chat.session_state.rename_session_buf.trim().to_string();
                                    if !new_name.is_empty() {
                                        if let Some(s) = chat.session_state.sessions.get_mut(idx) {
                                            s.name = new_name;
                                            chat.save_sessions_to_disk();
                                        }
                                    }
                                    chat.session_state.rename_session_idx = None;
                                    chat.session_state.rename_session_buf.clear();
                                }
                                if ui.button(i18n.t("chat.cancel")).clicked() {
                                    chat.session_state.rename_session_idx = None;
                                    chat.session_state.rename_session_buf.clear();
                                }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                let resp = ui.selectable_label(
                                    selected,
                                    format!(
                                        "\u{200B}{}\u{200B}{}",
                                        idx, &chat.session_state.sessions[idx].name
                                    ),
                                );
                                if resp.double_clicked() {
                                    chat.session_state.rename_session_idx = Some(idx);
                                    chat.session_state.rename_session_buf =
                                        chat.session_state.sessions[idx].name.clone();
                                } else if resp.clicked() {
                                    chat.session_state.active_session = idx;
                                    // Restore mode from session to preserve per-session mode setting
                                    chat.selected_phase =
                                        chat.session_state.sessions[idx].phase.clone();
                                    chat.model_state.selected_model =
                                        chat.session_state.sessions[idx].model.clone();
                                    if !chat.session_state.sessions[idx].mode.is_empty() {
                                        chat.selected_mode =
                                            chat.session_state.sessions[idx].mode.clone();
                                        chat.mode_policy = ModePolicy::new(&chat.selected_mode);
                                    }
                                    chat.sync_model_selection();
                                    chat.ai_status = AiStatus::Idle;
                                    chat.edit_msg_idx = None;
                                    chat.edit_msg_buf.clear();
                                    chat.session_state.rename_session_idx = None;
                                    chat.session_state.rename_session_buf.clear();
                                }
                                // Delete button
                                if ui
                                    .button("✕")
                                    .on_hover_text(i18n.t("chat.deleteSession"))
                                    .clicked()
                                {
                                    to_remove = Some(idx);
                                }
                            });
                        }
                        // Mode/phase indicator as a simple label
                        ui.label(format!(
                            "{} | {}",
                            i18n.t(&format!("mode.{}", chat.session_state.sessions[idx].mode)),
                            i18n.t(&format!("phase.{}", chat.session_state.sessions[idx].phase)),
                        ));
                        ui.add_space(4.0);
                    }

                    if let Some(idx) = to_remove {
                        if chat.session_state.sessions.len() > 1 {
                            chat.session_state.sessions.remove(idx);
                            if idx < chat.session_state.active_session {
                                chat.session_state.active_session =
                                    chat.session_state.active_session.saturating_sub(1);
                            } else if chat.session_state.active_session
                                >= chat.session_state.sessions.len()
                            {
                                chat.session_state.active_session =
                                    chat.session_state.sessions.len().saturating_sub(1);
                            }
                            if chat.session_state.active_session < chat.session_state.sessions.len()
                            {
                                // Restore mode from session to preserve per-session mode setting
                                chat.selected_phase = chat.session_state.sessions
                                    [chat.session_state.active_session]
                                    .phase
                                    .clone();
                                chat.model_state.selected_model = chat.session_state.sessions
                                    [chat.session_state.active_session]
                                    .model
                                    .clone();
                                if !chat.session_state.sessions[chat.session_state.active_session]
                                    .mode
                                    .is_empty()
                                {
                                    chat.selected_mode = chat.session_state.sessions
                                        [chat.session_state.active_session]
                                        .mode
                                        .clone();
                                    chat.mode_policy = ModePolicy::new(&chat.selected_mode);
                                }
                                chat.sync_model_selection();
                            }
                            chat.save_sessions_to_disk();
                        } else {
                            // Can't delete last session — show feedback
                            chat.error = i18n.t("chat.cannotDeleteLastSession").to_string();
                        }
                    }
                });
        });
    });
}
