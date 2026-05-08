use crate::config::save_app_config;
use crate::config::AppConfig;
use crate::i18n::I18n;

pub struct SettingsView;

impl SettingsView {
    pub fn show(ui: &mut egui::Ui, i18n: &I18n, config: &mut AppConfig) {
        ui.heading(i18n.t("settings.title"));
        ui.label(i18n.t("settings.hint"));
        ui.separator();
        ui.add_space(8.0);

        let mut changed = false;

        // 使用 ScrollArea 支持滚动
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                // ─── 核心功能 ────────────────────────────────────
                ui.label(egui::RichText::new("🔷 核心功能 / Core Features").strong());
                ui.add_space(4.0);
                egui::Grid::new("core_features_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(i18n.t("tab.monitor"));
                        if ui.checkbox(&mut config.features.monitor, "").changed() {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("tab.chat"));
                        if ui.checkbox(&mut config.features.chat, "").changed() {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("tab.skills"));
                        if ui.checkbox(&mut config.features.skills, "").changed() {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("tab.workflow"));
                        if ui.checkbox(&mut config.features.workflow, "").changed() {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("tab.autotune"));
                        if ui.checkbox(&mut config.features.autotune, "").changed() {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("tab.security"));
                        if ui.checkbox(&mut config.features.security, "").changed() {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("tab.config"));
                        if ui.checkbox(&mut config.features.config, "").changed() {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("tab.providers"));
                        if ui.checkbox(&mut config.features.providers, "").changed() {
                            changed = true;
                        }
                        ui.end_row();
                    });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // ─── 高级功能 ────────────────────────────────────
                ui.label(egui::RichText::new("⚡ 高级功能 / Advanced Features").strong());
                ui.add_space(4.0);
                egui::Grid::new("advanced_features_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(i18n.t("settings.feature.workflowRunCenter"));
                        if ui
                            .checkbox(&mut config.features.workflow_run_center, "")
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("settings.feature.autotuneChainInjection"));
                        if ui
                            .checkbox(&mut config.features.autotune_chain_injection, "")
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("settings.feature.skillsLifecycle"));
                        if ui
                            .checkbox(&mut config.features.skills_lifecycle, "")
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("settings.feature.providersOps"));
                        if ui
                            .checkbox(&mut config.features.providers_ops, "")
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("settings.feature.monitorHistoryAlerts"));
                        if ui
                            .checkbox(&mut config.features.monitor_history_alerts, "")
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();
                    });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // ─── 系统设置 ────────────────────────────────────
                ui.label(egui::RichText::new("⚙️ 系统设置 / System Settings").strong());
                ui.add_space(4.0);
                egui::Grid::new("system_features_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(i18n.t("settings.feature.configSafeMode"));
                        if ui
                            .checkbox(&mut config.features.config_safe_mode, "")
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();

                        ui.label(i18n.t("settings.feature.setupEnterprise"));
                        if ui
                            .checkbox(&mut config.features.setup_enterprise, "")
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();
                    });

                if changed {
                    save_app_config(config);
                    ui.ctx().request_repaint();
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // ─── 企业设置 ────────────────────────────────────
                if config.features.setup_enterprise {
                    ui.label(egui::RichText::new("🏢 企业设置 / Enterprise Settings").strong());
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        ui.label(i18n.t("settings.enterprise.environment"));
                        egui::ComboBox::from_id_salt("enterprise_environment")
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
                                        save_app_config(config);
                                    }
                                }
                            });
                    });

                    if let Some(active_index) = config
                        .enterprise
                        .environments
                        .iter()
                        .position(|env| env.name == config.enterprise.active_environment)
                    {
                        let mut backend_url = config.enterprise.environments[active_index]
                            .backend_url
                            .clone();
                        let mut url_changed = false;
                        ui.horizontal(|ui| {
                            ui.label(i18n.t("settings.enterprise.environmentUrl"));
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut backend_url).desired_width(320.0),
                            );
                            url_changed = resp.changed();
                        });
                        if url_changed {
                            let normalized = backend_url.trim().trim_end_matches('/').to_string();
                            config.enterprise.environments[active_index].backend_url =
                                normalized.clone();
                            config.backend_url = normalized;
                            save_app_config(config);
                        }
                    }

                    ui.horizontal(|ui| {
                        ui.label(i18n.t("settings.enterprise.secretSource"));
                        egui::ComboBox::from_id_salt("enterprise_secret_source")
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
                                        save_app_config(config);
                                    }
                                }
                            });
                    });

                    ui.horizontal(|ui| {
                        ui.label(i18n.t("settings.enterprise.exportPath"));
                        ui.text_edit_singleline(&mut config.enterprise.export_path);
                    });
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("settings.enterprise.importPath"));
                        ui.text_edit_singleline(&mut config.enterprise.import_path);
                    });

                    ui.horizontal(|ui| {
                        if ui
                            .button(i18n.t("settings.enterprise.exportMasked"))
                            .clicked()
                        {
                            let mut masked = config.clone();
                            for provider in &mut masked.providers {
                                if !provider.api_key.is_empty() {
                                    provider.api_key = "********".to_string();
                                }
                            }
                            if let Ok(content) = serde_json::to_string_pretty(&masked) {
                                let _ = std::fs::write(&config.enterprise.export_path, content);
                            }
                        }
                        if ui
                            .button(i18n.t("settings.enterprise.exportFull"))
                            .clicked()
                        {
                            if let Ok(content) = serde_json::to_string_pretty(config) {
                                let _ = std::fs::write(&config.enterprise.export_path, content);
                            }
                        }
                        if ui
                            .button(i18n.t("settings.enterprise.importConfig"))
                            .clicked()
                        {
                            if let Ok(content) =
                                std::fs::read_to_string(&config.enterprise.import_path)
                            {
                                if let Ok(imported) = serde_json::from_str::<AppConfig>(&content) {
                                    *config = imported;
                                    save_app_config(config);
                                }
                            }
                        }
                        if ui
                            .button(i18n.t("settings.enterprise.syncCurrent"))
                            .clicked()
                        {
                            if let Some(active_env) = config
                                .enterprise
                                .environments
                                .iter_mut()
                                .find(|env| env.name == config.enterprise.active_environment)
                            {
                                active_env.backend_url = config.backend_url.clone();
                                save_app_config(config);
                            }
                        }
                    });

                    ui.label(i18n.t("settings.enterprise.hint"));
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);
                }

                // ─── 后端地址 ────────────────────────────────────
                ui.label(egui::RichText::new("🔗 后端地址 / Backend URL").strong());
                ui.add_space(4.0);
                ui.label(i18n.t("settings.backendUrlHint"));
                ui.horizontal(|ui| {
                    let mut url = config.backend_url.clone();
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut url)
                            .hint_text("http://127.0.0.1:8090")
                            .desired_width(300.0),
                    );
                    if resp.changed() && !url.is_empty() {
                        config.backend_url = url.trim().trim_end_matches('/').to_string();
                        save_app_config(config);
                        ui.ctx().request_repaint();
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // ─── 语言 ────────────────────────────────────
                ui.label(egui::RichText::new("🌐 语言 / Language").strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let langs = [
                        ("lang.en", "en"),
                        ("lang.zhCn", "zh-CN"),
                        ("lang.zhTw", "zh-TW"),
                    ];
                    for (key, code) in &langs {
                        if ui
                            .selectable_label(config.language == *code, i18n.t(key))
                            .clicked()
                        {
                            config.language = code.to_string();
                            save_app_config(config);
                            ui.ctx().request_repaint();
                        }
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // ─── 主题 ────────────────────────────────────
                ui.label(egui::RichText::new("🎨 主题 / Theme").strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(i18n.t("settings.theme"));
                    let themes = crate::theme::Theme::all();
                    let current_display = themes
                        .iter()
                        .find(|(_, name)| *name == config.theme)
                        .map(|(v, _)| v.display_name(i18n))
                        .unwrap_or_else(|| std::borrow::Cow::Borrowed(config.theme.as_str()));
                    egui::ComboBox::from_id_salt("theme_selector")
                        .selected_text(current_display)
                        .show_ui(ui, |ui| {
                            for (theme_variant, config_name) in themes {
                                let label = theme_variant.display_name(i18n);
                                if ui
                                    .selectable_label(config.theme == *config_name, label)
                                    .clicked()
                                {
                                    config.theme = config_name.to_string();
                                    save_app_config(config);
                                    ui.ctx().request_repaint();
                                }
                            }
                        });
                });

                ui.add_space(20.0); // 底部留白，确保可以滚动到最后
            }); // ScrollArea 结束
    }
}
