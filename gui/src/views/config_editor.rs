use crate::config::{save_app_config, AppConfig};

pub struct ConfigEditorView {
    draft: String,
    status: String,
    initialized: bool,
}

impl ConfigEditorView {
    pub fn new() -> Self {
        Self {
            draft: String::new(),
            status: String::new(),
            initialized: false,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, config: &mut AppConfig) {
        if !self.initialized {
            self.draft = serde_json::to_string_pretty(config).unwrap_or_default();
            self.initialized = true;
        }

        ui.heading("Config");
        let text = "Edit GUI config as JSON. Apply updates live and persist to disk.".to_string();
        let resp = ui.label(&text);
        resp.context_menu(|ui| {
            if ui.button("📋 Copy").clicked() {
                ui.ctx().copy_text(text.clone());
                ui.close_menu();
            }
        });
        ui.separator();

        ui.add(
            egui::TextEdit::multiline(&mut self.draft)
                .desired_rows(20)
                .desired_width(f32::INFINITY),
        );

        ui.horizontal(|ui| {
            if ui.button("Reload From Current").clicked() {
                self.draft = serde_json::to_string_pretty(config).unwrap_or_default();
                self.status = "Reloaded from in-memory config.".to_string();
            }
            if ui.button("Apply JSON").clicked() {
                match serde_json::from_str::<AppConfig>(&self.draft) {
                    Ok(new_cfg) => {
                        *config = new_cfg;
                        save_app_config(config);
                        self.status = "Config applied and saved.".to_string();
                    }
                    Err(e) => {
                        self.status = format!("Invalid JSON: {e}");
                    }
                }
            }
        });

        if !self.status.is_empty() {
            let text = self.status.clone();
            let resp = ui.label(&text);
            resp.context_menu(|ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close_menu();
                }
            });
        }
    }
}
