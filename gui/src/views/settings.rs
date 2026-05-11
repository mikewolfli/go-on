use crate::config::save_app_config;
use crate::config::AppConfig;
use crate::config::UiStabilityConfig;
use crate::i18n::I18n;

pub struct SettingsView;

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiStabilityPreset {
    Balanced,
    Stable,
    LowEnd,
    LowLatency,
    Custom,
}

impl SettingsView {
    fn ui_stability_preset(config: &UiStabilityConfig) -> UiStabilityPreset {
        match (
            config.backend_refresh_interval_secs,
            config.backend_ui_commit_debounce_ms,
            config.health_disconnect_debounce_count,
            config.chat_stream_chunk_flush_ms,
            config.chat_repaint_interval_ms,
            config.chat_max_pending_events_per_frame,
        ) {
            (5, 120, 2, 33, 33, 256) => UiStabilityPreset::Balanced,
            (8, 180, 3, 50, 50, 320) => UiStabilityPreset::Stable,
            (10, 220, 3, 66, 66, 192) => UiStabilityPreset::LowEnd,
            (3, 80, 2, 24, 24, 384) => UiStabilityPreset::LowLatency,
            _ => UiStabilityPreset::Custom,
        }
    }

    fn apply_ui_stability_preset(config: &mut UiStabilityConfig, preset: UiStabilityPreset) {
        match preset {
            UiStabilityPreset::Balanced => {
                config.backend_refresh_interval_secs = 5;
                config.backend_ui_commit_debounce_ms = 120;
                config.health_disconnect_debounce_count = 2;
                config.chat_stream_chunk_flush_ms = 33;
                config.chat_repaint_interval_ms = 33;
                config.chat_max_pending_events_per_frame = 256;
            }
            UiStabilityPreset::Stable => {
                config.backend_refresh_interval_secs = 8;
                config.backend_ui_commit_debounce_ms = 180;
                config.health_disconnect_debounce_count = 3;
                config.chat_stream_chunk_flush_ms = 50;
                config.chat_repaint_interval_ms = 50;
                config.chat_max_pending_events_per_frame = 320;
            }
            UiStabilityPreset::LowEnd => {
                config.backend_refresh_interval_secs = 10;
                config.backend_ui_commit_debounce_ms = 220;
                config.health_disconnect_debounce_count = 3;
                config.chat_stream_chunk_flush_ms = 66;
                config.chat_repaint_interval_ms = 66;
                config.chat_max_pending_events_per_frame = 192;
            }
            UiStabilityPreset::LowLatency => {
                config.backend_refresh_interval_secs = 3;
                config.backend_ui_commit_debounce_ms = 80;
                config.health_disconnect_debounce_count = 2;
                config.chat_stream_chunk_flush_ms = 24;
                config.chat_repaint_interval_ms = 24;
                config.chat_max_pending_events_per_frame = 384;
            }
            UiStabilityPreset::Custom => {}
        }
    }

    fn ui_stability_preset_label(preset: UiStabilityPreset) -> &'static str {
        match preset {
            UiStabilityPreset::Balanced => "Balanced / 平衡",
            UiStabilityPreset::Stable => "Stable / 稳态优先",
            UiStabilityPreset::LowEnd => "Low-end / 低性能机器",
            UiStabilityPreset::LowLatency => "Low latency / 低延迟",
            UiStabilityPreset::Custom => "Custom / 自定义",
        }
    }

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
                // Core features section
                ui.label(egui::RichText::new(i18n.t("settings.section.core")).strong());
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

                // Advanced features section
                ui.label(egui::RichText::new(i18n.t("settings.section.advanced")).strong());
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

                // System settings section
                ui.label(egui::RichText::new(i18n.t("settings.section.system")).strong());
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

                // Enterprise settings section
                if config.features.setup_enterprise {
                    ui.label(egui::RichText::new(i18n.t("settings.section.enterprise")).strong());
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
                            // Config no longer stores api_key in plaintext
                            // (keys are in system keyring). Direct export is safe.
                            if let Ok(content) = serde_json::to_string_pretty(&config) {
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

                // Backend URL section
                ui.label(egui::RichText::new(i18n.t("settings.section.backend")).strong());
                ui.add_space(4.0);
                ui.label(i18n.t("settings.backendUrlHint"));
                ui.horizontal(|ui| {
                    let mut url = config.backend_url.clone();
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut url)
                            .hint_text(i18n.t("settings.backendUrlPlaceholder"))
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

                // Language section
                ui.label(egui::RichText::new(i18n.t("settings.section.language")).strong());
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

                // Theme section
                ui.label(egui::RichText::new(i18n.t("settings.section.theme")).strong());
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

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // UI stability section (anti-jitter tuning)
                ui.label(egui::RichText::new("UI Stability / 防抖参数").strong());
                ui.add_space(4.0);
                ui.label("Adjust repaint batching and cadence to reduce periodic shaking.");
                ui.add_space(4.0);

                let stability = &mut config.ui_stability;

                ui.horizontal(|ui| {
                    ui.label("Preset");
                    let mut selected_preset = Self::ui_stability_preset(stability);
                    egui::ComboBox::from_id_salt("ui_stability_preset_selector")
                        .selected_text(Self::ui_stability_preset_label(selected_preset))
                        .show_ui(ui, |ui| {
                            for preset in [
                                UiStabilityPreset::Balanced,
                                UiStabilityPreset::Stable,
                                UiStabilityPreset::LowEnd,
                                UiStabilityPreset::LowLatency,
                            ] {
                                if ui
                                    .selectable_label(
                                        selected_preset == preset,
                                        Self::ui_stability_preset_label(preset),
                                    )
                                    .clicked()
                                {
                                    selected_preset = preset;
                                }
                            }
                        });

                    if selected_preset != UiStabilityPreset::Custom
                        && selected_preset != Self::ui_stability_preset(stability)
                    {
                        Self::apply_ui_stability_preset(stability, selected_preset);
                        changed = true;
                    }
                });
                ui.add_space(4.0);

                egui::Grid::new("ui_stability_grid")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label("Backend refresh interval (s)");
                        let mut v = stability.backend_refresh_interval_secs;
                        if ui
                            .add(egui::Slider::new(&mut v, 1..=60).suffix(" s"))
                            .changed()
                        {
                            stability.backend_refresh_interval_secs = v;
                            changed = true;
                        }
                        ui.end_row();

                        ui.label("Backend UI commit debounce (ms)");
                        let mut v = stability.backend_ui_commit_debounce_ms;
                        if ui
                            .add(egui::Slider::new(&mut v, 16..=1000).suffix(" ms"))
                            .changed()
                        {
                            stability.backend_ui_commit_debounce_ms = v;
                            changed = true;
                        }
                        ui.end_row();

                        ui.label("Disconnect debounce samples");
                        let mut v = u64::from(stability.health_disconnect_debounce_count);
                        if ui.add(egui::Slider::new(&mut v, 1..=8)).changed() {
                            stability.health_disconnect_debounce_count = v as u8;
                            changed = true;
                        }
                        ui.end_row();

                        ui.label("Chat stream chunk flush (ms)");
                        let mut v = stability.chat_stream_chunk_flush_ms;
                        if ui
                            .add(egui::Slider::new(&mut v, 16..=200).suffix(" ms"))
                            .changed()
                        {
                            stability.chat_stream_chunk_flush_ms = v;
                            changed = true;
                        }
                        ui.end_row();

                        ui.label("Chat repaint interval (ms)");
                        let mut v = stability.chat_repaint_interval_ms;
                        if ui
                            .add(egui::Slider::new(&mut v, 16..=200).suffix(" ms"))
                            .changed()
                        {
                            stability.chat_repaint_interval_ms = v;
                            changed = true;
                        }
                        ui.end_row();

                        ui.label("Chat max pending events/frame");
                        let mut v = stability.chat_max_pending_events_per_frame as u64;
                        if ui.add(egui::Slider::new(&mut v, 16..=4096)).changed() {
                            stability.chat_max_pending_events_per_frame = v as usize;
                            changed = true;
                        }
                        ui.end_row();
                    });

                if changed {
                    save_app_config(config);
                    ui.ctx().request_repaint();
                }

                ui.add_space(20.0); // 底部留白，确保可以滚动到最后
            }); // ScrollArea 结束
    }
}
