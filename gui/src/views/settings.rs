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

        egui::Grid::new("feature_grid")
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

        if changed {
            save_app_config(config);
            ui.ctx().request_repaint();
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(8.0);

        // Backend URL setting
        ui.label(i18n.t("settings.backendUrl"));
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

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(8.0);

        // Language selector
        ui.label(i18n.t("settings.language"));
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

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(8.0);

        // Theme selector
        ui.label(i18n.t("theme.title"));
        ui.horizontal(|ui| {
            let themes = crate::theme::Theme::all();
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
    }
}
