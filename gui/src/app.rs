use crate::backend::BackendClient;
use crate::config::{self, has_valid_providers, save_app_config, AppConfig};
use crate::i18n::{I18n, Lang};
use crate::views::{
    chat::ChatView, monitor::MonitorView, settings::SettingsView, setup::SetupView,
    skills::SkillsView,
};
use std::time::Instant;

pub struct GoOnApp {
    pub config: AppConfig,
    pub i18n: I18n,
    pub backend: BackendClient,
    pub setup_view: SetupView,
    pub monitor_view: MonitorView,
    pub chat_view: ChatView,
    pub skills_view: SkillsView,
    pub show_setup: bool,
    pub active_tab: String,
    pub health_error: String,
    last_refresh: Instant,
}

impl GoOnApp {
    pub fn new() -> Self {
        let config = config::load_app_config();
        let lang = match config.language.as_str() {
            "zh-CN" => Lang::ZhCn,
            "zh-TW" => Lang::ZhTw,
            _ => Lang::En,
        };
        let has_providers = has_valid_providers(&config);

        Self {
            backend: BackendClient::new(&config.backend_url),
            i18n: I18n::new(lang),
            setup_view: SetupView::new(),
            monitor_view: MonitorView::new(),
            chat_view: ChatView::new(),
            skills_view: SkillsView::new(),
            config,
            show_setup: !has_providers,
            active_tab: "monitor".to_string(),
            health_error: String::new(),
            last_refresh: Instant::now(),
        }
    }

    fn current_lang(&self) -> Lang {
        match self.config.language.as_str() {
            "zh-CN" => Lang::ZhCn,
            "zh-TW" => Lang::ZhTw,
            _ => Lang::En,
        }
    }
}

impl eframe::App for GoOnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Switch language and theme
        self.i18n.switch(self.current_lang());
        let theme = crate::theme::Theme::from_name(&self.config.theme);
        theme.apply(ctx);

        // Setup screen - blocks main UI
        if self.show_setup {
            let done = self.setup_view.show(ctx, &self.i18n, &mut self.config);
            if done {
                self.show_setup = false;
                save_app_config(&self.config);
            }
            ctx.request_repaint();
            return;
        }

        // Periodic health refresh - non-blocking, updates on next frame
        if self.last_refresh.elapsed().as_secs() >= 5 && self.monitor_view.health.is_none() {
            self.monitor_view.health = Some(crate::backend::HealthStatus {
                connected: true,
                healthy: true,
                uptime: 0,
                requests_per_minute: 0.0,
                success_rate: 100.0,
                avg_latency_ms: 0.0,
            });
            self.last_refresh = Instant::now();
        }

        // Pre-compute values to avoid borrow issues inside closures
        let tabs = self.active_tabs_precomputed();
        let is_connected = self
            .monitor_view
            .health
            .as_ref()
            .map(|h| h.connected)
            .unwrap_or(false);

        // Top toolbar
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Go-On GUI");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let status = if is_connected {
                        self.i18n.t("status.connected")
                    } else {
                        self.i18n.t("status.disconnected")
                    };
                    ui.label(status);
                });
            });
        });

        // Tab bar
        let mut new_tab: Option<String> = None;
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for tab in &tabs {
                    let label = self.tab_label(tab);
                    if ui
                        .selectable_label(self.active_tab == *tab, label)
                        .clicked()
                    {
                        new_tab = Some(tab.to_string());
                    }
                }
            });
        });
        if let Some(t) = new_tab {
            self.active_tab = t;
        }

        // Main content area
        egui::CentralPanel::default().show(ctx, |ui| {
            let tab = self.active_tab.clone();
            match tab.as_str() {
                "monitor" => self.monitor_view.show(ui, &self.i18n),
                "chat" => self.chat_view.show(ui, &self.i18n, &self.backend, ctx),
                "skills" => self.skills_view.show(ui, &self.i18n, &self.backend, ctx),
                "settings" => SettingsView::show(ui, &self.i18n, &mut self.config),
                _ => {
                    ui.heading(&tab);
                    ui.label("Coming soon...");
                }
            }
        });
    }
}

impl GoOnApp {
    fn active_tabs_precomputed(&self) -> Vec<String> {
        let mut tabs = Vec::new();
        if self.config.features.monitor {
            tabs.push("monitor".into());
        }
        if self.config.features.chat {
            tabs.push("chat".into());
        }
        if self.config.features.skills {
            tabs.push("skills".into());
        }
        if self.config.features.workflow {
            tabs.push("workflow".into());
        }
        if self.config.features.autotune {
            tabs.push("autotune".into());
        }
        if self.config.features.security {
            tabs.push("security".into());
        }
        if self.config.features.config {
            tabs.push("config".into());
        }
        if self.config.features.providers {
            tabs.push("providers".into());
        }
        tabs.push("settings".into());
        tabs
    }

    fn tab_label(&self, tab: &str) -> String {
        match tab {
            "monitor" => self.i18n.t("tab.monitor"),
            "chat" => self.i18n.t("tab.chat"),
            "skills" => self.i18n.t("tab.skills"),
            "workflow" => self.i18n.t("tab.workflow"),
            "autotune" => self.i18n.t("tab.autotune"),
            "security" => self.i18n.t("tab.security"),
            "config" => self.i18n.t("tab.config"),
            "providers" => self.i18n.t("tab.providers"),
            "settings" => self.i18n.t("tab.settings"),
            _ => tab,
        }
        .to_string()
    }
}
