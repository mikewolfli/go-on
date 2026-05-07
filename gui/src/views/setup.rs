use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;

pub struct SetupView {
    selected_provider: String,
    api_key: String,
    selected_model: String,
    validating: bool,
    error_msg: String,
    success_msg: String,
    validate_start: Option<std::time::Instant>,
}

const VALIDATE_DELAY_MS: u128 = 500;

impl SetupView {
    pub fn new() -> Self {
        Self {
            selected_provider: "openai".to_string(),
            api_key: String::new(),
            selected_model: "auto".to_string(),
            validating: false,
            error_msg: String::new(),
            success_msg: String::new(),
            validate_start: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, i18n: &I18n, config: &mut AppConfig) -> bool {
        let mut done = false;

        // ── Non-blocking validation timer ──────────────────────
        if self.validating {
            if let Some(start) = self.validate_start {
                if start.elapsed().as_millis() >= VALIDATE_DELAY_MS {
                    // Simulated validation passed – persist provider
                    config.providers.push(ProviderConfig {
                        name: self.selected_provider.clone(),
                        api_key: self.api_key.clone(),
                        model: self.selected_model.clone(),
                        validated: true,
                    });
                    save_app_config(config);
                    self.success_msg = format!(
                        "{} {} {}",
                        i18n.t("setup.success"),
                        self.selected_provider,
                        i18n.t("toast.serviceRestarted"),
                    );
                    self.validating = false;
                    self.validate_start = None;
                } else {
                    ctx.request_repaint();
                }
            }
        }

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
                        self.validate_start = Some(std::time::Instant::now());
                        self.error_msg.clear();
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
