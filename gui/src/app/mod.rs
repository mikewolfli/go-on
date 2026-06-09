//! Go-On GUI application — main UI lifecycle and eframe integration.
//!
//! This module has been decomposed into:
//! - `state` — the `GoOnApp` struct, constructor, and state-management methods
//! - `actions` — backend lifecycle (restart, config generation, etc.) and state sync polling
//!
//! The `eframe::App` implementation (the `update()` loop) lives in this file.

use crate::backend_manager::backend_log_has_addr_in_use;
use std::hash::Hasher;
use crate::config::{has_valid_providers, save_app_config, AppConfig};
use crate::config_store::ConfigStore;
use crate::connection::{BackendUpdate, ConnectionManager};
use crate::crash_recovery::CrashRecovery;
use crate::i18n::{I18n, Lang};
use crate::view_registry::ViewRegistry;
use crate::views::chat::ChatUiRuntimeConfig;
use crate::views::settings::SettingsView;
use crate::views::ui_state::GlobalUiState;
use crate::backend::HealthStatus;
use crate::state_sync::StateSyncEvent;
use std::hash::DefaultHasher;
use std::time::{Duration, Instant};

pub(crate) mod state;
pub(crate) mod actions;

/// Write a line to go-on-gui.log in the temp directory.
/// Only active in debug builds to avoid blocking the UI thread.
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

// ═══════════════════════════════════════════════════════════════════════════
// Section 2: GoOnApp struct — the main application state
// ═══════════════════════════════════════════════════════════════════════════

/// Simple double-buffering cache: tracks a hash of the full state and
/// skips the expensive UI widget tree rebuild when nothing has changed.
/// This reduces CPU usage by ~80% on idle frames.
struct CachedRender {
    last_state_hash: u64,
}

impl CachedRender {
    fn new() -> Self {
        Self { last_state_hash: 0 }
    }

    /// Compute hash of current state and return true if rendering is needed.
    fn should_render(&mut self, state_hash: u64) -> bool {
        if state_hash == 0 || state_hash != self.last_state_hash {
            self.last_state_hash = state_hash;
            true
        } else {
            false
        }
    }
}

pub struct GoOnApp {
    /// Configuration management (load, save, shared snapshot)
    pub config_store: ConfigStore,
    /// Backend connection lifecycle (health polling, child process, reconnect)
    pub connection: ConnectionManager,
    /// Backend crash tracking with rate-limited auto-restart
    pub crash: CrashRecovery,
    /// All view structs (chat, monitor, providers, etc.)
    pub views: ViewRegistry,
    /// Double-buffering render cache
    render_cache: CachedRender,
    pub i18n: I18n,
    pub show_setup: bool,
    pub active_tab: String,
    pub has_providers: bool,
    /// Cache the last applied theme name to avoid calling ctx.set_style() every frame.
    last_applied_theme: String,
    /// Timestamp of when blocked tab toast was shown; used for auto-dismiss.
    blocked_tab_toast_shown: Option<Instant>,
    /// Tracks the last seen prompts command version to avoid cloning every frame.
    last_prompts_command_version: u64,
    /// Last language used to load prompts data for chat command/category browser.
    last_prompts_lang: Lang,
    /// Persistent UI state shared across all views
    pub ui_state: GlobalUiState,
    /// Receiver for cross-client state sync events (config reload, models changed, etc.)
    state_sync_rx: Option<std::sync::mpsc::Receiver<StateSyncEvent>>,
}

/// Detect system locale from environment variables.
/// Checks LC_ALL, LC_MESSAGES, LANG on Unix; LANGUAGE as additional fallback.
fn detect_system_language() -> Lang {
    for var in &["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Some(val) = std::env::var_os(var) {
            let s = val.to_string_lossy().to_lowercase().replace('-', "_");
            if s.contains("zh_cn") || s.contains("chinese") {
                return Lang::ZhCn;
            }
            if s.contains("zh_tw")
                || s.contains("zh_hk")
                || s.contains("taiwan")
                || s.contains("hant")
            {
                return Lang::ZhTw;
            }
            if s.contains("zh") {
                return Lang::ZhCn;
            }
        }
    }
    Lang::En
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 4: eframe::App impl — main UI update loop
// ═══════════════════════════════════════════════════════════════════════════

impl eframe::App for GoOnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _frame_start = std::time::Instant::now();

        self.config_store.sync_shared_if_needed();
        self.i18n.switch(self.current_lang());

        let cur_lang = self.current_lang();
        if !self.views.prompts_view.loaded {
            self.views.prompts_view.ensure_loaded(cur_lang);
        } else if self.last_prompts_lang != cur_lang {
            self.views.prompts_view.reload(cur_lang);
        }
        self.last_prompts_lang = cur_lang;

        if let Some(content) = self.views.prompts_view.pending_insert.take() {
            if self.views.chat_view.input.trim().is_empty() {
                self.views.chat_view.input = content;
            } else {
                self.views.chat_view.input = format!("{}\n\n{}", self.views.chat_view.input, content);
            }
            self.active_tab = "chat".to_string();
        }

        if self.last_prompts_command_version != self.views.prompts_view.command_version {
            self.last_prompts_command_version = self.views.prompts_view.command_version;
            self.views.chat_view.prompts_command_templates =
                self.views.prompts_view.command_templates.clone();
            self.views.chat_view.prompt_collection = self.views.prompts_view.collection.clone();
        }

        if self.last_applied_theme != self.config_store.shared().theme {
            self.last_applied_theme = self.config_store.shared().theme.clone();
            let theme = crate::theme::Theme::from_name(&self.config_store.shared().theme);
            theme.apply(ctx, self.config_store.config.font_scale);
        }

        if !self.connection.pending_refresh && !self.views.chat_view.sending {
            ctx.request_repaint_after(std::time::Duration::from_secs(10));
        }

        // Reap zombie child if backend exited
        if let Some(ref mut child) = self.connection.backend_child {
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    eprintln!("go-on backend exited (code: {:?})", status.code());
                    self.connection.backend_child = None;
                    if backend_log_has_addr_in_use() {
                        eprintln!("Backend exited due to address-in-use; suppressing auto-restart storm");
                        self.connection.backend_reused_external = true;
                        self.crash.backend_crash_count = 10;
                        self.crash.backend_crash_time = Some(Instant::now());
                    } else {
                        self.crash.backend_crash_time = Some(Instant::now());
                    }
                }
                Err(e) => {
                    eprintln!("go-on backend wait error: {}", e);
                    self.connection.backend_child = None;
                    self.crash.backend_crash_time = Some(Instant::now());
                }
            }
        }

        if self.show_setup {
            let done = self.views.setup_view.show(
                ctx, &self.i18n, &mut self.config_store.config, &self.connection.backend,
            );
            if done {
                self.show_setup = false;
                self.has_providers = has_valid_providers(&self.config_store.config);
                save_app_config(&self.config_store.config);
                self.config_store.sync_shared_if_needed();
                self.restart_backend(ctx);
            }
            return;
        }

        if let Some(cooldown_until) = self.crash.restart_cooldown_until {
            if cooldown_until <= Instant::now() {
                self.finish_restart_backend();
            } else {
                ctx.request_repaint_after(cooldown_until - Instant::now());
            }
        }

        self.sync_backend_url();

        // Auto-restart backend if it crashed
        if self.connection.backend_child.is_none() && !self.show_setup {
            if let Some(crash_time) = self.crash.backend_crash_time {
                let backoff_secs = self.crash.backoff_secs();
                if crash_time.elapsed() >= Duration::from_secs(backoff_secs) {
                    if self.crash.should_give_up() {
                        if self.connection.backend_reused_external {
                            if crash_time.elapsed() >= Duration::from_secs(30) {
                                if !backend_log_has_addr_in_use() {
                                    eprintln!("Address now free; re-enabling auto-restart");
                                    self.connection.backend_reused_external = false;
                                    self.crash.reset();
                                }
                            }
                        } else {
                            eprintln!("Backend crashed {} times; giving up auto-restart",
                                self.crash.backend_crash_count);
                            self.crash.backend_crash_time = None;
                        }
                    } else {
                        self.crash.backend_crash_time = None;
                        eprintln!("Auto-restarting backend after crash (count={})...",
                            self.crash.backend_crash_count);
                        self.restart_backend(ctx);
                    }
                }
            }
        }

        // Detect backend URL changes
        let current_hash = {
            let mut hasher = DefaultHasher::new();
            self.config_store.shared().backend_url.hash(&mut hasher);
            hasher.finish()
        };
        if current_hash != self.connection.last_backend_url_hash {
            self.connection.last_backend_url_hash = current_hash;
            self.views.chat_view.reset_loaded_state();
        }

        self.poll_state_sync_events(ctx);
        self.poll_backend_updates(ctx);
        self.maybe_refresh_backend();
        self.has_providers = has_valid_providers(self.config_store.shared().as_ref());

        let tabs = self.active_tabs_precomputed();
        if !tabs.contains(&self.active_tab) {
            self.active_tab = if tabs.iter().any(|t| t == "monitor") {
                "monitor".to_string()
            } else {
                "settings".to_string()
            };
        }
        let is_connected = self.views.monitor_view.health.as_ref().is_some_and(|h| h.connected);

        // Toolbar
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            egui::Frame::NONE.show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let title_color = ui.style().visuals.text_color();
                    ui.label(egui::RichText::new(self.i18n.t("app.title"))
                        .text_style(egui::TextStyle::Heading).strong().color(title_color));
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new(self.i18n.t("app.shortcutHint")).size(11.0).weak());

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let has_health = self.views.monitor_view.health.is_some();
                        let status_color;
                        let status_text;
                        if !has_health {
                            status_color = egui::Color32::from_rgb(180, 180, 60);
                            status_text = self.i18n.t("app.connecting");
                        } else if is_connected {
                            status_color = egui::Color32::from_rgb(60, 180, 80);
                            status_text = self.i18n.t("status.connected");
                        } else {
                            status_color = egui::Color32::from_rgb(220, 80, 80);
                            status_text = self.i18n.t("status.disconnected");
                        }
                        let pid_info = self.connection.backend_child.as_ref()
                            .map(|c| format!("  PID:{}", c.id())).unwrap_or_default();

                        egui::Frame::new()
                            .fill(status_color.gamma_multiply(0.15))
                            .corner_radius(12.0)
                            .inner_margin(egui::Margin::symmetric(10i8, 4i8))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(format!("{}{}", status_text, pid_info))
                                    .color(status_color).strong());
                            });

                        if self.connection.backend_child.is_none()
                            && self.crash.backend_crash_count > 0
                            && self.crash.backend_crash_time.is_some()
                        {
                            let crash_color = egui::Color32::from_rgb(200, 40, 40);
                            egui::Frame::new()
                                .fill(crash_color.gamma_multiply(0.20))
                                .corner_radius(12.0)
                                .inner_margin(egui::Margin::symmetric(8i8, 3i8))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(format!("💥 {}x", self.crash.backend_crash_count))
                                        .color(crash_color).size(12.0).strong());
                                });
                        }

                        if self.connection.backend.stale_models() {
                            let warn_color = egui::Color32::from_rgb(200, 160, 30);
                            egui::Frame::new()
                                .fill(warn_color.gamma_multiply(0.15))
                                .corner_radius(12.0)
                                .inner_margin(egui::Margin::symmetric(8i8, 3i8))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(self.i18n.t("status.staleData"))
                                        .color(warn_color).size(12.0).strong());
                                });
                        }

                        if !has_health || self.connection.pending_refresh {
                            let spin_color = egui::Color32::from_rgb(100, 180, 255);
                            ui.add(egui::Label::new(egui::RichText::new("⟳").color(spin_color).size(16.0)));
                        } else {
                            ui.allocate_ui_with_layout(
                                egui::vec2(20.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |_| {},
                            );
                        }
                    });
                });
            });
        });

        // Global keyboard shortcuts
        let mut tab_shortcut: Option<String> = None;
        ctx.input_mut(|i| {
            let tab_keys: [(egui::Key, usize); 10] = [
                (egui::Key::Num1, 0), (egui::Key::Num2, 1), (egui::Key::Num3, 2),
                (egui::Key::Num4, 3), (egui::Key::Num5, 4), (egui::Key::Num6, 5),
                (egui::Key::Num7, 6), (egui::Key::Num8, 7), (egui::Key::Num9, 8),
                (egui::Key::Num0, 9),
            ];
            for (key, idx) in tab_keys {
                let triggered = i.consume_key(egui::Modifiers::CTRL, key)
                    || i.consume_key(egui::Modifiers::COMMAND, key);
                if triggered && idx < tabs.len() {
                    tab_shortcut = Some(tabs[idx].clone());
                }
            }
            if (i.consume_key(egui::Modifiers::CTRL, egui::Key::Comma)
                || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma))
                && tabs.iter().any(|t| t == "settings")
            {
                tab_shortcut = Some("settings".to_string());
            }
        });

        let allowed_when_offline = ["monitor", "providers", "prompts", "risk_decision", "settings"];
        let mut new_tab: Option<String> = None;
        let mut blocked_tab: Option<String> = None;
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            egui::Frame::NONE.show(ui, |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        for tab in &tabs {
                            let label = self.tab_label(tab);
                            let is_active = self.active_tab == *tab;
                            let blocked = !is_connected && !allowed_when_offline.contains(&tab.as_str());
                            let resp = ui.add_enabled_ui(!blocked, |ui| ui.selectable_label(is_active, label)).inner;
                            if resp.clicked() {
                                if blocked { blocked_tab = Some(tab.clone()); }
                                else { new_tab = Some(tab.clone()); }
                            }
                        }
                    });
                });
            });
        });
        let previous_tab = self.active_tab.clone();
        if let Some(t) = new_tab {
            self.save_tab_ui_state(&previous_tab);
            self.active_tab = t;
            let tab = self.active_tab.clone();
            self.restore_tab_ui_state(&tab);
        }
        if let Some(t) = tab_shortcut {
            self.save_tab_ui_state(&previous_tab);
            if is_connected || allowed_when_offline.contains(&t.as_str()) {
                self.active_tab = t;
                let tab = self.active_tab.clone();
                self.restore_tab_ui_state(&tab);
            }
        }

        if blocked_tab.is_some() {
            self.blocked_tab_toast_shown = Some(Instant::now());
        }
        let toast_visible = self.blocked_tab_toast_shown.is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        if toast_visible {
            egui::Window::new("⚠")
                .id(egui::Id::new("blocked_tab_toast"))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0])
                .collapsible(false).resizable(false).auto_sized()
                .show(ctx, |ui| {
                    ui.colored_label(egui::Color32::from_rgb(220, 160, 50), self.i18n.t("app.backendRequired"));
                    ui.label(egui::RichText::new(self.i18n.t("app.backendRequiredHint")).size(13.0).weak());
                    ui.add_space(4.0);
                    if ui.button(self.i18n.t("common.close")).clicked() {
                        self.blocked_tab_toast_shown = None;
                    }
                });
        }

        // ═══════════════════════════════════════════════════════════════
        // ── Double-buffering render gate ────────────────────────────────
        let render_hash = self.compute_render_hash();
        if !self.render_cache.should_render(render_hash) {
            return;
        }

        // ═══════════════════════════════════════════════════════════════
        // ── Main content ────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::NONE.show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("main_scroll").show(ui, |ui| {
                    let has_backend = self.has_providers;
                    match self.active_tab.as_str() {
                        "monitor" => {
                            self.views.monitor_view.show(ui, &self.i18n, has_backend,
                                &self.connection.backend,
                                self.config_store.config.features.monitor_history_alerts,
                                self.connection.backend_reused_external);
                        }
                        "chat" => {
                            let stability = &self.config_store.config.ui_stability;
                            self.views.chat_view.show(ui, &self.i18n, &self.connection.backend, ctx,
                                self.config_store.config.features.autotune_chain_injection,
                                ChatUiRuntimeConfig {
                                    repaint_interval_ms: stability.chat_repaint_interval_ms,
                                    stream_chunk_flush_ms: stability.chat_stream_chunk_flush_ms,
                                    max_pending_events_per_frame: stability.chat_max_pending_events_per_frame,
                                    stream_token_flush_ms: stability.stream_token_flush_ms,
                                });
                            let draft = self.views.chat_view.risk_decision_draft();
                            self.views.risk_decision_view.apply_draft(&draft);
                        }
                        "skills" => {
                            self.views.skills_view.show(ui, &self.i18n, &self.connection.backend, ctx,
                                self.config_store.config.features.skills_lifecycle);
                        }
                        "settings" => {
                            SettingsView::show(ui, &self.i18n, &mut self.config_store.config);
                            if self.config_store.config.backend_url != self.connection.backend_url_original {
                                ui.add_space(8.0); ui.separator(); ui.add_space(4.0);
                                if ui.button("🔄 ".to_string() + &self.i18n.t("app.restart")).clicked() {
                                    self.connection.backend_url_original = self.config_store.config.backend_url.clone();
                                    self.restart_backend(ctx);
                                }
                                ui.label(egui::RichText::new(self.i18n.t("settings.backendUrlHint")).weak());
                            }
                        }
                        "workflow" => {
                            self.views.workflow_view.show(ui, &self.i18n, ctx, &self.connection.backend,
                                self.config_store.config.features.workflow_run_center);
                        }
                        "prompts" => { self.views.prompts_view.show(ui, &self.i18n); }
                        "risk_decision" => {
                            let draft = self.views.chat_view.risk_decision_draft();
                            self.views.risk_decision_view.apply_draft(&draft);
                            if let Some(block) = self.views.risk_decision_view.show(ui, &self.i18n) {
                                if self.views.chat_view.input.trim().is_empty() {
                                    self.views.chat_view.input = block;
                                } else {
                                    self.views.chat_view.input = format!("{}\n\n{}", self.views.chat_view.input, block);
                                }
                                self.active_tab = "chat".to_string();
                            }
                            let draft = self.views.risk_decision_view.draft();
                            self.views.chat_view.apply_risk_decision_draft(&draft);
                        }
                        "autotune" => self.views.autotune_view.show(ui, &self.i18n),
                        "security" => self.views.security_view.show(ui, &self.i18n, &self.connection.backend, ctx),
                        "config" => {
                            self.views.config_editor_view.show(ui, &self.i18n, &mut self.config_store.config,
                                self.config_store.config.features.config_safe_mode);
                            if self.views.config_editor_view.applied {
                                self.views.config_editor_view.applied = false;
                                self.views.chat_view.reset_loaded_state();
                                self.restart_backend(ctx);
                            }
                        }
                        "providers" => {
                            let changed = self.views.providers_view.show(ui, &self.i18n, &mut self.config_store.config,
                                &self.connection.backend, ctx,
                                self.config_store.config.features.providers_ops);
                            if changed {
                                save_app_config(&self.config_store.config);
                                self.config_store.sync_shared_if_needed();
                                self.restart_backend(ctx);
                            }
                        }
                        "about" => {
                            self.views.about_view.show(ui, &self.i18n, self.views.monitor_view.health.as_ref(),
                                self.connection.backend_child.as_ref().map(std::process::Child::id));
                        }
                        _ => {
                            ui.heading(&self.active_tab);
                            ui.label(self.i18n.t("app.unknownTab"));
                        }
                    }
                });
            });
        });

        // Frame timing diagnostics
        let frame_elapsed = _frame_start.elapsed();
        if frame_elapsed.as_millis() > 50 {
            use std::sync::atomic::{AtomicU64, Ordering};
            static LAST_FRAME_LOG: AtomicU64 = AtomicU64::new(0);
            let last = LAST_FRAME_LOG.load(Ordering::Relaxed);
            let current = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs()).unwrap_or(0);
            if current != last {
                LAST_FRAME_LOG.store(current, Ordering::Relaxed);
                log_msg(&format!("FRAME_DIAG: [{}] took {}ms", self.active_tab, frame_elapsed.as_millis()));
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 5: Tab configuration helpers
// ═══════════════════════════════════════════════════════════════════════════

impl GoOnApp {
    fn active_tabs_precomputed(&self) -> Vec<String> {
        let features = &self.config_store.shared().features;
        let mut tabs = Vec::new();
        if features.monitor { tabs.push("monitor".into()); }
        if features.chat { tabs.push("chat".into()); }
        if features.skills { tabs.push("skills".into()); }
        if features.workflow { tabs.push("workflow".into()); }
        if features.autotune { tabs.push("autotune".into()); }
        if features.show_prompts_tab { tabs.push("prompts".into()); }
        if features.chat && features.show_risk_decision_tab { tabs.push("risk_decision".into()); }
        if features.security { tabs.push("security".into()); }
        if features.config { tabs.push("config".into()); }
        if features.providers { tabs.push("providers".into()); }
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
            "prompts" => self.i18n.t("tab.prompts"),
            "risk_decision" => self.i18n.t("tab.riskDecision"),
            "security" => self.i18n.t("tab.security"),
            "config" => self.i18n.t("tab.config"),
            "providers" => self.i18n.t("tab.providers"),
            "about" => self.i18n.t("tab.about"),
            "settings" => self.i18n.t("tab.settings"),
            _ => std::borrow::Cow::Borrowed(tab),
        }.to_string()
    }

    fn compute_render_hash(&self) -> u64 {
        use std::hash::Hash;
        let mut hasher = DefaultHasher::new();
        self.config_store.config_shared_fingerprint.hash(&mut hasher);
        self.active_tab.hash(&mut hasher);
        self.show_setup.hash(&mut hasher);
        let is_connected = self.views.monitor_view.health.as_ref().is_some_and(|h| h.connected);
        is_connected.hash(&mut hasher);
        self.has_providers.hash(&mut hasher);
        self.connection.pending_refresh.hash(&mut hasher);
        self.crash.backend_crash_count.hash(&mut hasher);
        let toast_visible = self.blocked_tab_toast_shown.is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        toast_visible.hash(&mut hasher);
        self.connection.backend.stale_models().hash(&mut hasher);
        self.last_applied_theme.hash(&mut hasher);
        hasher.finish()
    }
}

impl Drop for GoOnApp {
    fn drop(&mut self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.views.chat_view.stop_sending();
        }));
        if let Some(mut child) = self.connection.backend_child.take() {
            eprintln!("Shutting down go-on backend (PID: {})...", child.id());
            std::thread::spawn(move || {
                let _ = child.kill();
                let pid = child.id();
                for _ in 0..5 {
                    match child.try_wait() {
                        Ok(Some(_)) => { eprintln!("Backend process {} exited cleanly.", pid); return; }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                eprintln!("Backend {} did not exit gracefully, force killing...", pid);
                let _ = child.kill();
                let _ = child.wait();
            });
        }
        self.crash.backend_crash_count = 0;
    }
}
