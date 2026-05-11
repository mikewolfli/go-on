use crate::backend::BackendClient;
use crate::config::{has_valid_providers, save_app_config, AppConfig};

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
    about::AboutView, autotune::AutoTuneView, chat::ChatView, config_editor::ConfigEditorView,
    monitor::MonitorView, providers::ProvidersView, security::SecurityView, settings::SettingsView,
    setup::SetupView, skills::SkillsView, workflow::WorkflowView,
};
use std::hash::{Hash, Hasher};
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

enum BackendUpdate {
    Health(HealthStatus),
    Providers(Vec<ProviderStatus>),
    RefreshDone,
}

use crate::backend::{HealthStatus, ProviderStatus};
use std::collections::hash_map::DefaultHasher;

/// Find the go-on backend binary path relative to the GUI executable.
fn find_backend_binary() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe_name = if cfg!(target_os = "windows") {
        "go-on.exe"
    } else {
        "go-on"
    };
    let mut candidates = vec![
        exe_dir.join("backend").join(exe_name),
        exe_dir.join(exe_name),
    ];
    // Also search in Resources/backend (macOS .app bundle layout)
    if let Some(resources) = exe_dir.parent().map(|p| p.join("Resources")) {
        candidates.push(resources.join("backend").join(exe_name));
        candidates.push(resources.join(exe_name));
    }
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
    pub about_view: AboutView,
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
    /// Track when the backend crashed to enable auto-restart with rate limiting
    backend_crash_time: Option<Instant>,
    /// Hash of backend URL to detect changes for cache invalidation
    last_backend_url_hash: u64,
    /// Original backend URL to detect changes for showing restart button
    backend_url_original: String,
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
    /// Print diagnostic info about key sources for debugging.
    fn diagnostic_key_report(config: &AppConfig) {
        eprintln!("=== KEY DIAGNOSTIC ===");
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
            let config_key = config
                .providers
                .iter()
                .find(|p| p.name.to_lowercase() == *name)
                .map(|p| {
                    if p.api_key.is_empty() {
                        "(empty)".to_string()
                    } else {
                        format!("{}...", &p.api_key[..4.min(p.api_key.len())])
                    }
                })
                .unwrap_or_else(|| "(not in config)".to_string());
            let keyring_key = crate::keyring_util::get_api_key(name)
                .map(|k| format!("{}...", &k[..4.min(k.len())]))
                .unwrap_or_else(|| "(not in keyring)".to_string());
            eprintln!("  {}: config={}, keyring={}", name, config_key, keyring_key);
        }
        eprintln!("=== END DIAGNOSTIC ===");
    }

    /// Start or restart the backend child process with fresh env vars from keyring.
    fn spawn_backend(config: &AppConfig) -> (BackendClient, Option<std::process::Child>) {
        Self::diagnostic_key_report(config);
        match find_backend_binary() {
            Some(path) => {
                let config_dir = path.parent().unwrap_or(std::path::Path::new("."));
                let mut cmd = std::process::Command::new(&path);
                cmd.current_dir(config_dir)
                    .arg("--protocol-mode")
                    .arg("acp_http")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());

                // Inject API keys for ALL configured providers into backend process environment.
                // Priority: keyring > config file > inherited env.
                for provider in &config.providers {
                    let provider_lower = provider.name.to_lowercase();
                    let derived_var = format!("{}_API_KEY", provider_lower.to_uppercase());
                    let env_var = match provider_lower.as_str() {
                        "copilot" => "GITHUB_COPILOT_TOKEN",
                        "replicate" => "REPLICATE_API_TOKEN",
                        _ => &derived_var,
                    };

                    // Try keyring first
                    let mut key = crate::keyring_util::get_api_key(&provider_lower);

                    // Fallback: config file api_key
                    if key.is_none() {
                        let k = provider.api_key.clone();
                        if !k.is_empty() && k != "********" {
                            key = Some(k);
                        }
                    }

                    // Fallback: inherited env var
                    if key.is_none() {
                        key = std::env::var(env_var).ok().filter(|v| !v.is_empty());
                    }

                    if let Some(k) = key {
                        let preview = if k.len() > 4 {
                            format!("{}...", &k[..4])
                        } else {
                            "[short]".to_string()
                        };
                        eprintln!("backend: set {}={}", env_var, preview);
                        cmd.env(env_var, k);
                    } else {
                        eprintln!("backend: no key found for '{}'", provider_lower);
                    }
                }

                // Sync language between GUI and backend
                cmd.env("LANG", &config.language);

                match cmd.spawn() {
                    Ok(child) => {
                        eprintln!("go-on backend started (PID: {})", child.id());
                        (BackendClient::new(&config.backend_url), Some(child))
                    }
                    Err(e) => {
                        eprintln!("warning: failed to start backend: {}", e);
                        (BackendClient::new(&config.backend_url), None)
                    }
                }
            }
            None => {
                eprintln!("warning: go-on backend binary not found");
                (BackendClient::new(&config.backend_url), None)
            }
        }
    }

    /// Kill the current backend child and start a new one with fresh env vars.
    /// Called after adding/updating API keys so the new keys take effect immediately.
    fn restart_backend(&mut self) {
        // Kill old process
        if let Some(mut child) = self.backend_child.take() {
            eprintln!("Restarting backend (old PID: {})...", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
        // Start new
        let (backend, child) = Self::spawn_backend(&self.config);
        self.backend = backend;
        self.backend_child = child;
        // Force immediate refresh on next update() cycle
        self.pending_refresh = false;
        self.last_refresh = Instant::now() - std::time::Duration::from_secs(10);
        // Clear stale health/providers so monitor shows correct state
        self.monitor_view.health = None;
        self.monitor_view.providers = Vec::new();
        // Also reset chat cache so it re-fetches models from new backend
        self.chat_view.reset_loaded_state();
        // Reset providers loaded state so models are re-fetched
        self.providers_view.reset_loaded_state();
        eprintln!("Backend restarted");
    }

    pub fn new() -> Self {
        let config = crate::config::load_app_config();
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

        // Start backend with env vars from keyring
        let (backend, backend_child) = Self::spawn_backend(&config);

        // Compute hash before moving config
        let initial_url_hash = {
            let mut hasher = DefaultHasher::new();
            config.backend_url.hash(&mut hasher);
            hasher.finish()
        };
        let initial_url = config.backend_url.clone();

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
            about_view: AboutView::new(),
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
            backend_crash_time: None,
            last_backend_url_hash: initial_url_hash,
            backend_url_original: initial_url,
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
                                backend_version: None,
                                backend_build: None,
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
        ctx.request_repaint_after(Duration::from_millis(500));

        // Reap zombie child if backend exited
        if let Some(ref mut child) = self.backend_child {
            match child.try_wait() {
                Ok(None) => {} // still running
                Ok(Some(status)) => {
                    eprintln!("go-on 后端已退出 (code: {:?})", status.code());
                    self.backend_child = None;
                    self.backend_crash_time = Some(Instant::now());
                }
                Err(e) => {
                    eprintln!("go-on 后端 wait 错误: {}", e);
                    self.backend_child = None;
                    self.backend_crash_time = Some(Instant::now());
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
                // Restart backend so it picks up the new API key from env
                self.restart_backend();
            }
            ctx.request_repaint();
            return;
        }

        self.sync_backend_url();

        // Auto-restart backend if it crashed and enough time has passed
        if self.backend_child.is_none() && !self.show_setup {
            if let Some(crash_time) = self.backend_crash_time {
                // Wait 3 seconds before auto-restart to avoid rapid restart loops
                if crash_time.elapsed() >= Duration::from_secs(3) {
                    self.backend_crash_time = None;
                    eprintln!("Auto-restarting backend after crash...");
                    self.restart_backend();
                }
            }
        }

        // Detect backend URL changes and reset chat cache
        let current_hash = {
            let mut hasher = DefaultHasher::new();
            self.config.backend_url.hash(&mut hasher);
            hasher.finish()
        };
        if current_hash != self.last_backend_url_hash {
            self.last_backend_url_hash = current_hash;
            self.chat_view.reset_loaded_state();
        }

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

                // Pending refresh spinner
                if self.pending_refresh {
                    let secs = _frame_start.elapsed().as_secs_f64();
                    let alpha = ((secs * 4.0).sin() * 0.5 + 0.5) as f32;
                    let color = egui::Color32::from_rgba_premultiplied(
                        100,
                        180,
                        255,
                        (alpha * 255.0) as u8,
                    );
                    ui.add(egui::Label::new(
                        egui::RichText::new("⟳").color(color).size(16.0),
                    ));
                }
            });
        });

        // Global keyboard shortcuts for tab switching
        let mut tab_shortcut: Option<String> = None;
        ctx.input_mut(|i| {
            // Ctrl+1 through Ctrl+9 -> tabs[0..8], Ctrl+0 -> tabs[9]
            let tab_keys: [(egui::Key, usize); 10] = [
                (egui::Key::Num1, 0),
                (egui::Key::Num2, 1),
                (egui::Key::Num3, 2),
                (egui::Key::Num4, 3),
                (egui::Key::Num5, 4),
                (egui::Key::Num6, 5),
                (egui::Key::Num7, 6),
                (egui::Key::Num8, 7),
                (egui::Key::Num9, 8),
                (egui::Key::Num0, 9),
            ];
            for (key, idx) in tab_keys {
                if i.consume_key(egui::Modifiers::CTRL, key) && idx < tabs.len() {
                    tab_shortcut = Some(tabs[idx].clone());
                }
            }
            // Ctrl+, for settings
            if i.consume_key(egui::Modifiers::CTRL, egui::Key::Comma)
                && tabs.contains(&"settings".to_string())
            {
                tab_shortcut = Some("settings".to_string());
            }
        });

        // Tab bar
        let mut new_tab: Option<String> = None;
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                for tab in &tabs {
                    let label = self.tab_label(tab);
                    let is_active = self.active_tab == *tab;

                    let shortcut_hint = match tab.as_str() {
                        "monitor" => "Ctrl+1",
                        "chat" => "Ctrl+2",
                        "skills" => "Ctrl+3",
                        "workflow" => "Ctrl+4",
                        "autotune" => "Ctrl+5",
                        "security" => "Ctrl+6",
                        "config" => "Ctrl+7",
                        "providers" => "Ctrl+8",
                        "about" => "",
                        "settings" => "Ctrl+9",
                        _ => "",
                    };

                    let resp = egui::Frame::new()
                        .fill(if is_active {
                            if ctx.style().visuals.dark_mode {
                                egui::Color32::from_rgb(45, 50, 60)
                            } else {
                                egui::Color32::from_rgb(220, 224, 232)
                            }
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(10i8, 4i8))
                        .show(ui, |ui| ui.selectable_label(is_active, label))
                        .response;

                    let resp = if !shortcut_hint.is_empty() {
                        resp.on_hover_text(format!("{} ({})", self.tab_label(tab), shortcut_hint))
                    } else {
                        resp
                    };

                    if resp.clicked() {
                        new_tab = Some(tab.clone());
                    }
                }
            });
        });
        if let Some(t) = new_tab {
            self.active_tab = t;
        }
        if let Some(t) = tab_shortcut {
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
                "settings" => {
                    SettingsView::show(ui, &self.i18n, &mut self.config);
                    // Show restart button if backend URL changed
                    if self.config.backend_url != self.backend_url_original {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        if ui
                            .button("🔄 ".to_string() + &self.i18n.t("app.restart"))
                            .clicked()
                        {
                            self.backend_url_original = self.config.backend_url.clone();
                            self.restart_backend();
                        }
                        ui.label(
                            egui::RichText::new(self.i18n.t("settings.backendUrlHint")).weak(),
                        );
                    }
                }
                "workflow" => self.workflow_view.show(
                    ui,
                    &self.i18n,
                    ctx,
                    &self.backend,
                    workflow_run_center_enabled,
                ),
                "autotune" => self.autotune_view.show(ui, &self.i18n),
                "security" => self.security_view.show(ui, &self.i18n, &self.backend, ctx),
                "config" => {
                    self.config_editor_view.show(
                        ui,
                        &self.i18n,
                        &mut self.config,
                        config_safe_mode_enabled,
                    );
                    // Config changes may affect backend connectivity — reset chat cache only on apply
                    if self.config_editor_view.applied {
                        self.config_editor_view.applied = false;
                        self.chat_view.reset_loaded_state();
                        // Check if backend URL changed
                        if self.config.backend_url != self.backend_url_original {
                            self.backend_url_original = self.config.backend_url.clone();
                            self.restart_backend();
                        }
                    }
                }
                "providers" => {
                    let changed = self.providers_view.show(
                        ui,
                        &self.i18n,
                        &mut self.config,
                        &self.backend,
                        ctx,
                        providers_ops_enabled,
                    );
                    if changed {
                        save_app_config(&self.config);
                        // Providers page may have added/updated API keys in keyring.
                        // Restart backend so it picks up the new keys.
                        self.restart_backend();
                    }
                }
                "about" => {
                    self.about_view.show(
                        ui,
                        &self.i18n,
                        self.monitor_view.health.as_ref(),
                        self.backend_child.as_ref().map(std::process::Child::id),
                    );
                }
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
        tabs.push("about".into());
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
            "about" => self.i18n.t("tab.about"),
            "settings" => self.i18n.t("tab.settings"),
            _ => std::borrow::Cow::Borrowed(tab),
        }
        .to_string()
    }
}
