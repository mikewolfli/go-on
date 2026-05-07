use crate::i18n::I18n;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub enabled: Option<bool>,
    pub imported_at: Option<u64>,
}

pub struct SkillsView {
    pub skills: Vec<SkillRecord>,
    pub loading: bool,
    pub error: String,
    pub success: String,
    pub show_create: bool,
    pub create_name: String,
    pub create_desc: String,
    pub create_prompt: String,
    pub create_input_schema: String,
    pub import_url: String,
    pub show_import: bool,
}

impl SkillsView {
    pub fn new() -> Self {
        Self {
            skills: Vec::new(),
            loading: false,
            error: String::new(),
            success: String::new(),
            show_create: false,
            create_name: String::new(),
            create_desc: String::new(),
            create_prompt: String::new(),
            create_input_schema: r#"{"query": "string"}"#.to_string(),
            import_url: String::new(),
            show_import: false,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        ui.heading(i18n.t("tab.skills"));
        ui.separator();
        ui.add_space(4.0);

        // Loading indicator
        if self.loading {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(i18n.t("skills.loading"));
            });
            ui.add_space(4.0);
        }

        // Error message
        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::RED, &self.error);
            ui.add_space(4.0);
        }

        // Success message
        if !self.success.is_empty() {
            ui.colored_label(egui::Color32::GREEN, &self.success);
            ui.add_space(4.0);
        }

        // Action buttons
        ui.horizontal(|ui| {
            if ui.button("➕").clicked() {
                self.show_create = !self.show_create;
                self.error.clear();
                self.success.clear();
            }
            if ui.button("📥").clicked() {
                self.show_import = !self.show_import;
                self.error.clear();
                self.success.clear();
            }
            if ui
                .add_enabled(!self.loading, egui::Button::new("🔄"))
                .clicked()
            {
                self.loading = true;
                self.error.clear();
                self.success.clear();
            }
        });

        // Create dialog
        if self.show_create {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(i18n.t("skills.create.title"));
                ui.horizontal(|ui| {
                    ui.label(i18n.t("skills.create.name"));
                    ui.text_edit_singleline(&mut self.create_name);
                });
                ui.horizontal(|ui| {
                    ui.label(i18n.t("skills.create.desc"));
                    ui.text_edit_singleline(&mut self.create_desc);
                });
                ui.label(i18n.t("skills.create.prompt"));
                ui.text_edit_multiline(&mut self.create_prompt);
                ui.label(i18n.t("skills.create.schema"));
                ui.text_edit_multiline(&mut self.create_input_schema);
                if ui
                    .add_enabled(
                        !self.loading,
                        egui::Button::new(i18n.t("skills.create.save")),
                    )
                    .clicked()
                {
                    let name = self.create_name.trim().to_string();
                    let desc = self.create_desc.trim().to_string();
                    let prompt = self.create_prompt.trim().to_string();

                    if name.is_empty() {
                        self.error =
                            format!("{} {}", i18n.t("skills.create.error"), "Name is required.");
                    } else if prompt.is_empty() {
                        self.error = format!(
                            "{} {}",
                            i18n.t("skills.create.error"),
                            "Prompt is required."
                        );
                    } else {
                        self.error.clear();
                        self.success.clear();

                        // Store the record locally
                        self.skills.push(SkillRecord {
                            name: Some(name),
                            description: Some(desc),
                            version: Some("1".to_string()),
                            enabled: Some(true),
                            imported_at: Some(
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
                                    .as_secs(),
                            ),
                        });
                        self.create_name.clear();
                        self.create_desc.clear();
                        self.create_prompt.clear();
                        self.show_create = false;
                        self.success = i18n.t("skills.create.success").to_string();
                    }
                }
            });
        }

        // Import dialog
        if self.show_import {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(i18n.t("skills.import.title"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.import_url)
                            .hint_text(i18n.t("skills.import.placeholder")),
                    );
                    if ui
                        .add_enabled(
                            !self.loading,
                            egui::Button::new(i18n.t("skills.import.btn")),
                        )
                        .clicked()
                    {
                        let url = self.import_url.trim().to_string();
                        if url.is_empty() {
                            self.error =
                                format!("{} {}", i18n.t("skills.import.error"), "URL is required.");
                        } else {
                            self.error.clear();
                            self.success.clear();

                            // Store imported skill locally
                            self.skills.push(SkillRecord {
                                name: Some(
                                    url.split('/').next_back().unwrap_or("imported").to_string(),
                                ),
                                description: Some(format!("Imported from {}", url)),
                                version: Some("1".to_string()),
                                enabled: Some(true),
                                imported_at: Some(
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
                                        .as_secs(),
                                ),
                            });
                            self.import_url.clear();
                            self.show_import = false;
                            self.success = i18n.t("skills.import.success").to_string();
                        }
                    }
                });
            });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // Skills list / empty state
        if self.skills.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(i18n.t("skills.none"));
            });
            return;
        }

        for skill in &self.skills {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let name = skill.name.as_deref().unwrap_or("unnamed");
                    ui.colored_label(egui::Color32::from_rgb(100, 150, 255), name);
                    if let Some(enabled) = skill.enabled {
                        let (color, label) = if enabled {
                            (egui::Color32::GREEN, "●")
                        } else {
                            (egui::Color32::GRAY, "○")
                        };
                        ui.colored_label(color, label);
                    }
                });
                if let Some(desc) = &skill.description {
                    ui.label(desc);
                }
            });
            ui.add_space(4.0);
        }
    }
}
