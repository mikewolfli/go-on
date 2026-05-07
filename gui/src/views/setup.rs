use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;

pub struct SetupView {
    selected_provider: String,
    api_key: String,
    selected_model: String,
    error_msg: String,
    success_msg: String,
}

impl SetupView {
    pub fn new() -> Self {
        Self {
            selected_provider: "openai".to_string(),
            api_key: String::new(),
            selected_model: "auto".to_string(),
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
                    egui::ComboBox::from_id_salt("provider_sel")
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
                    egui::ComboBox::from_id_salt("model_sel")
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
                    if ui
                        .add_enabled(
                            !self.api_key.is_empty(),
                            egui::Button::new(i18n.t("setup.save")),
                        )
                        .clicked()
                    {
                        let api_key = self.api_key.trim().to_string();
                        let provider_lower = self.selected_provider.to_lowercase();

                        // Store in system keyring (shared with backend)
                        match crate::keyring_util::store_api_key(&provider_lower, &api_key) {
                            Ok(_) => {
                                eprintln!(
                                    "API key for '{}' saved to system keyring",
                                    provider_lower
                                );
                            }
                            Err(e) => {
                                self.error_msg = format!("保存到系统 keyring 失败: {}", e);
                                return;
                            }
                        }

                        // Persist provider info to local config
                        if let Some(existing) = config
                            .providers
                            .iter_mut()
                            .find(|p| p.name == self.selected_provider)
                        {
                            existing.api_key = api_key.clone();
                            existing.model = self.selected_model.clone();
                            existing.validated = true;
                        } else {
                            config.providers.push(ProviderConfig {
                                name: self.selected_provider.clone(),
                                api_key: api_key.clone(),
                                model: self.selected_model.clone(),
                                validated: true,
                            });
                        }
                        save_app_config(config);

                        self.api_key = api_key;
                        self.error_msg.clear();
                        self.success_msg = i18n.t("setup.success").to_string();
                        ctx.request_repaint();
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
