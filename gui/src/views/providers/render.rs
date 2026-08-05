//! Rendering logic for the providers view — extracted from `mod.rs`.
//!
//! This module contains the `show()` method of `ProvidersView`, which draws
//! the entire providers management UI including add/edit/delete, key management,
//! Copilot OAuth device flow, and provider capabilities display.

use super::*;

impl ProvidersView {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        config: &mut AppConfig,
        backend: &BackendClient,
        ctx: &egui::Context,
        ops_enabled: bool,
    ) -> bool {
        let mut changed = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.process_pending(i18n, config);
                self.ensure_models_loaded(backend, ctx);
                self.refresh_security_cache();

                // Copy needed bools to avoid holding a reference to self over closures.
                let redact_keys = self.cached_security.redact_api_keys_in_ui;
                let confirm_dangerous = self.cached_security.confirm_dangerous_actions;

                let _resp = egui::Frame::NONE.show(ui, |ui| {
                        ui.heading(i18n.t("providers.title"));
                        ui.separator();
                        ui.add_space(8.0);

                        // ── Add new provider section ──────────────────────────────────
                        // Basic key CRUD is always available (not gated by ops_enabled).
                        // ops_enabled only controls advanced operations like test/capabilities.
                        {
                    ui.label(i18n.t("providers.add_new"));
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("providers.provider"));
                        let provider_options = self.provider_names.clone();
                        egui::ComboBox::from_id_salt("add_provider_sel")
                            .selected_text(provider_label(i18n, &self.selected_provider))
                            .show_ui(ui, |ui| {
                                for p in &provider_options {
                                    if ui
                                        .selectable_value(
                                            &mut self.selected_provider,
                                            p.to_string(),
                                            provider_label(i18n, p),
                                        )
                                        .clicked()
                                    {
                                        self.new_key.clear();
                                        self.new_secret_key.clear();
                                        self.new_model = "auto".to_string();
                                        self.copilot_token_stored = false;
                                        self.copilot_device_state = None;
                                        self.copilot_status.clear();
                                    }
                                }
                            });
                        ui.label(i18n.t("providers.api_key"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_key)
                                .password(true)
                                .hint_text(i18n.t("common.apiKeyPlaceholder"))
                                .desired_width(260.0),
                        );
                        // ── Secret key field (dual-auth providers) ──
                        if provider_requires_secret(&self.selected_provider) {
                            ui.label(i18n.t("providers.secret_key"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_secret_key)
                                    .password(true)
                                    .hint_text(i18n.t("providers.secret_key_placeholder"))
                                    .desired_width(260.0),
                            );
                        }
                        // Show auto-push hint when updating an existing provider
                        if self.update_target >= 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(100, 180, 100),
                                i18n.t("providers.auto_push_hint")
                            );
                        }
                        // Label field: required when adding a second instance of the same provider
                        ui.label(i18n.t("providers.label"));
                        let existing_same = config.providers.iter().filter(|p| p.name == self.selected_provider).count();
                        if existing_same > 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 160, 50),
                                i18n.t("providers.labelRequiredHint"),
                            );
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_label)
                                .hint_text(if existing_same > 0 { i18n.t("providers.labelPlaceholderRequired") } else { i18n.t("providers.labelPlaceholder") })
                                .desired_width(120.0),
                        );
                        ui.label(i18n.t("providers.model"));
                        egui::ComboBox::from_id_salt("add_model_sel")
                            .selected_text({
                                if self.new_model == "auto" {
                                    i18n.t("providers.auto").to_string()
                                } else {
                                    format!(
                                        "{}: {}",
                                        provider_label(i18n, &self.selected_provider),
                                        self.new_model
                                    )
                                }
                            })
                            .show_ui(ui, |ui| {
                                // Show hint for copilot
                                if self.selected_provider.to_lowercase() == "copilot" {
                                    ui.label(i18n.t("providers.copilot_hint"));
                                }
                                let models = self.available_models_for_provider(&self.selected_provider);
                                for m in models {
                                    let display_name = if m == "auto" {
                                        i18n.t("providers.auto").to_string()
                                    } else {
                                        format!(
                                            "{}: {}",
                                            provider_label(i18n, &self.selected_provider),
                                            m
                                        )
                                    };
                                    ui.selectable_value(
                                        &mut self.new_model,
                                        m,
                                        display_name,
                                    );
                                }
                            });
                        // ── Copilot Device Code authorization ──
                        if self.selected_provider.to_lowercase() == "copilot" {
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(2.0);

                            if self.copilot_device_state.is_none()
                                && ui.button(i18n.t("providers.copilot_authorize")).clicked()
                            {
                                self.copilot_device_state = Some("requesting".to_string());
                                self.copilot_status = i18n.t("providers.copilot_requesting").to_string();
                                let backend_clone = backend.clone();
                                let tx = self.pending_tx.clone();
                                let ctx_clone = ctx.clone();
                                tokio::spawn(async move {
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(15),
                                        backend_clone.copilot_device_code(),
                                    )
                                    .await
                                    {
                                        Ok(Ok(body)) => {
                                            let msg = format!("__copilot_device__:{}", serde_json::to_string(&body).unwrap_or_default());
                                            if let Err(e) = tx.try_send(msg) {
                                                eprintln!("WARN: providers try_send failed: {:?}", e);
                                            }
                                        }
                                        Ok(Err(e)) => {
                                            let msg = format!("__copilot_device_err__:{}", e);
                                            if let Err(e) = tx.try_send(msg) {
                                                eprintln!("WARN: providers try_send failed: {:?}", e);
                                            }
                                        }
                                        Err(_) => {
                                            let msg = "__copilot_device_err__:Request timed out.".to_string();
                                            if let Err(e) = tx.try_send(msg) {
                                                eprintln!("WARN: providers try_send failed: {:?}", e);
                                            }
                                        }
                                    }
                                    ctx_clone.request_repaint();
                                });
                            }

                            if let Some(state) = self.copilot_device_state.clone() {
                                let mut open = true;
                                egui::Window::new(i18n.t("providers.copilot_authorize"))
                                    .id(egui::Id::new("copilot_device_auth"))
                                    .open(&mut open)
                                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                                    .resizable(false)
                                    .collapsible(false)
                                    .show(ui.ctx(), |ui| {
                                        egui::Frame::NONE.show(ui, |ui| {
                                            match state.as_str() {
                                                "requesting" => {
                                                    ui.horizontal(|ui| {
                                                        ui.spinner();
                                                        ui.label(i18n.t("providers.copilot_requesting"));
                                                    });
                                                }
                                                "polling" => {
                                                    ui.vertical(|ui| {
                                                        ui.heading(i18n.t("providers.copilot_authorize"));
                                                        ui.add_space(8.0);
                                                        ui.label(i18n.t("providers.copilot_open_url"));
                                                        if ui.link(&self.copilot_verification_uri).clicked() {
                                                            let _ = webbrowser::open(&self.copilot_verification_uri);
                                                        }
                                                        ui.add_space(4.0);
                                                        ui.horizontal(|ui| {
                                                            ui.label(i18n.t("providers.copilot_enter_code"));
                                                            ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(&self.copilot_user_code)
                                                                        .size(28.0)
                                                                        .color(egui::Color32::from_rgb(60, 180, 100))
                                                                        .monospace(),
                                                                ),
                                                            );
                                                        });
                                                        ui.add_space(8.0);
                                                        ui.horizontal(|ui| {
                                                            ui.spinner();
                                                            ui.label(&self.copilot_status);
                                                        });
                                                        ui.add_space(6.0);
                                                        let last_poll_age = self.copilot_last_poll.elapsed().as_secs();
                                                        ui.small(format!(
                                                            "Debug: polls={}, interval={}s, slow_downs={}, last_poll={}s ago",
                                                            self.copilot_poll_attempts,
                                                            self.copilot_poll_interval,
                                                            self.copilot_slow_down_count,
                                                            last_poll_age
                                                        ));
                                                        if !self.copilot_last_poll_result.is_empty() {
                                                            ui.small(format!(
                                                                "Debug: last_result={}",
                                                                self.copilot_last_poll_result
                                                            ));
                                                        }
                                                    });
                                                }
                                                "done" => {
                                                    ui.vertical(|ui| {
                                                        ui.colored_label(
                                                            egui::Color32::from_rgb(60, 180, 100),
                                                            i18n.t("providers.copilot_authorized"),
                                                        );
                                                        ui.add_space(4.0);
                                                        if !self.copilot_access_token.is_empty() {
                                                            let preview = if self.copilot_access_token.len() > 8 {
                                                                format!(
                                                                    "{}...{}",
                                                                    &self.copilot_access_token[..4],
                                                                    &self.copilot_access_token[self.copilot_access_token.len() - 4..]
                                                                )
                                                            } else {
                                                                "********".to_string()
                                                            };
                                                            ui.label(format!(
                                                                "{}: {}",
                                                                i18n.t("providers.tokenPreview"),
                                                                preview
                                                            ));
                                                        }
                                                        ui.add_space(8.0);
                                                        if ui.button(i18n.t("common.close")).clicked() {
                                                            self.copilot_device_state = None;
                                                        }
                                                    });
                                                }
                                                "error" => {
                                                    ui.vertical(|ui| {
                                                        ui.colored_label(
                                                            egui::Color32::from_rgb(220, 80, 80),
                                                            &self.copilot_status,
                                                        );
                                                        ui.add_space(8.0);
                                                        if ui.button(i18n.t("providers.copilot_retry")).clicked() {
                                                            self.copilot_device_state = None;
                                                            self.copilot_status.clear();
                                                        }
                                                        if ui.button(i18n.t("common.close")).clicked() {
                                                            self.copilot_device_state = None;
                                                        }
                                                    });
                                                }
                                                _ => {}
                                            }
                                        });
                                    });
                                if !open {
                                    self.copilot_device_state = None;
                                }
                            }
                        }
                        let selected_requires_secret =
                            provider_requires_secret(&self.selected_provider.to_lowercase());
                        let can_add = if self.selected_provider.to_lowercase() == "copilot" {
                            true
                        } else if selected_requires_secret {
                            !self.new_key.trim().is_empty() && !self.new_secret_key.trim().is_empty()
                        } else {
                            !self.new_key.trim().is_empty()
                        };

                        if ui
                            .add_enabled(can_add, egui::Button::new(i18n.t("providers.add")))
                            .clicked()
                        {
                            let name = self.selected_provider.clone();
                            let key: String = self.new_key
                                .chars()
                                .filter(|c| !c.is_control() || *c == '\t')
                                .collect::<String>()
                                .trim()
                                .to_string();
                            let secret_key: String = self.new_secret_key
                                .chars()
                                .filter(|c| !c.is_control() || *c == '\t')
                                .collect::<String>()
                                .trim()
                                .to_string();
                            let model = self.new_model.trim().to_string();
                            let provider_lower = name.to_lowercase();

                            if provider_requires_secret(&provider_lower) && secret_key.is_empty() {
                                self.status = i18n.t("providers.secret_key_placeholder").to_string();
                                return;
                            }

                            // Try keyring, but don't block the save if it fails
                            if let Err(e) =
                                crate::keyring_util::store_api_key(&provider_lower, &key)
                            {
                                eprintln!(
                                    "Warning: failed to store API key in system keyring: {}",
                                    e
                                );
                            }

                            if !secret_key.is_empty() {
                                if let Err(e) =
                                    crate::keyring_util::store_secret_key(&provider_lower, &secret_key)
                                {
                                    eprintln!(
                                        "Warning: failed to store secret key in system keyring: {}",
                                        e
                                    );
                                }
                            }

                            // Check if adding a duplicate provider name.
                            // If the user provides a label, it becomes a separate agent entry.
                            // If no label, update the existing matching provider (or the first unlabeled one).
                            let existing_count = config.providers.iter().filter(|p| p.name == name).count();
                            let label = self.new_label.trim().to_string();

                            if existing_count > 0 && label.is_empty() {
                                // No label provided, but multiple instances exist or would exist —
                                // update the first matching entry instead.
                                if !key.is_empty() {
                                    if let Some(existing) = config.providers.iter_mut().find(|p| p.name == name && p.label.is_empty()) {
                                        // Key is stored in system keyring above — no need to write to config.
                                        // The validated flag and model are still tracked in config.
                                        existing.validated = true;
                                        if !model.is_empty() && model != "auto" {
                                            existing.model = model.clone();
                                        }
                                    }
                                    save_app_config(config);
                                    changed = true;
                                    self.status = format!(
                                        "{} '{}' {}",
                                        i18n.t("providers.api_key"),
                                        provider_label(i18n, &name),
                                        i18n.t("providers.updated")
                                    );
                                    // Auto-push updated key to backend
                                    if !self.sending {
                                        self.sending = true;
                                        let tx = self.pending_tx.clone();
                                        let backend_clone = backend.clone();
                                        let push_name = name.clone();
                                        let push_key = key.clone();
                                        let push_secret_key = secret_key.clone();
                                        let push_model = model.clone();
                                        if push_model.is_empty() || push_model == "auto" {
                                            // Model will be resolved by the backend
                                        }
                                        let ctx_clone = ctx.clone();
                                        let ok_fmt = format!(
                                            "{} '{}' {}",
                                            i18n.t("providers.api_key"),
                                            provider_label(i18n, &name),
                                            i18n.t("providers.push_success")
                                        );
                                        let err_fmt = format!(
                                            "{} '{}': %s",
                                            i18n.t("providers.push_failed"),
                                            provider_label(i18n, &name)
                                        );
                                        let has_secret = !push_secret_key.is_empty();
                                        tokio::spawn(async move {
                                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                                            let result = if has_secret {
                                                tokio::time::timeout(
                                                    std::time::Duration::from_secs(10),
                                                    backend_clone.configure_provider_with_secret(&push_name, &push_key, &push_secret_key, &push_model),
                                                ).await
                                            } else {
                                                tokio::time::timeout(
                                                    std::time::Duration::from_secs(10),
                                                    backend_clone.configure_provider(&push_name, &push_key, &push_model),
                                                ).await
                                            };
                                            let msg = match result {
                                                Ok(Ok(_)) => ok_fmt,
                                                Ok(Err(e)) => err_fmt.replace("%s", &e),
                                                Err(_) => err_fmt.replace("%s", "timeout"),
                                            };
                                            if let Err(e) = tx.try_send(msg) {
                                                eprintln!("WARN: providers try_send failed: {:?}", e);
                                            }
                                            ctx_clone.request_repaint();
                                        });
                                    }
                                } else {
                                    self.status = format!(
                                        "{} '{}' {}.",
                                        i18n.t("providers.provider"),
                                        provider_label(i18n, &name),
                                        i18n.t("providers.already_exists")
                                    );
                                }
                            } else {
                                // New provider entry (possibly a labeled duplicate for multi-model)
                                let label_clean = label.replace(' ', "_");
                                config.providers.push(ProviderConfig {
                                    name: name.clone(),
                                    api_key: key.clone(),
                                    secret_key: String::new(),
                                    model: model.clone(),
                                    validated: true,
                                    label: label_clean.clone(),
                                });
                                save_app_config(config);
                                changed = true;
                                self.status = format!(
                                    "{} '{}' {}.",
                                    i18n.t("providers.provider"),
                                    provider_label(i18n, &name),
                                    i18n.t("providers.added")
                                );
                                // Auto-push new provider key to backend so it takes effect immediately.
                                if !self.sending {
                                    self.sending = true;
                                    let tx = self.pending_tx.clone();
                                    let backend_clone = backend.clone();
                                    let push_name = name.clone();
                                    let push_key = key.clone();
                                    let push_secret_key = secret_key.clone();
                                    let push_model = model.clone();
                                    let ctx_clone = ctx.clone();
                                    let ok_fmt = format!(
                                        "{} '{}' {}",
                                        i18n.t("providers.api_key"),
                                        provider_label(i18n, &name),
                                        i18n.t("providers.push_success")
                                    );
                                    let err_fmt = format!(
                                        "{} '{}': %s",
                                        i18n.t("providers.push_failed"),
                                        provider_label(i18n, &name)
                                    );
                                    let has_secret = !push_secret_key.is_empty();
                                    tokio::spawn(async move {
                                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                                        let result = if has_secret {
                                            tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone.configure_provider_with_secret(&push_name, &push_key, &push_secret_key, &push_model),
                                            ).await
                                        } else {
                                            tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone.configure_provider(&push_name, &push_key, &push_model),
                                            ).await
                                        };
                                        let msg = match result {
                                            Ok(Ok(_)) => ok_fmt,
                                            Ok(Err(e)) => err_fmt.replace("%s", &e),
                                            Err(_) => err_fmt.replace("%s", "timeout"),
                                        };
                                        if let Err(e) = tx.try_send(msg) {
                                            eprintln!("WARN: providers try_send failed: {:?}", e);
                                        }
                                        ctx_clone.request_repaint();
                                    });
                                }
                            }
                            self.new_key.clear();
                            self.new_secret_key.clear();
                        }
                    });
                }

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);

                // ── Existing providers list ────────────────────────────────────
                ui.label(i18n.t("providers.saved"));
                let mut remove_idx = None;
                for (idx, provider) in config.providers.iter_mut().enumerate() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let display_name = if provider.label.is_empty() {
                                provider_label(i18n, &provider.name).to_string()
                            } else {
                                format!("{} ({})", provider_label(i18n, &provider.name), provider.label)
                            };
                            ui.label(&display_name);
                            let cached_key = crate::keyring_util::get_api_key_with_fallback(
                                &provider.name.to_lowercase(),
                                Some(&provider.api_key),
                            )
                            .unwrap_or_default();
                            let key_preview_str = if redact_keys {
                                "********".to_string()
                            } else if cached_key.len() > 8 {
                                cached_key[..8].to_string()
                            } else if !cached_key.is_empty() {
                                cached_key.clone()
                            } else {
                                i18n.t("providers.noKey").to_string()
                            };
                            ui.label(format!(
                                "{} {}",
                                i18n.t("providers.key_preview"),
                                key_preview_str
                            ));
                            ui.label(i18n.t("providers.model"));
                            // Model dropdown for saved providers
                            let models = self.available_models_for_provider(&provider.name);
                            egui::ComboBox::from_id_salt(format!("model_{}", idx))
                                .selected_text({
                                    if provider.model == "auto" || provider.model.is_empty() {
                                        i18n.t("providers.auto").to_string()
                                    } else {
                                        format!(
                                            "{}: {}",
                                            provider_label(i18n, &provider.name),
                                            provider.model
                                        )
                                    }
                                })
                                .show_ui(ui, |ui| {
                                    for m in models {
                                        let display_name = if m == "auto" {
                                            i18n.t("providers.auto").to_string()
                                        } else {
                                            format!(
                                                "{}: {}",
                                                provider_label(i18n, &provider.name),
                                                m
                                            )
                                        };
                                        if ui
                                            .selectable_value(
                                                &mut provider.model,
                                                m.clone(),
                                                display_name,
                                            )
                                            .clicked()
                                        {
                                            changed = true;
                                            // Auto-push updated model to backend
                                            if !self.sending {
                                                self.sending = true;
                                                let tx_push = self.pending_tx.clone();
                                                let backend_push = backend.clone();
                                                let name_push = provider.name.clone();
                                                let model_push = m;
                                                let ctx_push = ctx.clone();
                                                let key_push = crate::keyring_util::get_api_key_with_fallback(
                                                    &name_push.to_lowercase(),
                                                    Some(&provider.api_key),
                                                ).unwrap_or_default();
                                                tokio::spawn(async move {
                                                    tokio::time::sleep(
                                                        std::time::Duration::from_millis(100),
                                                    )
                                                    .await;
                                                    let _ = backend_push
                                                        .configure_provider(
                                                            &name_push,
                                                            &key_push,
                                                            &model_push,
                                                        )
                                                        .await;
                                                    // Clear sending flag after push
                                                    if let Err(e) = tx_push.try_send(String::new()) {
                                                        eprintln!("WARN: providers try_send failed: {:?}", e);
                                                    }
                                                    ctx_push.request_repaint();
                                                });
                                            }
                                        }
                                    }
                                });
                            if ui
                                .checkbox(&mut provider.validated, i18n.t("providers.validated"))
                                .changed()
                            {
                                changed = true;
                            }

                            // Update button - opens inline edit for this provider
                            // Key update is a basic operation (not gated by ops_enabled).
                            if self.update_target == idx as isize {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_key)
                                        .password(true)
                                        .hint_text(i18n.t("common.apiKeyPlaceholder"))
                                        .desired_width(260.0),
                                );
                                if ui.button(i18n.t("providers.save_key")).clicked() {
                                    let new_key: String = self.new_key
                                        .chars()
                                        .filter(|c| !c.is_control() || *c == '\t')
                                        .collect::<String>()
                                        .trim()
                                        .to_string();
                                    if !new_key.is_empty() {
                                        let provider_lower = provider.name.to_lowercase();
                                        let provider_name = provider.name.clone();
                                        // Store to system keyring
                                        match crate::keyring_util::store_api_key(
                                            &provider_lower,
                                            &new_key,
                                        ) {
                                            Ok(_) => {
                                                eprintln!(
                                                    "keyring: stored key for '{}'",
                                                    provider_lower
                                                );
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "keyring: failed to store key for '{}': {}",
                                                    provider_lower, e
                                                );
                                            }
                                        }
                                        // Key is stored exclusively in system keyring — DO NOT write to config.
                                        // The config's api_key field is cleared on serialization (see save_app_config).
                                        provider.validated = true;
                                        self.status = format!(
                                            "{} '{}' {}.",
                                            i18n.t("providers.api_key"),
                                            provider_label(i18n, &provider_name),
                                            i18n.t("providers.updated")
                                        );
                                        self.new_key.clear();
                                        self.update_target = -1;
                                        changed = true;

                                        // Auto-push to backend after saving
                                        if !self.sending {
                                            self.sending = true;
                                            let tx = self.pending_tx.clone();
                                            let backend_clone = backend.clone();
                                            let name = provider_name.clone();
                                            let key = new_key;
                                            let model = provider.model.clone();
                                            let ctx_clone = ctx.clone();
                                            let ok_fmt = format!(
                                                "{} '{}' {}",
                                                i18n.t("providers.api_key"),
                                                provider_label(i18n, &name),
                                                i18n.t("providers.push_success")
                                            );
                                            let err_fmt = format!(
                                                "{} '{}': %s",
                                                i18n.t("providers.push_failed"),
                                                provider_label(i18n, &name)
                                            );
                                            tokio::spawn(async move {
                                                // Small delay to ensure keyring write completes
                                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                                                let secret = crate::keyring_util::get_secret_key(&name.to_lowercase())
                                                    .unwrap_or_default();
                                                let has_secret = !secret.is_empty();
                                                let result = if has_secret {
                                                    match tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        backend_clone.configure_provider_with_secret(&name, &key, &secret, &model),
                                                    )
                                                    .await
                                                    {
                                                        Ok(r) => r,
                                                        Err(_) => {
                                                            #[cfg(debug_assertions)]
                                                            eprintln!("Warning: auto-push configure_provider timed out");
                                                            Err("timeout".to_string())
                                                        }
                                                    }
                                                } else {
                                                    match tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        backend_clone.configure_provider(&name, &key, &model),
                                                    )
                                                    .await
                                                    {
                                                        Ok(r) => r,
                                                        Err(_) => {
                                                            #[cfg(debug_assertions)]
                                                            eprintln!("Warning: auto-push configure_provider timed out");
                                                            Err("timeout".to_string())
                                                        }
                                                    }
                                                };
                                                let msg = match result {
                                                    Ok(_) => ok_fmt,
                                                    Err(e) => err_fmt.replace("%s", &e.to_string()),
                                                };
                                                if let Err(e) = tx.try_send(msg) {
                                                    eprintln!("WARN: providers try_send failed: {:?}", e);
                                                }
                                                ctx_clone.request_repaint();
                                            });
                                        }
                                    }
                                }
                                if ui.button(i18n.t("providers.cancel")).clicked() {
                                    self.update_target = -1;
                                    self.new_key.clear();
                                }
                            } else if ui.button(i18n.t("providers.update_key")).clicked()
                            {
                                self.update_target = idx as isize;
                                self.new_key.clear();
                                self.copilot_token_stored = false;
                                self.status = format!(
                                    "{} '{}' {}.",
                                    i18n.t("providers.enter_new_key"),
                                    provider_label(i18n, &provider.name),
                                    i18n.t("providers.save_key")
                                );
                            }

                            if ops_enabled && ui.button(i18n.t("providers.push")).clicked()
                                && !self.sending
                            {
                                self.sending = true;
                                self.status.clear();
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let name = provider.name.clone();
                                // Read API key from keyring (fallback to config)
                                let key = crate::keyring_util::get_api_key_with_fallback(
                                    &name.to_lowercase(),
                                    Some(&provider.api_key),
                                )
                                .unwrap_or_default();
                                let secret = crate::keyring_util::get_secret_key(&name.to_lowercase())
                                    .unwrap_or_default();
                                let model = provider.model.clone();
                                let ctx_clone = ctx.clone();
                                let ok_fmt = format!(
                                    "{} %s {}.",
                                    i18n.t("providers.provider"),
                                    i18n.t("providers.configured")
                                );
                                let err_fmt = format!("{} %s", i18n.t("providers.push_failed"));
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let has_secret = !secret.is_empty();
                                    let result = if has_secret {
                                        match tokio::time::timeout(
                                            std::time::Duration::from_secs(10),
                                            backend_clone.configure_provider_with_secret(&name, &key, &secret, &model),
                                        )
                                        .await
                                        {
                                            Ok(r) => r,
                                            Err(_) => {
                                                #[cfg(debug_assertions)]
                                                eprintln!("Warning: configure_provider timed out");
                                                Err("timeout".to_string())
                                            }
                                        }
                                    } else {
                                        match tokio::time::timeout(
                                            std::time::Duration::from_secs(10),
                                            backend_clone.configure_provider(&name, &key, &model),
                                        )
                                        .await
                                        {
                                            Ok(r) => r,
                                            Err(_) => {
                                                #[cfg(debug_assertions)]
                                                eprintln!("Warning: configure_provider timed out");
                                                Err("timeout".to_string())
                                            }
                                        }
                                    };
                                    let msg = match result {
                                        Ok(_) => ok_fmt.replace("%s", &name),
                                        Err(e) => err_fmt.replace("%s", &e.to_string()),
                                    };
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!("WARN: providers try_send failed: {:?}", e);
                                    }
                                    ctx_clone.request_repaint();
                                });
                            }
                            if ops_enabled
                                && ui.button(i18n.t("providers.ops.testConn")).clicked()
                            {
                                let name = provider.name.clone();
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let ctx_clone = ctx.clone();
                                let ok_tpl = i18n.t("providers.ops.connStatus").to_string();
                                let err_tpl = i18n.t("providers.ops.connStatusFailed").to_string();
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.provider_test_connection(&name),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!(
                                                "Warning: provider_test_connection timed out"
                                            );
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let msg = match result {
                                        Ok(v) => {
                                            let ok = v
                                                .get("ok")
                                                .and_then(serde_json::Value::as_bool)
                                                .unwrap_or(false);
                                            let latency = v
                                                .get("latency_ms")
                                                .and_then(serde_json::Value::as_u64)
                                                .unwrap_or(0);
                                            format!(
                                                "__ops__:{}:{}",
                                                name,
                                                ok_tpl
                                                    .replace("{ok}", &ok.to_string())
                                                    .replace("{latency}", &latency.to_string())
                                            )
                                        }
                                        Err(e) => format!(
                                            "__ops__:{}:{}",
                                            name,
                                            err_tpl.replace("{error}", &e.to_string())
                                        ),
                                    };
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!("WARN: providers try_send failed: {:?}", e);
                                    }
                                    ctx_clone.request_repaint();
                                });
                            }
                            if ops_enabled
                                && ui.button(i18n.t("providers.ops.testCompletion")).clicked()
                            {
                                let name = provider.name.clone();
                                let model = if provider.model.is_empty() {
                                    None
                                } else {
                                    Some(provider.model.clone())
                                };
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let ctx_clone = ctx.clone();
                                let ok_tpl = i18n.t("providers.ops.completionStatus").to_string();
                                let err_tpl =
                                    i18n.t("providers.ops.completionStatusFailed").to_string();
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone
                                            .provider_test_completion(&name, model.as_deref()),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!(
                                                "Warning: provider_test_completion timed out"
                                            );
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let msg = match result {
                                        Ok(v) => {
                                            let ok = v
                                                .get("ok")
                                                .and_then(serde_json::Value::as_bool)
                                                .unwrap_or(false);
                                            let model = v
                                                .get("model")
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or("-");
                                            format!(
                                                "__ops__:{}:{}",
                                                name,
                                                ok_tpl
                                                    .replace("{ok}", &ok.to_string())
                                                    .replace("{model}", model)
                                            )
                                        }
                                        Err(e) => format!(
                                            "__ops__:{}:{}",
                                            name,
                                            err_tpl.replace("{error}", &e.to_string())
                                        ),
                                    };
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!("WARN: providers try_send failed: {:?}", e);
                                    }
                                    ctx_clone.request_repaint();
                                });
                            }
                            if ops_enabled
                                && ui.button(i18n.t("providers.ops.capabilities")).clicked()
                            {
                                let name = provider.name.clone();
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let ctx_clone = ctx.clone();
                                let count_tpl =
                                    i18n.t("providers.ops.capabilitiesCount").to_string();
                                let failed_tpl =
                                    i18n.t("providers.ops.capabilitiesFailed").to_string();
                                let encode_failed_tpl =
                                    i18n.t("providers.ops.capabilitiesEncodeFailed").to_string();
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.provider_capabilities(&name),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!("Warning: provider_capabilities timed out");
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let msg = match result {
                                        Ok(models) => match serde_json::to_string(&models) {
                                            Ok(payload) => {
                                                if let Err(e) = tx.try_send(format!(
                                                    "__ops__:{}:{}",
                                                    name,
                                                    count_tpl.replace("{count}", &models.len().to_string())
                                                )) {
                                                    eprintln!("WARN: providers try_send failed: {:?}", e);
                                                }
                                                format!("__caps__:{}:{}", name, payload)
                                            }
                                            Err(e) => format!(
                                                "__ops__:{}:{}",
                                                name,
                                                encode_failed_tpl.replace("{error}", &e.to_string())
                                            ),
                                        },
                                        Err(e) => format!(
                                            "__ops__:{}:{}",
                                            name,
                                            failed_tpl.replace("{error}", &e.to_string())
                                        ),
                                    };
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!("WARN: providers try_send failed: {:?}", e);
                                    }
                                    ctx_clone.request_repaint();
                                });
                            }
                            let delete_label = if self.pending_delete_confirmation == Some(idx) {
                                i18n.t("providers.confirm_delete")
                            } else {
                                i18n.t("providers.delete")
                            };
                            // Delete is a basic operation (not gated by ops_enabled).
                            if ui.button(delete_label).clicked() {
                                if confirm_dangerous
                                    && self.pending_delete_confirmation != Some(idx)
                                {
                                    self.pending_delete_confirmation = Some(idx);
                                    self.status = format!(
                                        "{} {}.",
                                        i18n.t("providers.click_delete_again"),
                                        provider_label(i18n, &provider.name)
                                    );
                                } else {
                                    remove_idx = Some(idx);
                                    self.pending_delete_confirmation = None;
                                }
                            }
                        });
                        if let Some(ops_status) = self.provider_ops_status.get(&provider.name) {
                            ui.label(ops_status);
                        }
                        if let Some(models) = self.provider_capabilities.get(&provider.name) {
                            for model in models.iter().take(3) {
                                let model_name = model
                                    .name
                                    .as_deref()
                                    .unwrap_or(model.id.as_str())
                                    .to_string();
                                let context = model
                                    .context_window
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "-".to_string());
                                let tool = model.tool_calling.unwrap_or(false);
                                let vision = model.vision.unwrap_or(false);
                                let cost = model
                                    .cost_tier
                                    .clone()
                                    .unwrap_or_else(|| "-".to_string());
                                ui.label(format!(
                                    "{} | {}={} | {}={} | {}={} | {}={}",
                                    model_name,
                                    i18n.t("providers.cap.context"),
                                    context,
                                    i18n.t("providers.cap.tool"),
                                    tool,
                                    i18n.t("providers.cap.vision"),
                                    vision,
                                    i18n.t("providers.cap.cost"),
                                    cost
                                ));
                            }
                            if models.len() > 3 {
                                ui.label(
                                    i18n
                                        .t("providers.cap.moreModels")
                                        .replace("{count}", &(models.len() - 3).to_string()),
                                );
                            }
                        }
                    });
                    ui.add_space(4.0);
                }

                if let Some(idx) = remove_idx {
                    // Remove from config first, then check if any instances with the same
                    // provider name remain. Keyring stores keys per provider name (not per
                    // name+label pair), so we must only delete the key when the count reaches
                    // zero — otherwise a remaining labeled instance loses access to its key.
                    let removed = config.providers.remove(idx);
                    let remaining = config.providers.iter().filter(|p| p.name == removed.name).count();
                    if remaining == 0 {
                        let _ = crate::keyring_util::delete_api_key(&removed.name.to_lowercase());
                        // Also clean up secret_key for dual-auth providers (wenxin, qianfan)
                        let _ = crate::keyring_util::delete_secret_key(&removed.name.to_lowercase());
                        // For copilot, also clean up github_copilot_token alias
                        if removed.name.to_lowercase() == "copilot" {
                            let _ = crate::keyring_util::delete_copilot_token();
                        }
                    }
                    changed = true;
                    self.status = i18n.t("providers.removed").to_string();
                } else if self.pending_delete_confirmation.is_some()
                    && !config.providers.is_empty()
                    && self.pending_delete_confirmation.unwrap_or(0) >= config.providers.len()
                {
                    self.pending_delete_confirmation = None;
                }

                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
                    });

                // ── Copilot Device Code auto-poll (uses system proxy) ──
                if self.copilot_device_state.as_deref() == Some("polling") {
                    let poll_interval = Duration::from_secs(self.copilot_poll_interval);
                    let elapsed = self.copilot_last_poll.elapsed();
                    if elapsed >= poll_interval {
                        self.copilot_last_poll = Instant::now();
                        self.copilot_poll_repaint_requested = false;
                        self.copilot_poll_attempts = self.copilot_poll_attempts.saturating_add(1);
                        let backend_clone = backend.clone();
                        let tx = self.pending_tx.clone();
                        let device_code = self.copilot_device_code.clone();
                        let ctx_clone = ctx.clone();
                        #[cfg(debug_assertions)]
                        {
                            let proxy_url = std::env::var("HTTPS_PROXY").unwrap_or_default();
                            eprintln!("[poll] device_code={}, HTTPS_PROXY={}", &device_code[..8.min(device_code.len())], proxy_url);
                        }
                        tokio::spawn(async move {
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(15),
                                backend_clone.copilot_device_code_poll(&device_code),
                            )
                            .await
                            {
                                Ok(Ok(body)) => {
                                    let msg = format!("__copilot_poll__:{}", serde_json::to_string(&body).unwrap_or_default());
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!("WARN: providers try_send failed: {:?}", e);
                                    }
                                }
                                Ok(Err(e)) => {
                                    let msg = format!("__copilot_poll_err__:{}", e);
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!("WARN: providers try_send failed: {:?}", e);
                                    }
                                }
                                Err(_) => {
                                    let msg = "__copilot_poll_err__:Poll timed out.".to_string();
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!("WARN: providers try_send failed: {:?}", e);
                                    }
                                }
                            }
                            ctx_clone.request_repaint();
                        });
                    } else if !self.copilot_poll_repaint_requested {
                        // Schedule repaint for the exact moment the next poll is due.
                        // This avoids forcing a full UI repaint every second while polling.
                        let remaining = poll_interval.saturating_sub(elapsed);
                        ctx.request_repaint_after(remaining.max(Duration::from_millis(100)));
                        self.copilot_poll_repaint_requested = true;
                    }
                }
            });

        changed
    }
}
