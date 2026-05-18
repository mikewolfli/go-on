use crate::backend::BackendClient;
use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;
use std::sync::mpsc;

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
    remote_models: std::collections::HashMap<String, Vec<String>>,
    models_loaded: bool,
    provider_names: Vec<String>,
    catalog_loaded: bool,
    pending_rx: mpsc::Receiver<std::collections::HashMap<String, Vec<String>>>,
    pending_tx: mpsc::SyncSender<std::collections::HashMap<String, Vec<String>>>,
    catalog_rx: mpsc::Receiver<Vec<String>>,
    catalog_tx: mpsc::SyncSender<Vec<String>>,
}

impl SetupView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::sync_channel(1);
        let (catalog_tx, catalog_rx) = mpsc::sync_channel(1);
        Self {
            selected_provider: "openai".to_string(),
            api_key: String::new(),
            selected_model: "auto".to_string(),
            error_msg: String::new(),
            success_msg: String::new(),
            remote_models: std::collections::HashMap::new(),
            models_loaded: false,
            provider_names: crate::views::providers::PROVIDER_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            catalog_loaded: false,
            pending_rx,
            pending_tx,
            catalog_rx,
            catalog_tx,
        }
    }

    fn ensure_models_loaded(&mut self, backend: &BackendClient, ctx: &egui::Context) {
        if self.models_loaded {
            return;
        }
        self.models_loaded = true;
        let backend_clone = backend.clone();
        let tx = self.pending_tx.clone();
        let ctx_clone = ctx.clone();
        tokio::spawn(async move {
            let models = match tokio::time::timeout(
                std::time::Duration::from_secs(3),
                backend_clone.fetch_models(),
            )
            .await
            {
                Ok(m) => m,
                Err(_) => std::collections::HashMap::new(),
            };
            let _ = tx.try_send(models);
            ctx_clone.request_repaint();
        });

        if !self.catalog_loaded {
            self.catalog_loaded = true;
            let backend_clone = backend.clone();
            let tx = self.catalog_tx.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                let names = match tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    backend_clone.provider_catalog(),
                )
                .await
                {
                    Ok(Ok(value)) => value
                        .get("catalog")
                        .and_then(serde_json::Value::as_array)
                        .map(|items| {
                            let mut names = items
                                .iter()
                                .filter_map(|item| {
                                    item.get("name").and_then(serde_json::Value::as_str)
                                })
                                .map(ToString::to_string)
                                .collect::<Vec<_>>();
                            names.sort();
                            names.dedup();
                            names
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let _ = tx.try_send(names);
                ctx_clone.request_repaint();
            });
        }
    }

    fn available_models_for_selected_provider(&self) -> Vec<String> {
        let mut models = Vec::<String>::new();
        models.push("auto".to_string());

        if let Some(remote) = self.remote_models.iter().find_map(|(name, models)| {
            if name.eq_ignore_ascii_case(&self.selected_provider) {
                Some(models.clone())
            } else {
                None
            }
        }) {
            for model in remote {
                if !model.trim().is_empty() && model != "auto" {
                    models.push(model);
                }
            }
        }

        for fallback in crate::views::providers::models_for_provider(&self.selected_provider) {
            if *fallback != "auto" {
                models.push((*fallback).to_string());
            }
        }

        let mut deduped = Vec::<String>::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for model in models {
            if seen.insert(model.clone()) {
                deduped.push(model);
            }
        }
        deduped
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        i18n: &I18n,
        config: &mut AppConfig,
        backend: &BackendClient,
    ) -> bool {
        let mut done = false;

        self.ensure_models_loaded(backend, ctx);
        if let Ok(models) = self.pending_rx.try_recv() {
            self.remote_models = models;
        }
        if let Ok(names) = self.catalog_rx.try_recv() {
            if !names.is_empty() {
                self.provider_names = names;
                if !self
                    .provider_names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&self.selected_provider))
                {
                    self.selected_provider = self.provider_names[0].clone();
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
                    ui.colored_label(egui::Color32::from_rgb(20, 120, 70), &self.success_msg);
                    ui.add_space(10.0);
                    if ui.button(i18n.t("app.start")).clicked() {
                        done = true;
                    }
                    return;
                }

                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.provider"));
                    let provider_options = self.provider_names.clone();
                    egui::ComboBox::from_id_salt("provider_sel")
                        .selected_text(provider_label(i18n, &self.selected_provider))
                        .show_ui(ui, |ui| {
                            for p in &provider_options {
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
                            let models = self.available_models_for_selected_provider();
                            for m in models {
                                ui.selectable_value(&mut self.selected_model, m.clone(), m);
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
                                label: String::new(),
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
