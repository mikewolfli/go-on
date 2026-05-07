use crate::backend::BackendClient;
use crate::config::{self, has_valid_providers, save_app_config, AppConfig};
use crate::i18n::{I18n, Lang};
use crate::views::{
    autotune::AutoTuneView, chat::ChatView, config_editor::ConfigEditorView, monitor::MonitorView,
    providers::ProvidersView, security::SecurityView, settings::SettingsView, setup::SetupView,
    skills::SkillsView, workflow::WorkflowView,
};
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

enum BackendUpdate {
    Health(HealthStatus),
    Providers(Vec<ProviderStatus>),
    RefreshDone,
}

/// Find the go-on backend binary path relative to the GUI executable.
///
/// Look order:
/// 1. Same directory as GUI (release layout: gui + backend/ side by side)
/// 2. `backend/` subdirectory next to GUI
/// 3. `$PATH`
fn find_backend_binary() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();

    // Layout 1: backend/go-on next to the GUI binary
    let candidates = [exe_dir.join("backend").join("go-on"), exe_dir.join("go-on")];
    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }

    // Layout 2: search PATH
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join("go-on");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

/// Start the backend process in the background.
/// Returns the child process handle or an error message.
fn start_backend_process() -> Result<std::process::Child, String> {
    let backend_path = find_backend_binary().ok_or_else(|| {
        "找不到 go-on 后端程序。请确保 go-on 放在 backend/ 目录下或 PATH 中。".to_string()
    })?;

    let config_dir = backend_path.parent().unwrap_or(std::path::Path::new("."));

    let child = std::process::Command::new(&backend_path)
        .current_dir(config_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("启动后端失败: {}", e))?;

    Ok(child)
}

use crate::backend::{HealthStatus, ProviderStatus};

pub struct GoOnApp {
    pub config: AppConfig,
    pub i18n: I18n,
    pub backend: BackendClient,
    pub setup_view: SetupView,
    pub monitor_view: MonitorView,
    pub chat_view: ChatView,
    pub skills_view: SkillsView,
    pub workflow_view: WorkflowView,
    pub autotune_view: AutoTuneView,
    pub security_view: SecurityView,
    pub config_editor_view: ConfigEditorView,
    pub providers_view: ProvidersView,
    pub show_setup: bool,
    pub active_tab: String,
    pub has_providers: bool,
    backend_updates: mpsc::Receiver<BackendUpdate>,
    backend_tx: mpsc::Sender<BackendUpdate>,
    pending_refresh: bool,
    last_refresh: Instant,
    /// Handle to the managed backend child process.
    backend_child: Option<std::process::Child>,
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

        // Try to start the backend automatically
        let (backend, backend_child) = match start_backend_process() {
            Ok(child) => {
                eprintln!("go-on 后端已启动 (PID: {})", child.id());
                (BackendClient::new(&config.backend_url), Some(child))
            }
            Err(e) => {
                eprintln!("警告: {}", e);
                (BackendClient::new(&config.backend_url), None)
            }
        };

        Self {
            backend,
            i18n: I18n::new(lang),
            setup_view: SetupView::new(),
            monitor_view: MonitorView::new(),
            chat_view: ChatView::new(),
            skills_view: SkillsView::new(),
            workflow_view: WorkflowView::new(),
            autotune_view: AutoTuneView::new(),
            security_view: SecurityView::new(),
            config_editor_view: ConfigEditorView::new(),
            providers_view: ProvidersView::new(),
            config,
            show_setup: !providers_valid,
            active_tab: "monitor".to_string(),
            has_providers: providers_valid,
            backend_updates,
            backend_tx,
            pending_refresh: false,
            last_refresh: Instant::now(),
            backend_child,
        }
    }

    fn current_lang(&self) -> Lang {
        match self.config.language.as_str() {
            "zh-CN" => Lang::ZhCn,
            "zh-TW" => Lang::ZhTw,
            _ => Lang::En,
        }
    }

    /// Sync the backend client's URL with the config (in case user changed it in settings)
    fn sync_backend_url(&mut self) {
        let config_url = self.config.backend_url.trim_end_matches('/').to_string();
        if self.backend.base_url() != config_url {
            self.backend.set_base_url(&config_url);
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
        ctx.request_repaint_after(Duration::from_millis(200));

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

        // Sync backend URL from config to the client in case the user changed it in settings
        self.sync_backend_url();

        // Poll for async backend updates arriving from spawned tasks
        self.poll_backend_updates();

        // Periodically trigger a new async health / provider refresh
        self.maybe_refresh_backend();

        // Keep provider availability in sync with current editable config
        self.has_providers = has_valid_providers(&self.config);

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
            .is_some_and(|h| h.connected);

        // Top toolbar
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.i18n.t("app.title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let status = if is_connected {
                        self.i18n.t("status.connected")
                    } else {
                        self.i18n.t("status.disconnected")
                    };
                    // Also show backend PID if managed
                    let pid_info = self
                        .backend_child
                        .as_ref()
                        .map(|c| format!(" [PID:{}]", c.id()))
                        .unwrap_or_default();
                    ui.label(format!("{}{}", status, pid_info));
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
                        new_tab = Some(tab.clone());
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
                "skills" => {
                    self.skills_view.show(ui, &self.i18n, &self.backend, ctx);
                }
                "settings" => SettingsView::show(ui, &self.i18n, &mut self.config),
                "workflow" => self.workflow_view.show(ui, ctx),
                "autotune" => self.autotune_view.show(ui),
                "security" => self.security_view.show(ui, &self.backend, ctx),
                "config" => self.config_editor_view.show(ui, &mut self.config),
                "providers" => self
                    .providers_view
                    .show(ui, &mut self.config, &self.backend, ctx),
                _ => {
                    ui.heading(&tab);
                    ui.label("Unknown tab id.");
                }
            }
        });
    }
}

impl Drop for GoOnApp {
    fn drop(&mut self) {
        // Kill the managed backend process when GUI exits
        if let Some(mut child) = self.backend_child.take() {
            eprintln!("正在关闭 go-on 后端 (PID: {})...", child.id());
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("go-on 后端已关闭");
        }
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
