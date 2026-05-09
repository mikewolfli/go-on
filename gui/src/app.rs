use crate::backend::BackendClient;
use crate::config::{self, has_valid_providers, save_app_config, AppConfig};

/// Write a line to go-on-gui.log in the temp directory.
/// Only active in debug builds to avoid blocking the UI thread.
#[allow(dead_code)]
pub fn log_msg(msg: &str) {
    #[cfg(debug_assertions)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        let path = std::env::temp_dir().join("go-on-gui.log");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{}", msg);
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = msg;
}

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

use crate::backend::{HealthStatus, ProviderStatus};

/// Find the go-on backend binary path relative to the GUI executable.
fn find_backend_binary() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe_name = if cfg!(target_os = "windows") {
        "go-on.exe"
    } else {
        "go-on"
    };
    let candidates = [
        exe_dir.join("backend").join(exe_name),
        exe_dir.join(exe_name),
    ];
    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }
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
    /// Managed backend child process
    backend_child: Option<std::process::Child>,
    /// Cache the last applied theme name to avoid calling ctx.set_style() every frame.
    last_applied_theme: String,
}

/// Detect system locale from environment variables.
/// Checks LC_ALL, LC_MESSAGES, LANG on Unix; GetUserDefaultLocaleName on Windows.
fn detect_system_language() -> Lang {
    // Check env vars (cross-platform: Linux, macOS, Windows)
    for var in &["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(val) = std::env::var_os(var) {
            let s = val.to_string_lossy().to_lowercase();
            if s.contains("zh_cn")
                || s.contains("zh-cn")
                || s.contains("zh_cn.")
                || s.contains("chinese")
            {
                return Lang::ZhCn;
            }
            if s.contains("zh_tw")
                || s.contains("zh-tw")
                || s.contains("zh_tw.")
                || s.contains("taiwan")
            {
                return Lang::ZhTw;
            }
        }
    }
    // Fallback: try common locale env vars set by desktop environments
    for var in &["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(val) = std::env::var_os(var) {
            let s = val.to_string_lossy().to_lowercase();
            if s.contains("zh") {
                if s.contains("hant") || s.contains("tw") || s.contains("hk") {
                    return Lang::ZhTw;
                }
                return Lang::ZhCn;
            }
        }
    }
    Lang::En
}

impl GoOnApp {
    pub fn new() -> Self {
        let config = config::load_app_config();
        // Auto-detect: if user hasn't explicitly set a language, try system locale
        let lang = if config.language.is_empty() || config.language == "en" {
            detect_system_language()
        } else {
            match config.language.as_str() {
                "zh-CN" => Lang::ZhCn,
                "zh-TW" => Lang::ZhTw,
                _ => Lang::En,
            }
        };
        let providers_valid = has_valid_providers(&config);

        let (backend_tx, backend_updates) = mpsc::channel();

        // Only start backend if valid API keys exist
        let (backend, backend_child) = if providers_valid {
            match find_backend_binary() {
                Some(path) => {
                    let config_dir = path.parent().unwrap_or(std::path::Path::new("."));
                    let mut cmd = std::process::Command::new(&path);
                    cmd.current_dir(config_dir)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    // Load API keys from config and keyring
                    let mut set_env = |name: &str, key: &str| {
                        let env_var = match name {
                            "deepseek" => "DEEPSEEK_API_KEY",
                            "openai" => "OPENAI_API_KEY",
                            "anthropic" => "ANTHROPIC_API_KEY",
                            "qwen" => "QWEN_API_KEY",
                            "gemini" => "GEMINI_API_KEY",
                            "groq" => "GROQ_API_KEY",
                            "mistral" => "MISTRAL_API_KEY",
                            "copilot" => "GITHUB_COPILOT_TOKEN",
                            _ => return,
                        };
                        cmd.env(env_var, key);
                    };
                    for p in &config.providers {
                        if !p.api_key.is_empty() {
                            set_env(&p.name.to_lowercase(), &p.api_key);
                        }
                    }
                    let known = [
                        "deepseek",
                        "openai",
                        "anthropic",
                        "qwen",
                        "gemini",
                        "groq",
                        "mistral",
                        "copilot",
                    ];
                    for name in &known {
                        if let Some(key) = crate::keyring_util::get_api_key(name) {
                            set_env(name, &key);
                        }
                    }
                    // Sync language between GUI and backend
                    cmd.env("LANG", &config.language);
                    match cmd.spawn() {
                        Ok(child) => {
                            eprintln!("go-on 后端已启动 (PID: {})", child.id());
                            (BackendClient::new(&config.backend_url), Some(child))
                        }
                        Err(e) => {
                            eprintln!("警告: 启动后端失败: {}", e);
                            (BackendClient::new(&config.backend_url), None)
                        }
                    }
                }
                None => {
                    eprintln!("警告: 找不到 go-on 后端程序");
                    (BackendClient::new(&config.backend_url), None)
                }
            }
        } else {
            eprintln!("警告: 没有有效的 API Key，不启动后端");
            (BackendClient::new(&config.backend_url), None)
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
            // Internal tab IDs must stay stable (English keys); labels are localized in UI.
            active_tab: "monitor".to_string(),
            has_providers: providers_valid,
            backend_updates,
            backend_tx,
            pending_refresh: false,
            last_refresh: Instant::now(),
            backend_child,
            last_applied_theme: String::new(),
        }
    }

    fn current_lang(&self) -> Lang {
        match self.config.language.as_str() {
            "zh-CN" => Lang::ZhCn,
            "zh-TW" => Lang::ZhTw,
            _ => Lang::En,
        }
    }

    fn sync_backend_url(&mut self) {
        let config_url = self.config.backend_url.trim_end_matches('/').to_string();
        if self.backend.base_url() != config_url {
            self.backend.set_base_url(&config_url);
        }
    }

    fn poll_backend_updates(&mut self) {
        while let Ok(update) = self.backend_updates.try_recv() {
            match update {
                BackendUpdate::Health(h) => self.monitor_view.health = Some(h),
                BackendUpdate::Providers(p) => self.monitor_view.providers = p,
                BackendUpdate::RefreshDone => self.pending_refresh = false,
            }
        }
    }

    fn maybe_refresh_backend(&mut self) {
        if self.last_refresh.elapsed().as_secs() >= 5 && !self.pending_refresh {
            self.pending_refresh = true;
            let tx = self.backend_tx.clone();
            let backend = self.backend.clone();
            tokio::spawn(async move {
                // Add timeout to prevent hanging if backend is not responding
                let health =
                    match tokio::time::timeout(std::time::Duration::from_secs(5), backend.health())
                        .await
                    {
                        Ok(h) => h,
                        Err(_) => {
                            log_msg("Warning: Backend health check timed out");
                            HealthStatus {
                                connected: false,
                                healthy: false,
                                uptime: 0,
                                requests_per_minute: 0.0,
                                success_rate: 0.0,
                                avg_latency_ms: 0.0,
                            }
                        }
                    };
                let _ = tx.send(BackendUpdate::Health(health));

                let providers = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend.provider_status(),
                )
                .await
                {
                    Ok(p) => p,
                    Err(_) => {
                        log_msg("Warning: Backend provider status check timed out");
                        vec![]
                    }
                };
                let _ = tx.send(BackendUpdate::Providers(providers));
                let _ = tx.send(BackendUpdate::RefreshDone);
            });
            self.last_refresh = Instant::now();
        }
    }
}

impl eframe::App for GoOnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _frame_start = std::time::Instant::now();

        self.i18n.switch(self.current_lang());
        // Only apply theme when it actually changes — calling ctx.set_style() every
        // frame invalidates egui's text cache and forces a full re-layout each frame.
        if self.last_applied_theme != self.config.theme {
            self.last_applied_theme = self.config.theme.clone();
            let theme = crate::theme::Theme::from_name(&self.config.theme);
            theme.apply(ctx);
        }
        ctx.request_repaint_after(Duration::from_millis(200));

        // Reap zombie child if backend exited
        if let Some(ref mut child) = self.backend_child {
            match child.try_wait() {
                Ok(None) => {} // still running
                Ok(Some(status)) => {
                    eprintln!("go-on 后端已退出 (code: {:?})", status.code());
                    self.backend_child = None;
                }
                Err(e) => {
                    eprintln!("go-on 后端 wait 错误: {}", e);
                    self.backend_child = None;
                }
            }
        }

        // Setup screen
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

        self.sync_backend_url();
        self.poll_backend_updates();
        self.maybe_refresh_backend();
        self.has_providers = has_valid_providers(&self.config);

        let tabs = self.active_tabs_precomputed();
        if !tabs.contains(&self.active_tab) {
            self.active_tab = if tabs.contains(&"monitor".to_string()) {
                "monitor".to_string()
            } else {
                "settings".to_string()
            };
        }
        let is_connected = self
            .monitor_view
            .health
            .as_ref()
            .is_some_and(|h| h.connected);

        // Toolbar
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let title_color = if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(236, 241, 255)
                } else {
                    egui::Color32::from_rgb(19, 53, 110)
                };
                ui.label(
                    egui::RichText::new(self.i18n.t("app.title"))
                        .text_style(egui::TextStyle::Name("Title".into()))
                        .strong()
                        .color(title_color),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let status = if is_connected {
                        self.i18n.t("status.connected")
                    } else {
                        self.i18n.t("status.disconnected")
                    };
                    let status_color = if is_connected {
                        egui::Color32::from_rgb(32, 160, 95)
                    } else {
                        egui::Color32::from_rgb(198, 60, 60)
                    };
                    let pid_info = self
                        .backend_child
                        .as_ref()
                        .map(|c| format!("  PID:{}", c.id()))
                        .unwrap_or_default();

                    egui::Frame::new()
                        .fill(status_color.gamma_multiply(0.15))
                        .corner_radius(12.0)
                        .inner_margin(egui::Margin::symmetric(10i8, 4i8))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!("{}{}", status, pid_info))
                                    .color(status_color)
                                    .strong(),
                            );
                        });
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

        // Main content
        egui::CentralPanel::default().show(ctx, |ui| {
            let tab = self.active_tab.clone();
            let has_backend = self.has_providers;
            let monitor_history_alerts_enabled = self.config.features.monitor_history_alerts;
            let skills_lifecycle_enabled = self.config.features.skills_lifecycle;
            let workflow_run_center_enabled = self.config.features.workflow_run_center;
            let autotune_chain_enabled = self.config.features.autotune_chain_injection;
            let config_safe_mode_enabled = self.config.features.config_safe_mode;
            let providers_ops_enabled = self.config.features.providers_ops;
            match tab.as_str() {
                "monitor" => self.monitor_view.show(
                    ui,
                    &self.i18n,
                    has_backend,
                    &self.backend,
                    monitor_history_alerts_enabled,
                ),
                "chat" => {
                    self.chat_view
                        .show(ui, &self.i18n, &self.backend, ctx, autotune_chain_enabled);
                }
                "skills" => self.skills_view.show(
                    ui,
                    &self.i18n,
                    &self.backend,
                    ctx,
                    skills_lifecycle_enabled,
                ),
                "settings" => SettingsView::show(ui, &self.i18n, &mut self.config),
                "workflow" => self.workflow_view.show(
                    ui,
                    &self.i18n,
                    ctx,
                    &self.backend,
                    workflow_run_center_enabled,
                ),
                "autotune" => self.autotune_view.show(ui, &self.i18n),
                "security" => self.security_view.show(ui, &self.i18n, &self.backend, ctx),
                "config" => self.config_editor_view.show(
                    ui,
                    &self.i18n,
                    &mut self.config,
                    config_safe_mode_enabled,
                ),
                "providers" => self.providers_view.show(
                    ui,
                    &self.i18n,
                    &mut self.config,
                    &self.backend,
                    ctx,
                    providers_ops_enabled,
                ),
                _ => {
                    ui.heading(&tab);
                    ui.label(self.i18n.t("app.unknownTab"));
                }
            }
        });

        // Frame timing diagnostics — write to log file
        let frame_elapsed = _frame_start.elapsed();
        if frame_elapsed.as_millis() > 50 {
            log_msg(&format!(
                "FRAME_DIAG: [{}] took {}ms",
                self.active_tab,
                frame_elapsed.as_millis()
            ));
        }
    }
}

impl Drop for GoOnApp {
    fn drop(&mut self) {
        if let Some(mut child) = self.backend_child.take() {
            eprintln!("Shutting down go-on backend (PID: {})...", child.id());
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("go-on backend stopped");
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
