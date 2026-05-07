use crate::backend::BackendClient;
use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::views::security_prefs;
use std::sync::mpsc;

pub struct ProvidersView {
    new_name: String,
    new_key: String,
    new_model: String,
    status: String,
    sending: bool,
    pending_delete_confirmation: Option<usize>,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::Sender<String>,
}

impl ProvidersView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::channel();
        Self {
            new_name: String::new(),
            new_key: String::new(),
            new_model: "auto".to_string(),
            status: String::new(),
            sending: false,
            pending_delete_confirmation: None,
            pending_rx,
            pending_tx,
        }
    }

    fn process_pending(&mut self) {
        while let Ok(msg) = self.pending_rx.try_recv() {
            self.sending = false;
            self.status = msg;
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut AppConfig,
        backend: &BackendClient,
        ctx: &egui::Context,
    ) {
        self.process_pending();
        let mut changed = false;
        let security = security_prefs::load();

        ui.heading("Providers");
        ui.label("Manage provider credentials and push selected provider to backend runtime.");
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.new_name);
            ui.label("API Key");
            ui.add(egui::TextEdit::singleline(&mut self.new_key).password(true));
            ui.label("Model");
            ui.text_edit_singleline(&mut self.new_model);
            if ui.button("Add / Update").clicked() {
                let name = self.new_name.trim().to_string();
                let key = self.new_key.trim().to_string();
                let model = self.new_model.trim().to_string();
                if !name.is_empty() && !key.is_empty() {
                    if let Some(existing) = config.providers.iter_mut().find(|p| p.name == name) {
                        existing.api_key = key;
                        existing.model = model;
                        existing.validated = true;
                    } else {
                        config.providers.push(ProviderConfig {
                            name,
                            api_key: key,
                            model,
                            validated: true,
                        });
                    }
                    changed = true;
                    self.status = "Provider saved.".to_string();
                    self.new_key.clear();
                }
            }
        });

        ui.add_space(8.0);
        let mut remove_idx = None;
        for (idx, provider) in config.providers.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(&provider.name);
                    let key_preview = if security.redact_api_keys_in_ui {
                        "********"
                    } else if provider.api_key.len() > 8 {
                        &provider.api_key[..8]
                    } else {
                        &provider.api_key
                    };
                    ui.label(format!("Key: {key_preview}"));
                    ui.label("Model:");
                    if ui.text_edit_singleline(&mut provider.model).changed() {
                        changed = true;
                    }
                    if ui.checkbox(&mut provider.validated, "Validated").changed() {
                        changed = true;
                    }
                    if ui.button("Push to Backend").clicked() && !self.sending {
                        self.sending = true;
                        self.status.clear();
                        let tx = self.pending_tx.clone();
                        let backend_clone = backend.clone();
                        let name = provider.name.clone();
                        let key = provider.api_key.clone();
                        let model = provider.model.clone();
                        let ctx_clone = ctx.clone();
                        tokio::spawn(async move {
                            let msg =
                                match backend_clone.configure_provider(&name, &key, &model).await {
                                    Ok(_) => format!("Provider {name} configured on backend."),
                                    Err(e) => format!("Provider push failed: {e}"),
                                };
                            let _ = tx.send(msg);
                            ctx_clone.request_repaint();
                        });
                    }
                    let delete_label = if self.pending_delete_confirmation == Some(idx) {
                        "Confirm Delete"
                    } else {
                        "Delete"
                    };
                    if ui.button(delete_label).clicked() {
                        if security.confirm_dangerous_actions
                            && self.pending_delete_confirmation != Some(idx)
                        {
                            self.pending_delete_confirmation = Some(idx);
                            self.status = format!("Click delete again to remove {}.", provider.name);
                        } else {
                            remove_idx = Some(idx);
                            self.pending_delete_confirmation = None;
                        }
                    }
                });
            });
            ui.add_space(4.0);
        }

        if let Some(idx) = remove_idx {
            config.providers.remove(idx);
            changed = true;
            self.status = "Provider removed.".to_string();
        } else if self.pending_delete_confirmation.is_some()
            && !config.providers.is_empty()
            && self.pending_delete_confirmation.unwrap_or(0) >= config.providers.len()
        {
            self.pending_delete_confirmation = None;
        }

        if changed {
            save_app_config(config);
        }

        if !self.status.is_empty() {
            ui.label(&self.status);
        }
    }
}
