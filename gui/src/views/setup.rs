use crate::backend::BackendClient;
use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;
use std::sync::mpsc;

pub struct SetupView {
    selected_provider: String,
    api_key: String,
    selected_model: String,
    validating: bool,
    error_msg: String,
    success_msg: String,
    configure_done: bool,
    backend_attempted: bool,
    attempt_configure: bool,
    /// Channel for async backend configuration completion
    configure_rx: mpsc::Receiver<bool>,
    configure_tx: mpsc::Sender<bool>,
}

impl SetupView {
    pub fn new() -> Self {
        let (configure_tx, configure_rx) = mpsc::channel();
        Self {
            selected_provider: "openai".to_string(),
            api_key: String::new(),
            selected_model: "auto".to_string(),
            validating: false,
            error_msg: String::new(),
            success_msg: String::new(),
            configure_done: false,
            backend_attempted: false,
            attempt_configure: false,
            configure_rx,
            configure_tx,
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        i18n: &I18n,
        config: &mut AppConfig,
        backend: &BackendClient,
    ) -> bool {
        let mut done = false;

        // ── After validation, attempt to send API key to backend ─────
        if self.attempt_configure && !self.backend_attempted {
            self.backend_attempted = true;
            let backend_clone = backend.clone();
            let provider_name = self.selected_provider.clone();
            let api_key = self.api_key.clone();
            let model = self.selected_model.clone();
            let configure_tx = self.configure_tx.clone();
            tokio::spawn(async move {
                // Try to configure the provider on the backend
                match backend_clone
                    .configure_provider(&provider_name, &api_key, &model)
                    .await
                {
                    Ok(_) => {
                        // Optionally try to restart the backend to pick up env vars
                        let _ = backend_clone.restart_backend().await;
                        // The backend now knows about the provider
                    }
                    Err(e) => {
                        // Backend may not be running – that's OK, the config is saved locally
                        eprintln!("Backend not reachable for provider config: {}", e);
                    }
                }
                // Signal completion back to the main thread
                let _ = configure_tx.send(true);
            });
        }

        // Check if the async configure task has completed (signaled via channel)
        if self.backend_attempted && !self.configure_done && self.configure_rx.try_recv().is_ok() {
            self.configure_done = true;
            self.success_msg = i18n.t("setup.success").to_string();
            self.validating = false;
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
                        let api_key = self.api_key.trim().to_string();
                        // Persist provider to local config
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

                        // Trigger async backend configuration
                        self.validating = true;
                        self.attempt_configure = true;
                        self.backend_attempted = false;
                        self.configure_done = false;
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
