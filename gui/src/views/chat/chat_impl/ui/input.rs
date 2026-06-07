//! Input handling sub-module for the Chat UI.
//!
//! Manages the text input area, send button, keyboard shortcuts,
//! and the mode selection row.

use super::super::*;

use super::model_picker;

/// Render the mode selection row (ask/plan/edit/safeguard/full_auto).
pub fn render_mode_row(chat: &mut ChatView, ui: &mut egui::Ui, i18n: &I18n) {
    let dark = ui.visuals().dark_mode;
    let bg = if dark {
        egui::Color32::from_rgb(30, 32, 38)
    } else {
        egui::Color32::from_rgb(245, 246, 248)
    };
    let fg = if dark {
        egui::Color32::from_rgb(190, 194, 204)
    } else {
        egui::Color32::from_rgb(80, 82, 90)
    };
    egui::Frame::new()
        .fill(bg)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10i8, 6i8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(i18n.t("chat.mode")).color(fg).strong());
                ui.add_space(6.0);
                let prev_mode = chat.selected_mode.clone();
                egui::ComboBox::from_id_salt("mode_sel")
                    .selected_text(i18n.t(&mode_display_key(&chat.selected_mode)))
                    .show_ui(ui, |ui| {
                        for m in &["ask", "plan", "edit", "safeguard", "full_auto"] {
                            ui.selectable_value(
                                &mut chat.selected_mode,
                                m.to_string(),
                                i18n.t(&mode_display_key(m)),
                            );
                        }
                    });
                let _ = &prev_mode; // placeholder for future mode-change hooks
                ui.add_space(8.0);
                ui.label(egui::RichText::new(i18n.t("chat.model")).color(fg));
                ui.add_space(6.0);
                let model_changed = model_picker::render_model_picker(chat, ui, i18n);
                if model_changed {
                    chat.sync_model_selection();
                }
                ui.add_space(12.0);
                let _tok_resp = ui.checkbox(
                    &mut chat.show_token_details,
                    i18n.t("chat.showTokenDetails"),
                );
            });
        });
    ui.separator();
}

/// Map mode ID to its i18n key for display.
pub fn mode_display_key(mode_id: &str) -> String {
    match mode_id {
        "ask" => "mode.ask".to_string(),
        "plan" => "mode.plan".to_string(),
        "edit" => "mode.edit".to_string(),
        "safeguard" => "mode.safeguard".to_string(),
        "full_auto" => "mode.full_auto".to_string(),
        other => format!("mode.{other}"),
    }
}

/// Render the send/stop button.
pub fn render_send_button(
    chat: &mut ChatView,
    ui: &mut egui::Ui,
    i18n: &I18n,
    backend: &BackendClient,
    ctx: &egui::Context,
    autotune_chain_enabled: bool,
) {
    if chat.sending && chat.ai_status == AiStatus::Thinking {
        if ui
            .add(egui::Button::new(format!("⏹ {}", i18n.t("chat.stop"))).fill(egui::Color32::RED))
            .clicked()
        {
            chat.stop_sending();
        }
    } else {
        let (icon, col) = match chat.ai_status {
            AiStatus::Idle => (
                i18n.t("chat.send").to_string(),
                egui::Color32::from_rgb(40, 120, 220),
            ),
            AiStatus::Thinking => ("...".to_string(), egui::Color32::from_rgb(200, 160, 60)),
            AiStatus::Error => (i18n.t("chat.retry").to_string(), egui::Color32::RED),
        };
        if ui
            .add_enabled(
                !chat.sending,
                egui::Button::new(format!("▶ {}", icon))
                    .fill(col)
                    .min_size(egui::vec2(80.0, 28.0)),
            )
            .clicked()
        {
            chat.send_message(backend, ctx, autotune_chain_enabled);
        }
    }
}

/// Handle input keyboard shortcuts (Enter to send, Escape to close windows, etc.)
pub fn handle_input_shortcuts(
    chat: &mut ChatView,
    ui: &mut egui::Ui,
    input_focus: bool,
    i18n: &I18n,
    backend: &BackendClient,
    ctx: &egui::Context,
    autotune_chain_enabled: bool,
) {
    #[cfg(target_os = "linux")]
    {
        let mut do_send = false;
        ui.input_mut(|i| {
            if input_focus && i.consume_key(egui::Modifiers::CTRL, egui::Key::Enter) {
                do_send = true;
            }
        });
        if do_send {
            chat.send_message(backend, ctx, autotune_chain_enabled);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        if ui.input(|i| !input_focus || !i.key_pressed(egui::Key::Enter) || i.modifiers.shift) {
            // Don't send
        } else {
            chat.send_message(backend, ctx, autotune_chain_enabled);
        }
    }

    ui.input_mut(|i| {
        if i.consume_key(egui::Modifiers::CTRL, egui::Key::N)
            || i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)
        {
            chat.new_session();
            chat.refresh_default_session_names(i18n);
        }
        if i.consume_key(egui::Modifiers::CTRL, egui::Key::L)
            || i.consume_key(egui::Modifiers::COMMAND, egui::Key::L)
        {
            chat.input.clear();
        }
        if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
            chat.show_prompts = false;
            chat.show_model_picker = false;
        }
    });
}
