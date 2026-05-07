use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;

pub struct SetupView {
    pub visible: bool,
    selected_provider: String,
    api_key: String,
    selected_model: String,
    validating: bool,
    error_msg: String,
    success_msg: String,
}

impl SetupView {
    pub fn new() -> Self {
        Self {
            visible: true,
            selected_provider: "openai".to_string(),
            api_key: String::new(),
            selected_model: "auto".to_string(),
            validating: false,
            error_msg: String::new(),
            success_msg: String::new(),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, i18n: &I18n, config: &mut AppConfig) -> bool {
        let mut done = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading(i18n.t("setup.title"));
                ui.add_space(10.0);
                ui.label(i18n.t("setup.hint"));
                ui.add_space(20.0);

                if !self.success_msg.is_empty() {
                    ui.colored_label(egui::Color32::GREEN, &self.success_msg);
                    ui.add_space(10.0);
                    if ui.button(i18n.t("app.start")).clicked() {
                        done = true;
                    }
                    return;
                }

                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.provider"));
                    egui::ComboBox::from_id_source("provider_sel")
                        .selected_text(&self.selected_provider)
                        .show_ui(ui, |ui| {
                            let providers = [
                                "openai",
                                "anthropic",
                                "deepseek",
                                "qwen",
                                "gemini",
                                "copilot",
                                "mistral",
                                "groq",
                            ];
                            for p in &providers {
                                ui.selectable_value(&mut self.selected_provider, p.to_string(), *p);
                            }
                        });
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.apiKey"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.api_key)
                            .password(true)
                            .hint_text("sk-...")
                            .desired_width(300.0),
                    );
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.model"));
                    egui::ComboBox::from_id_source("model_sel")
                        .selected_text(&self.selected_model)
                        .show_ui(ui, |ui| {
                            let models = [
                                "auto",
                                "gpt-4o",
                                "gpt-4o-mini",
                                "claude-sonnet-4-20250514",
                                "deepseek-chat",
                            ];
                            for m in &models {
                                ui.selectable_value(&mut self.selected_model, m.to_string(), *m);
                            }
                        });
                });

                ui.add_space(10.0);

                if !self.error_msg.is_empty() {
                    ui.colored_label(egui::Color32::RED, &self.error_msg);
                }

                ui.horizontal(|ui| {
                    let btn_label = if self.validating {
                        format!("{}...", i18n.t("setup.validating"))
                    } else {
                        i18n.t("setup.save").to_string()
                    };
                    if ui
                        .add_enabled(
                            !self.validating && !self.api_key.is_empty(),
                            egui::Button::new(btn_label),
                        )
                        .clicked()
                    {
                        self.validating = true;
                        self.error_msg.clear();
                        config.providers.push(ProviderConfig {
                            name: self.selected_provider.clone(),
                            api_key: self.api_key.clone(),
                            model: self.selected_model.clone(),
                            validated: true,
                        });
                        save_app_config(config);
                        self.success_msg = i18n.t("setup.success").to_string();
                        self.validating = false;
                    }

                    if ui.button(i18n.t("setup.skip")).clicked() {
                        done = true;
                    }
                });
            });
        });
        done
    }
}
