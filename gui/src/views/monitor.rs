use crate::backend::{BackendClient, ErrorGroup, HealthStatus, MetricsWindowPoint, ProviderStatus};
use crate::i18n::I18n;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct MonitorView {
    pub health: Option<HealthStatus>,
    pub providers: Vec<ProviderStatus>,
    pub backend_configured: bool,
    error: String,
    metrics_window: String,
    metrics_lines: Vec<String>,
    trend_series: Vec<MetricsWindowPoint>,
    error_groups: Vec<ErrorGroup>,
    sample_failures_count: usize,
    provider_filter: String,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::Sender<String>,
    last_metrics_load: Instant,
}

impl MonitorView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::channel();
        Self {
            health: None,
            providers: Vec::new(),
            backend_configured: false,
            error: String::new(),
            metrics_window: "5m".to_string(),
            metrics_lines: Vec::new(),
            trend_series: Vec::new(),
            error_groups: Vec::new(),
            sample_failures_count: 0,
            provider_filter: String::new(),
            pending_rx,
            pending_tx,
            last_metrics_load: Instant::now(),
        }
    }

    fn process_pending(&mut self) {
        while let Ok(msg) = self.pending_rx.try_recv() {
            if let Some(payload) = msg.strip_prefix("__metrics__:") {
                self.metrics_lines = payload.lines().map(ToString::to_string).collect();
                self.error.clear();
            } else if let Some(payload) = msg.strip_prefix("__trends__:") {
                if let Ok(series) = serde_json::from_str::<Vec<MetricsWindowPoint>>(payload) {
                    self.trend_series = series;
                }
            } else if let Some(payload) = msg.strip_prefix("__errors_summary__:") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
                    self.error_groups = v
                        .get("groups")
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|value| serde_json::from_value::<ErrorGroup>(value).ok())
                        .collect();
                    self.sample_failures_count = v
                        .get("sample_failures_count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as usize;
                }
            } else if let Some(err) = msg.strip_prefix("__metrics_error__:") {
                self.error = err.to_string();
                self.trend_series.clear();
                self.error_groups.clear();
                self.sample_failures_count = 0;
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
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.process_pending();
                self.backend_configured = backend_configured;

                // Auto-refresh metrics every 30 seconds
                if self.last_metrics_load.elapsed() >= Duration::from_secs(30) && backend_configured
                {
                    self.last_metrics_load = Instant::now();
                    let backend_clone = backend.clone();
                    let window = self.metrics_window.clone();
                    let tx = self.pending_tx.clone();
                    let ctx_clone = ui.ctx().clone();
                    tokio::spawn(async move {
                        // Load trends
                        let result = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            backend_clone.metrics_window_query(&window),
                        )
                        .await;
                        if let Ok(Ok(series)) = result {
                            let trends_json =
                                serde_json::to_string(&series).unwrap_or_else(|_| "[]".to_string());
                            let metrics = format!("window={window} points={}", series.len());
                            let _ = tx.send(format!("__metrics__:{metrics}"));
                            let _ = tx.send(format!("__trends__:{trends_json}"));
                        };

                        // Load errors
                        let err_result = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            backend_clone.metrics_errors_summary(&window, 10),
                        )
                        .await;
                        if let Ok(Ok((groups, failures))) = err_result {
                            let summary_json = serde_json::json!({
                                "groups": groups,
                                "sample_failures_count": failures.len()
                            });
                            let _ = tx.send(format!("__errors_summary__:{summary_json}"));
                        }
                        ctx_clone.request_repaint();
                    });
                }

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
                                    if ui.button(i18n.t("common.copyButton")).clicked() {
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
                                    (
                                        "●",
                                        egui::Color32::from_rgb(20, 120, 70),
                                        i18n.t("monitor.healthy"),
                                    )
                                } else {
                                    ("◉", egui::Color32::YELLOW, i18n.t("monitor.unhealthy"))
                                };
                                let text = text.to_string();
                                let resp = ui.colored_label(color, &text);
                                resp.context_menu(|ui| {
                                    if ui.button(i18n.t("common.copyButton")).clicked() {
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
                                if ui.button(i18n.t("common.copyButton")).clicked() {
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
                                if ui.button(i18n.t("common.copyButton")).clicked() {
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
                                if ui.button(i18n.t("common.copyButton")).clicked() {
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
                                        ui.selectable_value(
                                            &mut self.metrics_window,
                                            w.to_string(),
                                            w,
                                        );
                                    }
                                });
                            if ui.button(i18n.t("monitor.loadTrends")).clicked() {
                                let backend_clone = backend.clone();
                                let window = self.metrics_window.clone();
                                let tx = self.pending_tx.clone();
                                let ctx_clone = ui.ctx().clone();
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.metrics_window_query(&window),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            eprintln!("Warning: metrics_window_query timed out");
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let payload = match result {
                                        Ok(series) => {
                                            let trends_json = serde_json::to_string(&series)
                                                .unwrap_or_else(|_| "[]".to_string());
                                            let metrics =
                                                format!("window={window} points={}", series.len());
                                            (
                                                format!("__metrics__:{metrics}"),
                                                format!("__trends__:{trends_json}"),
                                            )
                                        }
                                        Err(e) => (format!("__metrics_error__:{e}"), String::new()),
                                    };
                                    let _ = tx.send(payload.0);
                                    if !payload.1.is_empty() {
                                        let _ = tx.send(payload.1);
                                    }
                                    ctx_clone.request_repaint();
                                });
                            }
                            if ui
                                .button("⟳")
                                .on_hover_text(i18n.t("monitor.refreshNow"))
                                .clicked()
                            {
                                self.last_metrics_load = Instant::now() - Duration::from_secs(31);
                            }
                            if ui.button(i18n.t("monitor.loadErrors")).clicked() {
                                let backend_clone = backend.clone();
                                let window = self.metrics_window.clone();
                                let tx = self.pending_tx.clone();
                                let ctx_clone = ui.ctx().clone();
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.metrics_errors_summary(&window, 10),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            eprintln!("Warning: metrics_errors_summary timed out");
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let payload = match result {
                                        Ok((groups, failures)) => {
                                            let metrics = format!(
                                                "window={window} error_groups={}",
                                                groups.len()
                                            );
                                            let summary_json = serde_json::json!({
                                                "groups": groups,
                                                "sample_failures_count": failures.len()
                                            });
                                            (
                                                format!("__metrics__:{metrics}"),
                                                format!("__errors_summary__:{summary_json}"),
                                            )
                                        }
                                        Err(e) => (format!("__metrics_error__:{e}"), String::new()),
                                    };
                                    let _ = tx.send(payload.0);
                                    if !payload.1.is_empty() {
                                        let _ = tx.send(payload.1);
                                    }
                                    ctx_clone.request_repaint();
                                });
                            }
                        });

                        if let Some(last) = self.trend_series.last() {
                            ui.add_space(6.0);
                            ui.label(i18n.t("monitor.trendSummary"));
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!("{}: {:.2}", i18n.t("monitor.qps"), last.qps));
                                ui.label(format!("{}: {:.2}ms", i18n.t("monitor.p95"), last.p95));
                                ui.label(format!(
                                    "{}: {:.3}",
                                    i18n.t("monitor.errorRate"),
                                    last.error_rate
                                ));
                                ui.label(format!(
                                    "{}: {:.3}",
                                    i18n.t("monitor.successRate"),
                                    last.success_rate
                                ));
                            });
                        }

                        if !self.error_groups.is_empty() {
                            ui.add_space(6.0);
                            ui.label(i18n.t("monitor.errorTopGroups"));
                            for g in self.error_groups.iter().take(5) {
                                ui.label(format!("{}: {}", g.error_type, g.count));
                            }
                            ui.label(
                                i18n.t("monitor.sampleFailures")
                                    .replace("{count}", &self.sample_failures_count.to_string()),
                            );
                        }

                        for line in &self.metrics_lines {
                            // Color-code metric lines by content for quick visual scanning.
                            let color =
                                if line.contains("error_rate") || line.contains("error_groups") {
                                    egui::Color32::from_rgb(220, 80, 80)
                                } else if line.contains("success_rate") || line.contains("qps") {
                                    egui::Color32::from_rgb(60, 180, 100)
                                } else if line.contains("p95") || line.contains("timeout") {
                                    egui::Color32::from_rgb(220, 170, 60)
                                } else {
                                    ui.visuals().text_color()
                                };
                            ui.colored_label(color, line);
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

                // Provider search/filter
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.provider_filter)
                        .hint_text(i18n.t("monitor.filterProviders"))
                        .desired_width(200.0),
                );
                ui.add_space(4.0);

                if self.providers.is_empty() {
                    // Show a more descriptive message when health is known but no providers
                    match &self.health {
                        Some(h) if h.connected => {
                            let text = i18n.t("monitor.notReady").to_string();
                            let resp = ui.label(&text);
                            resp.context_menu(|ui| {
                                if ui.button(i18n.t("common.copyButton")).clicked() {
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
                    let filtered_providers: Vec<_> = if self.provider_filter.is_empty() {
                        self.providers.iter().collect()
                    } else {
                        let q = self.provider_filter.to_lowercase();
                        self.providers
                            .iter()
                            .filter(|p| p.name.to_lowercase().contains(&q))
                            .collect()
                    };
                    for p in filtered_providers {
                        egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (icon, color) = if p.ready {
                                        ("●", egui::Color32::from_rgb(20, 120, 70))
                                    } else {
                                        ("○", egui::Color32::RED)
                                    };
                                    ui.colored_label(color, icon);
                                    let text = p.name.clone();
                                    let resp = ui.label(&text);
                                    resp.context_menu(|ui| {
                                        if ui.button(i18n.t("common.copyButton")).clicked() {
                                            ui.ctx().copy_text(text.clone());
                                            ui.close_menu();
                                        }
                                    });
                                    if !p.model.is_empty() {
                                        let text = format!("({})", p.model);
                                        let resp = ui.label(&text);
                                        resp.context_menu(|ui| {
                                            if ui.button(i18n.t("common.copyButton")).clicked() {
                                                ui.ctx().copy_text(text.clone());
                                                ui.close_menu();
                                            }
                                        });
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let status_label = if p.ready {
                                                i18n.t("monitor.ready")
                                            } else {
                                                i18n.t("monitor.notReady")
                                            };
                                            let status_color = if p.ready {
                                                egui::Color32::from_rgb(20, 120, 70)
                                            } else {
                                                egui::Color32::from_rgb(198, 60, 60)
                                            };
                                            egui::Frame::new()
                                                .fill(status_color.gamma_multiply(0.15))
                                                .corner_radius(10.0)
                                                .inner_margin(egui::Margin::symmetric(8i8, 2i8))
                                                .show(ui, |ui| {
                                                    ui.colored_label(status_color, status_label);
                                                });
                                        },
                                    );
                                });
                            });
                        ui.add_space(2.0);
                    }
                }

                // ── Error message ───────────────────────────────────────
                if !self.error.is_empty() {
                    ui.colored_label(egui::Color32::RED, &self.error);
                }
            });
    }
}
