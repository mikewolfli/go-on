use crate::backend::{BackendClient, HealthStatus, ProviderStatus};
use crate::i18n::I18n;
use std::sync::mpsc;

pub struct MonitorView {
    pub health: Option<HealthStatus>,
    pub providers: Vec<ProviderStatus>,
    pub backend_configured: bool,
    error: String,
    restarting: bool,
    metrics_window: String,
    metrics_lines: Vec<String>,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::Sender<String>,
}

impl MonitorView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::channel();
        Self {
            health: None,
            providers: Vec::new(),
            backend_configured: false,
            error: String::new(),
            restarting: false,
            metrics_window: "5m".to_string(),
            metrics_lines: Vec::new(),
            pending_rx,
            pending_tx,
        }
    }

    fn process_pending(&mut self) {
        while let Ok(msg) = self.pending_rx.try_recv() {
            if let Some(payload) = msg.strip_prefix("__metrics__:") {
                self.metrics_lines = payload.lines().map(ToString::to_string).collect();
            } else if let Some(err) = msg.strip_prefix("__metrics_error__:") {
                self.error = err.to_string();
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend_configured: bool,
        backend: &BackendClient,
        monitor_history_alerts_enabled: bool,
    ) {
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        self.process_pending();
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
                        let text = i18n.t("app.connecting").to_string();
                        let resp = ui.label(&text);
                        resp.context_menu(|ui| {
                            if ui.button("📋 Copy").clicked() {
                                ui.ctx().copy_text(text.clone());
                                ui.close_menu();
                            }
                        });
                    });
                }
                Some(health) => {
                    ui.horizontal(|ui| {
                        let (_icon, color, text) = if !health.connected {
                            ("◌", egui::Color32::RED, i18n.t("monitor.offline"))
                        } else if health.healthy {
                            ("●", egui::Color32::GREEN, i18n.t("monitor.healthy"))
                        } else {
                            ("◉", egui::Color32::YELLOW, i18n.t("monitor.unhealthy"))
                        };
                        let text = text.to_string();
                        let resp = ui.colored_label(color, &text);
                        resp.context_menu(|ui| {
                            if ui.button("📋 Copy").clicked() {
                                ui.ctx().copy_text(text.clone());
                                ui.close_menu();
                            }
                        });
                    });
                    let text = format!(
                        "{}: {} ms",
                        i18n.t("monitor.latency"),
                        health.avg_latency_ms
                    );
                    let resp = ui.label(&text);
                    resp.context_menu(|ui| {
                        if ui.button("📋 Copy").clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close_menu();
                        }
                    });
                    let text = format!(
                        "{}: {}% (uptime: {}s)",
                        i18n.t("monitor.success"),
                        health.success_rate,
                        health.uptime
                    );
                    let resp = ui.label(&text);
                    resp.context_menu(|ui| {
                        if ui.button("📋 Copy").clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close_menu();
                        }
                    });
                    let text = format!(
                        "{}: {:.1}",
                        i18n.t("monitor.rpm"),
                        health.requests_per_minute
                    );
                    let resp = ui.label(&text);
                    resp.context_menu(|ui| {
                        if ui.button("📋 Copy").clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close_menu();
                        }
                    });
                }
            }
        });

        ui.add_space(16.0);

        if monitor_history_alerts_enabled {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("monitor_metrics_window")
                        .selected_text(self.metrics_window.clone())
                        .show_ui(ui, |ui| {
                            for w in ["1m", "5m", "1h"] {
                                ui.selectable_value(&mut self.metrics_window, w.to_string(), w);
                            }
                        });
                    if ui.button(i18n.t("monitor.loadTrends")).clicked() {
                        let backend_clone = backend.clone();
                        let window = self.metrics_window.clone();
                        let tx = self.pending_tx.clone();
                        tokio::spawn(async move {
                            // Add timeout to prevent hanging
                            let result = match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                backend_clone.metrics_window_query(&window)
                            ).await {
                                Ok(r) => r,
                                Err(_) => {
                                    eprintln!("Warning: metrics_window_query timed out");
                                    Err("timeout".to_string())
                                }
                            };
                            let payload = match result {
                                Ok(series) => {
                                    let mut out = vec![format!("window={window} points={}", series.len())];
                                    if let Some(last) = series.last() {
                                        out.push(format!(
                                            "latest qps={:.2} p95={:.2} error_rate={:.3} success_rate={:.3}",
                                            last.qps, last.p95, last.error_rate, last.success_rate
                                        ));
                                    }
                                    format!("__metrics__:{}", out.join("\n"))
                                }
                                Err(e) => format!("__metrics_error__:{e}"),
                            };
                            let _ = tx.send(payload);
                        });
                    }
                    if ui.button(i18n.t("monitor.loadErrors")).clicked() {
                        let backend_clone = backend.clone();
                        let window = self.metrics_window.clone();
                        let tx = self.pending_tx.clone();
                        tokio::spawn(async move {
                            // Add timeout to prevent hanging
                            let result = match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                backend_clone.metrics_errors_summary(&window, 10)
                            ).await {
                                Ok(r) => r,
                                Err(_) => {
                                    eprintln!("Warning: metrics_errors_summary timed out");
                                    Err("timeout".to_string())
                                }
                            };
                            let payload = match result {
                                Ok((groups, failures)) => {
                                    let mut out = vec![format!("error_groups={}", groups.len())];
                                    for g in groups.into_iter().take(5) {
                                        out.push(format!("{}={}", g.error_type, g.count));
                                    }
                                    out.push(format!("sample_failures={}", failures.len()));
                                    format!("__metrics__:{}", out.join("\n"))
                                }
                                Err(e) => format!("__metrics_error__:{e}"),
                            };
                            let _ = tx.send(payload);
                        });
                    }
                });
                for line in &self.metrics_lines {
                    ui.label(line);
                }
            });
            ui.add_space(10.0);
        }

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
                    let text = i18n.t("monitor.notReady").to_string();
                    let resp = ui.label(&text);
                    resp.context_menu(|ui| {
                        if ui.button("📋 Copy").clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close_menu();
                        }
                    });
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
                            let text = p.name.clone();
                            let resp = ui.label(&text);
                            resp.context_menu(|ui| {
                                if ui.button("📋 Copy").clicked() {
                                    ui.ctx().copy_text(text.clone());
                                    ui.close_menu();
                                }
                            });
                            if !p.model.is_empty() {
                                let text = format!("({})", p.model);
                                let resp = ui.label(&text);
                                resp.context_menu(|ui| {
                                    if ui.button("📋 Copy").clicked() {
                                        ui.ctx().copy_text(text.clone());
                                        ui.close_menu();
                                    }
                                });
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
                let text = i18n.t("monitor.restartHint").to_string();
                let resp = ui.label(&text);
                resp.context_menu(|ui| {
                    if ui.button("📋 Copy").clicked() {
                        ui.ctx().copy_text(text.clone());
                        ui.close_menu();
                    }
                });
            }
        });

        // ── Error message ───────────────────────────────────────
        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::RED, &self.error);
        }
        });
    }
}
