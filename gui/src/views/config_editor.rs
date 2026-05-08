use crate::config::{save_app_config, AppConfig};
use crate::i18n::I18n;

pub struct ConfigEditorView {
    draft: String,
    status: String,
    initialized: bool,
    snapshots: Vec<String>,
}

impl ConfigEditorView {
    pub fn new() -> Self {
        Self {
            draft: String::new(),
            status: String::new(),
            initialized: false,
            snapshots: Vec::new(),
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        config: &mut AppConfig,
        safe_mode_enabled: bool,
    ) {
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        if !self.initialized {
            self.draft = serde_json::to_string_pretty(config).unwrap_or_default();
            self.initialized = true;
        }

        ui.heading(i18n.t("tab.config"));
        let text = i18n.t("config.hint").to_string();
        let resp = ui.label(&text);
        resp.context_menu(|ui| {
            if ui.button("📋 Copy").clicked() {
                ui.ctx().copy_text(text.clone());
                ui.close_menu();
            }
        });
        ui.separator();

        if !safe_mode_enabled {
            ui.label(i18n.t("config.safeModeHidden"));
            ui.add_space(6.0);
        }

        ui.add(
            egui::TextEdit::multiline(&mut self.draft)
                .desired_rows(20)
                .desired_width(f32::INFINITY),
        );

        ui.horizontal(|ui| {
            if ui.button(i18n.t("config.reloadCurrent")).clicked() {
                self.draft = serde_json::to_string_pretty(config).unwrap_or_default();
                self.status = i18n.t("config.reloaded").to_string();
            }
            if safe_mode_enabled && ui.button(i18n.t("config.createSnapshot")).clicked() {
                self.snapshots.push(self.draft.clone());
                self.status = format!("{} (#{}).", i18n.t("config.snapshotSaved"), self.snapshots.len());
            }
            if ui.button(i18n.t("config.applyJson")).clicked() {
                match serde_json::from_str::<AppConfig>(&self.draft) {
                    Ok(new_cfg) => {
                        if safe_mode_enabled {
                            self.snapshots
                                .push(serde_json::to_string_pretty(config).unwrap_or_default());
                        }
                        *config = new_cfg;
                        save_app_config(config);
                        self.status = i18n.t("config.applied").to_string();
                    }
                    Err(e) => {
                        self.status = format!("{}: {e}", i18n.t("config.invalidJson"));
                    }
                }
            }
        });

        if safe_mode_enabled {
            ui.add_space(6.0);
            ui.label(format!("{}: {}", i18n.t("config.snapshots"), self.snapshots.len()));
            if ui
                .add_enabled(!self.snapshots.is_empty(), egui::Button::new(i18n.t("config.rollbackSnapshot")))
                .clicked()
            {
                if let Some(last) = self.snapshots.pop() {
                    self.draft = last;
                    self.status = i18n.t("config.rolledBack").to_string();
                }
            }
        }

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
        });
    }
}
