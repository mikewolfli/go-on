use crate::backend::BackendClient;
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
            show_create: false,
            create_name: String::new(),
            create_desc: String::new(),
            create_prompt: String::new(),
            create_input_schema: r#"{"query": "string"}"#.to_string(),
            import_url: String::new(),
            show_import: false,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        _backend: &BackendClient,
        _ctx: &egui::Context,
    ) {
        ui.heading(i18n.t("tab.skills"));
        ui.separator();
        ui.add_space(4.0);

        // Action buttons
        ui.horizontal(|ui| {
            if ui.button("➕").clicked() {
                self.show_create = !self.show_create;
            }
            if ui.button("📥").clicked() {
                self.show_import = !self.show_import;
            }
            if ui.button("🔄").clicked() { /* refresh */ }
        });

        // Create dialog
        if self.show_create {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label("Create New Skill");
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.create_name);
                });
                ui.horizontal(|ui| {
                    ui.label("Desc:");
                    ui.text_edit_singleline(&mut self.create_desc);
                });
                ui.label("Prompt Template:");
                ui.text_edit_multiline(&mut self.create_prompt);
                ui.label("Input Schema (JSON):");
                ui.text_edit_multiline(&mut self.create_input_schema);
                if ui.button("Save Skill").clicked() {
                    self.skills.push(SkillRecord {
                        name: Some(self.create_name.clone()),
                        description: Some(self.create_desc.clone()),
                        version: Some("1".to_string()),
                        enabled: Some(true),
                        imported_at: Some(
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs(),
                        ),
                    });
                    self.create_name.clear();
                    self.create_desc.clear();
                    self.create_prompt.clear();
                    self.show_create = false;
                }
            });
        }

        // Import dialog
        if self.show_import {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label("Import Skill from URL");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.import_url);
                    if ui.button("Import").clicked() {
                        self.import_url.clear();
                        self.show_import = false;
                    }
                });
            });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // Skills list
        if self.skills.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(i18n.t("chat.noMessages"));
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
