use crate::backend::{BackendClient, HealthStatus, ProviderStatus};
use crate::i18n::I18n;

pub struct MonitorView {
    pub health: Option<HealthStatus>,
    pub providers: Vec<ProviderStatus>,
    pub backend_configured: bool,
    error: String,
    restarting: bool,
}

impl MonitorView {
    pub fn new() -> Self {
        Self {
            health: None,
            providers: Vec::new(),
            backend_configured: false,
            error: String::new(),
            restarting: false,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, i18n: &I18n, backend_configured: bool, backend: &BackendClient) {
        self.backend_configured = backend_configured;

        ui.heading(i18n.t("monitor.health"));
        ui.separator();
        ui.add_space(8.0);

        // ── Health card ──────────────────────────────────────────
        egui::Frame::group(ui.style()).show(ui, |ui| {
            match &self.health {
                None => {
                    // No health info yet – backend may not be running
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::GRAY, "◌");
                        ui.label(i18n.t("app.connecting"));
                    });
                }
                Some(health) => {
                    ui.horizontal(|ui| {
                        let (icon, color, text) = if !health.connected {
                            ("◌", egui::Color32::RED, i18n.t("monitor.offline"))
                        } else if health.healthy {
                            ("●", egui::Color32::GREEN, i18n.t("monitor.healthy"))
                        } else {
                            ("◉", egui::Color32::YELLOW, i18n.t("monitor.unhealthy"))
                        };
                        ui.colored_label(color, icon);
                        ui.colored_label(color, text);
                    });
                    ui.label(format!(
                        "{}: {} ms",
                        i18n.t("monitor.latency"),
                        health.avg_latency_ms
                    ));
                    ui.label(format!(
                        "{}: {}% (uptime: {}s)",
                        i18n.t("monitor.success"),
                        health.success_rate,
                        health.uptime
                    ));
                    ui.label(format!(
                        "{}: {:.1}",
                        i18n.t("monitor.rpm"),
                        health.requests_per_minute
                    ));
                }
            }
        });

        ui.add_space(16.0);

        // ── Configured-but-backend-offline hint ────────────────
        if self.backend_configured && self.health.as_ref().is_none_or(|h| !h.connected) {
            ui.colored_label(
                egui::Color32::from_rgb(200, 160, 60),
                i18n.t("monitor.offlineHint"),
            );
            ui.add_space(8.0);
        }

        // ── Provider status ─────────────────────────────────────
        ui.heading(i18n.t("monitor.providers"));
        ui.separator();
        ui.add_space(4.0);

        if self.providers.is_empty() {
            // Show a more descriptive message when health is known but no providers
            match &self.health {
                Some(h) if h.connected => {
                    ui.label(i18n.t("monitor.notReady"));
                }
                _ => {
                    // Either no health data or backend offline – no point showing
                    // empty providers as "not ready"; just be silent.
                }
            }
        } else {
            for p in &self.providers {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, color) = if p.ready {
                                ("●", egui::Color32::GREEN)
                            } else {
                                ("○", egui::Color32::RED)
                            };
                            ui.colored_label(color, icon);
                            ui.label(&p.name);
                            if !p.model.is_empty() {
                                ui.label(format!("({})", p.model));
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let label = if p.ready {
                                        i18n.t("monitor.ready")
                                    } else {
                                        i18n.t("monitor.notReady")
                                    };
                                    ui.colored_label(color, label);
                                },
                            );
                        });
                    });
                ui.add_space(2.0);
            }
        }

        // ── Restart button ───────────────────────────────────────
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let restart_label = if self.restarting {
                i18n.t("monitor.restarting").to_string()
            } else {
                i18n.t("monitor.restart").to_string()
            };
            let btn = egui::Button::new(format!("\u{1f504} {}", restart_label))
                .min_size(egui::vec2(140.0, 32.0));
            if ui.add_enabled(!self.restarting, btn).clicked() {
                self.restarting = true;
                let backend_clone = backend.clone();
                tokio::spawn(async move {
                    if let Err(e) = backend_clone.restart_backend().await {
                        eprintln!("重启后端失败: {}", e);
                    }
                });
            }
            if self.restarting {
                ui.label(i18n.t("monitor.restartHint"));
            }
        });

        // ── Error message ───────────────────────────────────────
        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::RED, &self.error);
        }
    }
}
