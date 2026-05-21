use crate::config::save_app_config;
use crate::config::AppConfig;
use crate::config::UiStabilityConfig;
use crate::i18n::I18n;
use crate::section_hash;
use crate::widgets::cache::CachedView;
use std::cell::RefCell;

thread_local! {
    static SETTINGS_CACHE: RefCell<CachedView> = RefCell::new(CachedView::new());
}

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

    fn ui_stability_preset_label(preset: UiStabilityPreset, i18n: &I18n) -> String {
        match preset {
            UiStabilityPreset::Balanced => {
                i18n.t("settings.uiStability.preset.balanced").to_string()
            }
            UiStabilityPreset::Stable => i18n.t("settings.uiStability.preset.stable").to_string(),
            UiStabilityPreset::LowEnd => i18n.t("settings.uiStability.preset.lowend").to_string(),
            UiStabilityPreset::LowLatency => {
                i18n.t("settings.uiStability.preset.lowlatency").to_string()
            }
            UiStabilityPreset::Custom => i18n.t("settings.uiStability.preset.custom").to_string(),
        }
    }

    pub fn show(ui: &mut egui::Ui, i18n: &I18n, config: &mut AppConfig) {
        ui.heading(i18n.t("settings.title"));
        ui.label(i18n.t("settings.hint"));
        ui.separator();
        ui.add_space(8.0);

        let mut changed = false;

        // Use ScrollArea for scrolling support
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                // Compute hash from ALL render-relevant config fields
                let hash = section_hash!(
                    config.features.monitor,
                    config.features.chat,
                    config.features.skills,
                    config.features.workflow,
                    config.features.autotune,
                    config.features.security,
                    config.features.config,
                    config.features.providers,
                    config.features.show_prompts_tab,
                    config.features.workflow_run_center,
                    config.features.autotune_chain_injection,
                    config.features.skills_lifecycle,
                    config.features.providers_ops,
                    config.features.monitor_history_alerts,
                    config.features.config_safe_mode,
                    config.features.setup_enterprise,
                    config.enterprise.active_environment,
                    config.enterprise.secret_source,
                    config.enterprise.export_path,
                    config.enterprise.import_path,
                    config.enterprise.environments.len(),
                    config.backend_url,
                    config.language,
                    config.theme,
                    config.ui_stability.backend_refresh_interval_secs,
                    config.ui_stability.backend_ui_commit_debounce_ms,
                    config.ui_stability.health_disconnect_debounce_count,
                    config.ui_stability.chat_stream_chunk_flush_ms,
                    config.ui_stability.chat_repaint_interval_ms,
                    config.ui_stability.chat_max_pending_events_per_frame,
                );

                SETTINGS_CACHE.with(|cache| {
                    cache
                        .borrow_mut()
                        .check_or_render(ui, "settings", hash, |ui| {
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

                                    ui.label(i18n.t("tab.prompts"));
                                    if ui
                                        .checkbox(&mut config.features.show_prompts_tab, "")
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                    ui.end_row();
                                });

                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(8.0);

                            // Advanced features section
                            ui.label(
                                egui::RichText::new(i18n.t("settings.section.advanced")).strong(),
                            );
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
                            ui.label(
                                egui::RichText::new(i18n.t("settings.section.system")).strong(),
                            );
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

                            // Note: save_app_config is called once at the end of the function
                            // (after the stability section) to batch all changes into a single save.
                            // Do NOT call save_app_config here — it will be called at the bottom.
                            if changed {
                                ui.ctx().request_repaint();
                            }

                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(8.0);

                            // Enterprise settings section
                            if config.features.setup_enterprise {
                                ui.label(
                                    egui::RichText::new(i18n.t("settings.section.enterprise"))
                                        .strong(),
                                );
                                ui.add_space(4.0);

                                ui.horizontal(|ui| {
                                    ui.label(i18n.t("settings.enterprise.environment"));
                                    egui::ComboBox::from_id_salt("enterprise_environment")
                                        .selected_text(config.enterprise.active_environment.clone())
                                        .show_ui(ui, |ui| {
                                            for env in &config.enterprise.environments {
                                                if ui
                                                    .selectable_label(
                                                        config.enterprise.active_environment
                                                            == env.name,
                                                        &env.name,
                                                    )
                                                    .clicked()
                                                {
                                                    config.enterprise.active_environment =
                                                        env.name.clone();
                                                    config.backend_url = env.backend_url.clone();
                                                    changed = true;
                                                }
                                            }
                                        });
                                });

                                if let Some(active_index) =
                                    config.enterprise.environments.iter().position(|env| {
                                        env.name == config.enterprise.active_environment
                                    })
                                {
                                    let mut backend_url = config.enterprise.environments
                                        [active_index]
                                        .backend_url
                                        .clone();
                                    let mut url_changed = false;
                                    ui.horizontal(|ui| {
                                        ui.label(i18n.t("settings.enterprise.environmentUrl"));
                                        let resp = ui.add(
                                            egui::TextEdit::singleline(&mut backend_url)
                                                .desired_width(320.0),
                                        );
                                        url_changed = resp.changed();
                                    });
                                    if url_changed {
                                        let normalized =
                                            backend_url.trim().trim_end_matches('/').to_string();
                                        config.enterprise.environments[active_index].backend_url =
                                            normalized.clone();
                                        config.backend_url = normalized;
                                        changed = true;
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
                                                    config.enterprise.secret_source =
                                                        source.to_string();
                                                    changed = true;
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
                                            let _ = std::fs::write(
                                                &config.enterprise.export_path,
                                                content,
                                            );
                                        }
                                    }
                                    if ui
                                        .button(i18n.t("settings.enterprise.exportFull"))
                                        .clicked()
                                    {
                                        if let Ok(content) = serde_json::to_string_pretty(config) {
                                            let _ = std::fs::write(
                                                &config.enterprise.export_path,
                                                content,
                                            );
                                        }
                                    }
                                    if ui
                                        .button(i18n.t("settings.enterprise.importConfig"))
                                        .clicked()
                                    {
                                        if let Ok(content) =
                                            std::fs::read_to_string(&config.enterprise.import_path)
                                        {
                                            if let Ok(imported) =
                                                serde_json::from_str::<AppConfig>(&content)
                                            {
                                                *config = imported;
                                                save_app_config(config);
                                            }
                                        }
                                    }
                                    if ui
                                        .button(i18n.t("settings.enterprise.syncCurrent"))
                                        .clicked()
                                    {
                                        if let Some(active_env) =
                                            config.enterprise.environments.iter_mut().find(|env| {
                                                env.name == config.enterprise.active_environment
                                            })
                                        {
                                            active_env.backend_url = config.backend_url.clone();
                                            changed = true;
                                        }
                                    }
                                });

                                ui.label(i18n.t("settings.enterprise.hint"));
                                ui.add_space(12.0);
                                ui.separator();
                                ui.add_space(8.0);
                            }

                            // Backend URL section
                            ui.label(
                                egui::RichText::new(i18n.t("settings.section.backend")).strong(),
                            );
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
                                    config.backend_url =
                                        url.trim().trim_end_matches('/').to_string();
                                    changed = true;
                                    ui.ctx().request_repaint();
                                }
                            });

                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(8.0);

                            // Language section
                            ui.label(
                                egui::RichText::new(i18n.t("settings.section.language")).strong(),
                            );
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
                                        changed = true;
                                        ui.ctx().request_repaint();
                                    }
                                }
                            });

                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(8.0);

                            // Theme section
                            ui.label(
                                egui::RichText::new(i18n.t("settings.section.theme")).strong(),
                            );
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(i18n.t("settings.theme"));
                                let themes = crate::theme::Theme::all();
                                let current_display = themes
                                    .iter()
                                    .find(|(_, name)| *name == config.theme)
                                    .map(|(v, _)| v.display_name(i18n))
                                    .unwrap_or_else(|| {
                                        std::borrow::Cow::Borrowed(config.theme.as_str())
                                    });
                                egui::ComboBox::from_id_salt("theme_selector")
                                    .selected_text(current_display)
                                    .show_ui(ui, |ui| {
                                        for (theme_variant, config_name) in themes {
                                            let label = theme_variant.display_name(i18n);
                                            if ui
                                                .selectable_label(
                                                    config.theme == *config_name,
                                                    label,
                                                )
                                                .clicked()
                                            {
                                                config.theme = config_name.to_string();
                                                changed = true;
                                                ui.ctx().request_repaint();
                                            }
                                        }
                                    });
                            });

                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(8.0);

                            // UI stability section (anti-jitter tuning)
                            ui.label(
                                egui::RichText::new(i18n.t("settings.uiStability.title")).strong(),
                            );
                            ui.add_space(4.0);
                            ui.label(i18n.t("settings.uiStability.hint"));
                            ui.add_space(4.0);

                            let stability = &mut config.ui_stability;

                            ui.horizontal(|ui| {
                                ui.label(i18n.t("settings.uiStability.preset"));
                                let mut selected_preset = Self::ui_stability_preset(stability);
                                egui::ComboBox::from_id_salt("ui_stability_preset_selector")
                                    .selected_text(Self::ui_stability_preset_label(
                                        selected_preset,
                                        i18n,
                                    ))
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
                                                    Self::ui_stability_preset_label(preset, i18n),
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
                                    ui.label(i18n.t("settings.uiStability.backendRefreshInterval"));
                                    let mut v = stability.backend_refresh_interval_secs;
                                    if ui
                                        .add(egui::Slider::new(&mut v, 1..=60).suffix(" s"))
                                        .changed()
                                    {
                                        stability.backend_refresh_interval_secs = v;
                                        changed = true;
                                    }
                                    ui.end_row();

                                    ui.label(i18n.t("settings.uiStability.backendCommitDebounce"));
                                    let mut v = stability.backend_ui_commit_debounce_ms;
                                    if ui
                                        .add(egui::Slider::new(&mut v, 16..=1000).suffix(" ms"))
                                        .changed()
                                    {
                                        stability.backend_ui_commit_debounce_ms = v;
                                        changed = true;
                                    }
                                    ui.end_row();

                                    ui.label(i18n.t("settings.uiStability.disconnectDebounce"));
                                    let mut v =
                                        u64::from(stability.health_disconnect_debounce_count);
                                    if ui.add(egui::Slider::new(&mut v, 1..=8)).changed() {
                                        stability.health_disconnect_debounce_count = v as u8;
                                        changed = true;
                                    }
                                    ui.end_row();

                                    ui.label(i18n.t("settings.uiStability.chatStreamFlush"));
                                    let mut v = stability.chat_stream_chunk_flush_ms;
                                    if ui
                                        .add(egui::Slider::new(&mut v, 16..=200).suffix(" ms"))
                                        .changed()
                                    {
                                        stability.chat_stream_chunk_flush_ms = v;
                                        changed = true;
                                    }
                                    ui.end_row();

                                    ui.label(i18n.t("settings.uiStability.chatRepaintInterval"));
                                    let mut v = stability.chat_repaint_interval_ms;
                                    if ui
                                        .add(egui::Slider::new(&mut v, 16..=200).suffix(" ms"))
                                        .changed()
                                    {
                                        stability.chat_repaint_interval_ms = v;
                                        changed = true;
                                    }
                                    ui.end_row();

                                    ui.label(i18n.t("settings.uiStability.chatMaxPendingEvents"));
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

                            ui.add_space(20.0);
                            ui.separator();
                            ui.add_space(8.0);
                            // Reset to defaults
                            if ui
                                .button("🔄 ".to_string() + &i18n.t("settings.resetDefaults"))
                                .clicked()
                            {
                                *config = AppConfig::default();
                                save_app_config(config);
                                ui.ctx().request_repaint();
                            }

                            ui.add_space(20.0); // Bottom padding to ensure scrollable to the end
                        }); // End check_or_render
                }); // End SETTINGS_CACHE.with
            }); // End ScrollArea
    }
}
