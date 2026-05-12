use crate::config::{save_app_config, AppConfig};
use crate::i18n::I18n;

pub struct ConfigEditorView {
    draft: String,
    status: String,
    initialized: bool,
    snapshots: Vec<String>,
    search_query: String,
    is_valid_json: bool,
    json_parse_error: String,
    pub applied: bool,
}

impl ConfigEditorView {
    pub fn new() -> Self {
        Self {
            draft: String::new(),
            status: String::new(),
            initialized: false,
            snapshots: Vec::new(),
            search_query: String::new(),
            is_valid_json: true,
            json_parse_error: String::new(),
            applied: false,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        config: &mut AppConfig,
        safe_mode_enabled: bool,
    ) {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if !self.initialized {
                    self.draft = serde_json::to_string_pretty(config).unwrap_or_default();
                    self.initialized = true;
                }

                ui.heading(i18n.t("tab.config"));
                ui.label(i18n.t("config.hint"));
                ui.separator();

                // Search bar
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text(i18n.t("config.search"))
                            .desired_width(200.0),
                    );
                    if !self.search_query.is_empty() {
                        if ui.button("✕").clicked() {
                            self.search_query.clear();
                        }
                        // Live search count
                        let count = self.draft.matches(&self.search_query).count();
                        ui.label(format!("{} matches", count));
                    }
                });

                if !safe_mode_enabled {
                    ui.label(i18n.t("config.safeModeHidden"));
                    ui.add_space(6.0);
                }

                ui.add(
                    egui::TextEdit::multiline(&mut self.draft)
                        .desired_rows(12)
                        .desired_width(f32::INFINITY),
                );

                // Live JSON validation
                let validation_result = serde_json::from_str::<serde_json::Value>(&self.draft);
                match &validation_result {
                    Ok(_) => {
                        self.is_valid_json = true;
                        self.json_parse_error.clear();
                    }
                    Err(e) => {
                        self.is_valid_json = false;
                        self.json_parse_error = format!("⚠ {}", e);
                    }
                }

                // Validation status display
                ui.horizontal(|ui| {
                    if !self.is_valid_json {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 80, 80),
                            &self.json_parse_error,
                        );
                    } else if !self.draft.trim().is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(60, 180, 100),
                            i18n.t("config.validJson"),
                        );
                    }
                });

                ui.horizontal(|ui| {
                    if ui.button(i18n.t("config.reloadCurrent")).clicked() {
                        self.draft = serde_json::to_string_pretty(config).unwrap_or_default();
                        self.status = i18n.t("config.reloaded").to_string();
                    }
                    if safe_mode_enabled && ui.button(i18n.t("config.createSnapshot")).clicked() {
                        self.snapshots.push(self.draft.clone());
                        self.status = format!(
                            "{} (#{}).",
                            i18n.t("config.snapshotSaved"),
                            self.snapshots.len()
                        );
                    }
                    if ui.button(i18n.t("config.applyJson")).clicked() {
                        match serde_json::from_str::<AppConfig>(&self.draft) {
                            Ok(new_cfg) => {
                                if safe_mode_enabled {
                                    self.snapshots.push(
                                        serde_json::to_string_pretty(config).unwrap_or_default(),
                                    );
                                }
                                *config = new_cfg;
                                save_app_config(config);
                                self.status = i18n.t("config.applied").to_string();
                                self.applied = true;
                            }
                            Err(e) => {
                                self.status = format!("{}: {e}", i18n.t("config.invalidJson"));
                            }
                        }
                    }
                });

                if safe_mode_enabled {
                    ui.add_space(6.0);
                    ui.label(format!(
                        "{}: {}",
                        i18n.t("config.snapshots"),
                        self.snapshots.len()
                    ));
                    if ui
                        .add_enabled(
                            !self.snapshots.is_empty(),
                            egui::Button::new(i18n.t("config.rollbackSnapshot")),
                        )
                        .clicked()
                    {
                        if let Some(last) = self.snapshots.pop() {
                            self.draft = last;
                            self.status = i18n.t("config.rolledBack").to_string();
                        }
                    }
                }

                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });
    }
}
