use crate::config::{save_app_config, AppConfig};
use crate::i18n::I18n;
use crate::keyring_util::REDACTED_API_KEY;
use serde_json::Value;

const MAX_SNAPSHOTS: usize = 20;

pub struct ConfigEditorView {
    pub draft: String,
    status: String,
    initialized: bool,
    pub snapshots: Vec<String>,
    pub search_query: String,
    is_valid_json: bool,
    json_parse_error: String,
    pub applied: bool,
    draft_len_at_validation: usize,
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
            draft_len_at_validation: 0,
        }
    }

    /// Redact all `api_key` fields in a JSON string (recursive, handles any nesting).
    fn redact_api_keys_in_json(json: &str) -> String {
        fn redact_value(v: &mut Value) {
            match v {
                Value::Object(map) => {
                    if let Some(api_key) = map.get_mut("api_key") {
                        if let Some(key) = api_key.as_str() {
                            if !key.is_empty() {
                                *api_key = Value::String(if key.len() > 8 {
                                    format!("{}...{}", &key[..4], &key[key.len() - 4..])
                                } else {
                                    REDACTED_API_KEY.to_string()
                                });
                            }
                        }
                    }
                    // Also redact nested api_key fields (tools, agents, etc.)
                    for (_, val) in map.iter_mut() {
                        redact_value(val);
                    }
                }
                Value::Array(arr) => {
                    for item in arr.iter_mut() {
                        redact_value(item);
                    }
                }
                _ => {}
            }
        }

        match serde_json::from_str::<Value>(json) {
            Ok(mut root) => {
                redact_value(&mut root);
                serde_json::to_string_pretty(&root).unwrap_or_default()
            }
            _ => json.to_string(),
        }
    }

    /// Before applying redacted JSON, restore real api_key values from the live config.
    /// This prevents the redacted "sk-bc12...ef56" strings from overwriting real keys.
    fn restore_api_keys_in_draft(draft: &str, config: &AppConfig) -> String {
        match serde_json::from_str::<Value>(draft) {
            Ok(Value::Object(mut root)) => {
                // Restore api_key in providers array
                if let Some(providers) = root.get_mut("providers").and_then(|p| p.as_array_mut()) {
                    for p in providers.iter_mut() {
                        if let Some(obj) = p.as_object_mut() {
                            if let Some(redacted) = obj.get("api_key").and_then(|v| v.as_str()) {
                                if redacted.contains("...") || redacted == REDACTED_API_KEY {
                                    // Find matching provider in real config by name (and label, if present)
                                    let name =
                                        obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    let label =
                                        obj.get("label").and_then(|v| v.as_str()).unwrap_or("");
                                    if let Some(real) = config
                                        .providers
                                        .iter()
                                        .find(|p| p.name == name && p.label.as_str() == label)
                                    {
                                        if !real.api_key.is_empty() {
                                            obj["api_key"] = Value::String(real.api_key.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                serde_json::to_string_pretty(&Value::Object(root))
                    .unwrap_or_else(|_| draft.to_string())
            }
            _ => draft.to_string(),
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
                    let raw = serde_json::to_string_pretty(config).unwrap_or_default();
                    self.draft = Self::redact_api_keys_in_json(&raw);
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
                        ui.label(format!("{} {}", count, i18n.t("configEditor.matches")));
                    }
                });

                if !safe_mode_enabled {
                    ui.label(i18n.t("config.safeModeHidden"));
                    ui.add_space(6.0);
                }

                let edit_response = ui.add(
                    egui::TextEdit::multiline(&mut self.draft)
                        .desired_rows(12)
                        .desired_width(f32::INFINITY),
                );

                // Live JSON validation — only re-parse when the draft changes
                if edit_response.changed() || self.draft.len() != self.draft_len_at_validation {
                    self.draft_len_at_validation = self.draft.len();
                    match serde_json::from_str::<serde_json::Value>(&self.draft) {
                        Ok(_) => {
                            self.is_valid_json = true;
                            self.json_parse_error.clear();
                        }
                        Err(e) => {
                            self.is_valid_json = false;
                            self.json_parse_error = format!("⚠ {}", e);
                        }
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
                        let raw = serde_json::to_string_pretty(config).unwrap_or_default();
                        self.draft = Self::redact_api_keys_in_json(&raw);
                        self.status = i18n.t("config.reloaded").to_string();
                    }
                    if safe_mode_enabled && ui.button(i18n.t("config.createSnapshot")).clicked() {
                        if self.snapshots.len() >= MAX_SNAPSHOTS {
                            self.snapshots.remove(0);
                        }
                        self.snapshots.push(self.draft.clone());
                        self.status = format!(
                            "{} (#{}).",
                            i18n.t("config.snapshotSaved"),
                            self.snapshots.len()
                        );
                    }
                    if ui.button(i18n.t("config.applyJson")).clicked() {
                        // Restore real api_key values before parsing — the draft shows
                        // redacted "sk-bc12...ef56" strings that must not overwrite real keys.
                        let restored = Self::restore_api_keys_in_draft(&self.draft, config);
                        match serde_json::from_str::<AppConfig>(&restored) {
                            Ok(new_cfg) => {
                                if safe_mode_enabled {
                                    if self.snapshots.len() >= MAX_SNAPSHOTS {
                                        self.snapshots.remove(0);
                                    }
                                    self.snapshots.push(
                                        serde_json::to_string_pretty(config).unwrap_or_default(),
                                    );
                                }
                                *config = new_cfg;
                                save_app_config(config);
                                // Re-redact the draft so the editor continues to show redacted keys
                                self.draft = Self::redact_api_keys_in_json(
                                    &serde_json::to_string_pretty(config).unwrap_or_default(),
                                );
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
