use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;

fn provider_label(i18n: &I18n, provider: &str) -> String {
    let key = format!("provider.{}", provider.to_lowercase());
    let label = i18n.t(&key);
    if label.as_ref() == key {
        provider.to_string()
    } else {
        label.into_owned()
    }
}

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
                    ui.colored_label(egui::Color32::from_rgb(20, 120, 70), &self.success_msg);
                    ui.add_space(10.0);
                    if ui.button(i18n.t("app.start")).clicked() {
                        done = true;
                    }
                    return;
                }

                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.provider"));
                    egui::ComboBox::from_id_salt("provider_sel")
                        .selected_text(provider_label(i18n, &self.selected_provider))
                        .show_ui(ui, |ui| {
                            for p in crate::views::providers::PROVIDER_NAMES {
                                ui.selectable_value(
                                    &mut self.selected_provider,
                                    p.to_string(),
                                    provider_label(i18n, p),
                                );
                            }
                        });
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.apiKey"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.api_key)
                            .password(true)
                            .hint_text(i18n.t("common.apiKeyPlaceholder"))
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

                if config.features.setup_enterprise {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("setup.environment"));
                        egui::ComboBox::from_id_salt("setup_environment")
                            .selected_text(config.enterprise.active_environment.clone())
                            .show_ui(ui, |ui| {
                                for env in &config.enterprise.environments {
                                    if ui
                                        .selectable_label(
                                            config.enterprise.active_environment == env.name,
                                            &env.name,
                                        )
                                        .clicked()
                                    {
                                        config.enterprise.active_environment = env.name.clone();
                                        config.backend_url = env.backend_url.clone();
                                    }
                                }
                            });
                    });

                    ui.horizontal(|ui| {
                        ui.label(i18n.t("setup.secretSource"));
                        egui::ComboBox::from_id_salt("setup_secret_source")
                            .selected_text(config.enterprise.secret_source.clone())
                            .show_ui(ui, |ui| {
                                for source in ["keyring", "env", "file", "auto"] {
                                    if ui
                                        .selectable_label(
                                            config.enterprise.secret_source == source,
                                            source,
                                        )
                                        .clicked()
                                    {
                                        config.enterprise.secret_source = source.to_string();
                                    }
                                }
                            });
                    });
                }

                ui.add_space(10.0);

                if !self.error_msg.is_empty() {
                    let text = self.error_msg.clone();
                    let resp = ui.colored_label(egui::Color32::RED, &text);
                    resp.context_menu(|ui| {
                        if ui.button(i18n.t("common.copyButton")).clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close_menu();
                        }
                    });
                }

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.api_key.is_empty(),
                            egui::Button::new(i18n.t("setup.save")),
                        )
                        .clicked()
                    {
                        let api_key: String = self
                            .api_key
                            .chars()
                            .filter(|c| !c.is_control() || *c == '\t')
                            .collect::<String>()
                            .trim()
                            .to_string();
                        let provider_lower = self.selected_provider.to_lowercase();

                        // Store to system keyring (best-effort, may fail on some platforms)
                        if let Err(e) =
                            crate::keyring_util::store_api_key(&provider_lower, &api_key)
                        {
                            eprintln!(
                                "keyring: failed to store key for '{}': {}",
                                provider_lower, e
                            );
                        }

                        // Persist to config (always works)
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
                        if !save_app_config(config) {
                            self.error_msg = i18n.t("setup.saveError").to_string();
                            ctx.request_repaint();
                            return;
                        }

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
