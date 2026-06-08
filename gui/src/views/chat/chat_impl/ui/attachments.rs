//! Attachment handling sub-module for the Chat UI.
//!
//! Manages file attachments: display, addition via file picker,
//! paste event handling, and external editor integration.

use super::super::*;

/// Render the attachments bar showing current file attachments.
pub fn render_attachments(chat: &mut ChatView, ui: &mut egui::Ui) {
    if !chat.attachments.is_empty() {
        ui.horizontal(|ui| {
            for a in &chat.attachments {
                ui.label(format!(
                    "{} {}",
                    if a.mime.starts_with("image/") {
                        "🖼️"
                    } else {
                        "📎"
                    },
                    a.name
                ));
            }
            if ui.button("✕").clicked() {
                chat.attachments.clear();
            }
        });
    }
}

/// Handle file dialog button for adding attachments.
pub fn handle_attach_button(chat: &mut ChatView, _ui: &mut egui::Ui) {
    if let Some(files) = rfd::FileDialog::new().pick_files() {
        for f in files {
            let n = f
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
                .to_string();
            chat.attachments.push(Attachment {
                name: n,
                mime: ChatView::guess_mime(&f),
                data: f.display().to_string(),
            });
        }
        chat.error.clear();
    }
}

/// Handle external editor button: writes input to temp file and opens editor.
pub fn handle_external_editor(chat: &mut ChatView, _ui: &mut egui::Ui) {
    let p = std::env::temp_dir().join("go_on_chat_input.txt");
    if let Err(e) = std::fs::write(&p, &chat.input) {
        chat.error = format!("Failed to write temp file for external editor: {e}");
    }
    #[cfg(target_os = "windows")]
    let editors = &["notepad", "code", "zed"];
    #[cfg(target_os = "macos")]
    let editors = &["open", "code", "zed", "TextEdit"];
    #[cfg(target_os = "linux")]
    let editors = &["zed", "code", "gedit", "vim", "nano"];
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    let editors: &[&str] = &["code", "vim", "nano"];
    for e in editors {
        if let Ok(mut child) = std::process::Command::new(e).arg(&p).spawn() {
            let p_clone = p.clone();
            let tx = chat.pending_tx.clone();
            std::thread::spawn(move || {
                let _ = child.wait();
                if let Ok(edited) = std::fs::read_to_string(&p_clone) {
                    let trimmed = edited.trim().to_string();
                    if !trimmed.is_empty() {
                        let _ = tx.try_send(PendingResponse::ExternalEditorResult(trimmed));
                    }
                }
            });
            break;
        }
    }
}
