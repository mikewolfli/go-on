pub mod autotune;
pub mod chat;
pub mod config_editor;
pub mod monitor;
pub mod providers;
pub mod security;
pub mod security_prefs;
pub mod settings;
pub mod setup;
pub mod skills;
pub mod workflow;

#[allow(dead_code)]
/// Render a label with right-click "Copy" context menu support.
/// The text can be copied to clipboard via the menu or Ctrl+C.
pub fn copy_label(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let resp = ui.label(text);
    let text_owned = text.to_string();
    resp.context_menu(move |ui| {
        if ui.button("📋 Copy").clicked() {
            ui.ctx().copy_text(text_owned.clone());
            ui.close_menu();
        }
    });
    resp
}

/// Render a colored label with right-click "Copy" context menu.
#[allow(dead_code)]
pub fn copy_colored_label(ui: &mut egui::Ui, color: egui::Color32, text: &str) -> egui::Response {
    let resp = ui.colored_label(color, text);
    let text_owned = text.to_string();
    resp.context_menu(move |ui| {
        if ui.button("📋 Copy").clicked() {
            ui.ctx().copy_text(text_owned.clone());
            ui.close_menu();
        }
    });
    resp
}

#[allow(dead_code)]
/// Render a RichText label with right-click "Copy" context menu.
pub fn copy_rich_label(ui: &mut egui::Ui, richtext: egui::RichText) -> egui::Response {
    let text_owned = richtext.text().to_string();
    let resp = ui.label(richtext);
    resp.context_menu(move |ui| {
        if ui.button("📋 Copy").clicked() {
            ui.ctx().copy_text(text_owned.clone());
            ui.close_menu();
        }
    });
    resp
}
