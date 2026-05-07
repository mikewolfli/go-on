use crate::backend::{BackendClient, HealthStatus, ProviderStatus};
use crate::config::{self, has_valid_providers, save_app_config, AppConfig};
use crate::i18n::{I18n, Lang};
use crate::views::{
    chat::ChatView, monitor::MonitorView, settings::SettingsView, setup::SetupView,
    skills::SkillsView,
};
use std::sync::mpsc;
use std::time::Instant;

enum BackendUpdate {
    Health(HealthStatus),
    Providers(Vec<ProviderStatus>),
    RefreshDone,
}

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
    pub has_providers: bool,
    backend_updates: mpsc::Receiver<BackendUpdate>,
    backend_tx: mpsc::Sender<BackendUpdate>,
    pending_refresh: bool,
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
        let providers_valid = has_valid_providers(&config);

        let (backend_tx, backend_updates) = mpsc::channel();

        Self {
            backend: BackendClient::new(&config.backend_url),
            i18n: I18n::new(lang),
            setup_view: SetupView::new(),
            monitor_view: MonitorView::new(),
            chat_view: ChatView::new(),
            skills_view: SkillsView::new(),
            config,
            show_setup: !providers_valid,
            active_tab: "monitor".to_string(),
            has_providers: providers_valid,
            backend_updates,
            backend_tx,
            pending_refresh: false,
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

    /// Poll the channel for async backend updates and apply them to the monitor view.
    fn poll_backend_updates(&mut self) {
        while let Ok(update) = self.backend_updates.try_recv() {
            match update {
                BackendUpdate::Health(h) => {
                    self.monitor_view.health = Some(h);
                }
                BackendUpdate::Providers(p) => {
                    self.monitor_view.providers = p;
                }
                BackendUpdate::RefreshDone => {
                    self.pending_refresh = false;
                }
            }
        }
    }

    /// Trigger an async health + provider refresh if enough time has elapsed and no
    /// refresh is already in-flight.
    fn maybe_refresh_backend(&mut self) {
        if self.last_refresh.elapsed().as_secs() >= 5 && !self.pending_refresh {
            self.pending_refresh = true;
            let tx = self.backend_tx.clone();
            let backend = self.backend.clone();
            tokio::spawn(async move {
                // Fetch health
                let health = backend.health().await;
                let _ = tx.send(BackendUpdate::Health(health));
                // Fetch provider status
                let providers = backend.provider_status().await;
                let _ = tx.send(BackendUpdate::Providers(providers));
                // Signal that the refresh cycle is complete
                let _ = tx.send(BackendUpdate::RefreshDone);
            });
            self.last_refresh = Instant::now();
        }
    }
}

impl eframe::App for GoOnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Switch language and theme
        self.i18n.switch(self.current_lang());
        let theme = crate::theme::Theme::from_name(&self.config.theme);
        theme.apply(ctx);
        ctx.request_repaint(); // Ensure theme / repaint is applied each frame

        // Setup screen – blocks main UI
        if self.show_setup {
            let done = self.setup_view.show(ctx, &self.i18n, &mut self.config);
            if done {
                self.show_setup = false;
                self.has_providers = has_valid_providers(&self.config);
                save_app_config(&self.config);
            }
            ctx.request_repaint();
            return;
        }

        // Poll for async backend updates arriving from spawned tasks
        self.poll_backend_updates();

        // Periodically trigger a new async health / provider refresh
        self.maybe_refresh_backend();

        // Pre-compute values to avoid borrow issues inside closures
        let tabs = self.active_tabs_precomputed();

        // If the active feature tab was just disabled, redirect to settings
        if !tabs.contains(&self.active_tab) {
            if tabs.contains(&"monitor".to_string()) {
                self.active_tab = "monitor".to_string();
            } else {
                self.active_tab = "settings".to_string();
            }
        }
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
            let has_backend = self.has_providers;
            match tab.as_str() {
                "monitor" => self.monitor_view.show(ui, &self.i18n, has_backend),
                "chat" => self.chat_view.show(ui, &self.i18n, &self.backend, ctx),
                "skills" => self.skills_view.show(ui, &self.i18n),
                "settings" => SettingsView::show(ui, &self.i18n, &mut self.config),
                _ => {
                    ui.heading(&tab);
                    ui.label(self.i18n.t("app.comingSoon"));
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
            _ => std::borrow::Cow::Borrowed(tab),
        }
        .to_string()
    }
}
