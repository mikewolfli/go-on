use crate::backend::{BackendClient, ErrorGroup, HealthStatus, MetricsWindowPoint, ProviderStatus};
use crate::i18n::I18n;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct MonitorView {
    pub health: Option<HealthStatus>,
    pub providers: Vec<ProviderStatus>,
    pub backend_configured: bool,
    error: String,
    pub metrics_window: String,
    pub available_windows: Vec<String>,
    metrics_lines: Vec<String>,
    trend_series: Vec<MetricsWindowPoint>,
    error_groups: Vec<ErrorGroup>,
    sample_failures_count: usize,
    pub provider_filter: String,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::Sender<String>,
    last_metrics_load: Instant,
    pub auto_refresh_interval: u64,
    consecutive_metrics_failures: u32,
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
            available_windows: vec![
                "1m".to_string(),
                "5m".to_string(),
                "15m".to_string(),
                "1h".to_string(),
            ],
            metrics_lines: Vec::new(),
            trend_series: Vec::new(),
            error_groups: Vec::new(),
            sample_failures_count: 0,
            provider_filter: String::new(),
            pending_rx,
            pending_tx,
            last_metrics_load: Instant::now(),
            auto_refresh_interval: 30,
            consecutive_metrics_failures: 0,
        }
    }

    fn effective_refresh_interval(&self) -> Duration {
        let multiplier = 2_u64.pow(self.consecutive_metrics_failures.min(3));
        Duration::from_secs(self.auto_refresh_interval.saturating_mul(multiplier))
    }

    fn process_pending(&mut self) {
        const MAX_EVENTS_PER_FRAME: usize = 10;
        const MAX_METRICS_LINES: usize = 500;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Ok(msg) = self.pending_rx.try_recv() else {
                break;
            };
            if let Some(payload) = msg.strip_prefix("__metrics__:") {
                let mut lines: Vec<String> = payload.lines().map(ToString::to_string).collect();
                lines.truncate(MAX_METRICS_LINES);
                self.metrics_lines = lines;
                self.error.clear();
                self.consecutive_metrics_failures = 0;
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
                self.consecutive_metrics_failures =
                    self.consecutive_metrics_failures.saturating_add(1);
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

                let effective_refresh_interval = self.effective_refresh_interval();

                if self.last_metrics_load.elapsed() >= effective_refresh_interval
                    && backend_configured
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
                        match result {
                            Ok(Ok(series)) => {
                                let trends_json = serde_json::to_string(&series)
                                    .unwrap_or_else(|_| "[]".to_string());
                                let metrics = format!("window={window} points={}", series.len());
                                let _ = tx.send(format!("__metrics__:{metrics}"));
                                let _ = tx.send(format!("__trends__:{trends_json}"));
                            }
                            Ok(Err(err)) => {
                                let _ = tx.send(format!("__metrics_error__:{err}"));
                            }
                            Err(_) => {
                                let _ = tx.send("__metrics_error__:timeout".to_string());
                            }
                        }

                        // Load errors
                        let err_result = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            backend_clone.metrics_errors_summary(&window, 10),
                        )
                        .await;
                        match err_result {
                            Ok(Ok((groups, failures))) => {
                                let summary_json = serde_json::json!({
                                    "groups": groups,
                                    "sample_failures_count": failures.len()
                                });
                                let _ = tx.send(format!("__errors_summary__:{summary_json}"));
                            }
                            Ok(Err(err)) => {
                                let _ = tx.send(format!("__metrics_error__:{err}"));
                            }
                            Err(_) => {
                                let _ = tx.send("__metrics_error__:timeout".to_string());
                            }
                        }
                        ctx_clone.request_repaint_after(Duration::from_millis(16));
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
                                ui.label(i18n.t("app.connecting"));
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
                                ui.colored_label(color, &text.to_string());
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

                if !self.error.is_empty() {
                    let retry_in = effective_refresh_interval
                        .saturating_sub(self.last_metrics_load.elapsed())
                        .as_secs()
                        .max(1);
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 120, 80), &self.error);
                    ui.label(
                        i18n.t("monitor.retryIn")
                            .replace("{seconds}", &retry_in.to_string()),
                    );
                }

                ui.add_space(16.0);

                if monitor_history_alerts_enabled {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(i18n.t("monitor.timeWindow"));
                            egui::ComboBox::from_id_salt("monitor_metrics_window")
                                .selected_text(self.metrics_window.clone())
                                .show_ui(ui, |ui| {
                                    for w in &self.available_windows {
                                        ui.selectable_value(&mut self.metrics_window, w.clone(), w);
                                    }
                                });
                            ui.separator();
                            ui.label(i18n.t("monitor.refreshInterval"));
                            let mut refresh_interval = self.auto_refresh_interval as i32;
                            if ui
                                .add(egui::Slider::new(&mut refresh_interval, 10..=120))
                                .changed()
                            {
                                self.auto_refresh_interval = refresh_interval as u64;
                            }
                            ui.label(format!("{}s", self.auto_refresh_interval));
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
                                        Err(_) => Err("timeout".to_string()),
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
                                    ctx_clone.request_repaint_after(Duration::from_millis(16));
                                });
                            }
                            if ui
                                .button("⟳")
                                .on_hover_text(i18n.t("monitor.refreshNow"))
                                .clicked()
                            {
                                self.last_metrics_load =
                                    Instant::now() - self.effective_refresh_interval();
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
                                        Err(_) => Err("timeout".to_string()),
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
                                    ctx_clone.request_repaint_after(Duration::from_millis(16));
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
                            ui.label(i18n.t("monitor.notReady"));
                        }
                        _ => {
                            // Either no health data or backend offline – no point showing
                            // empty providers as "not ready"; just be silent.
                        }
                    }
                } else {
                    let q = self.provider_filter.to_lowercase();
                    let filter_enabled = !q.is_empty();
                    for p in &self.providers {
                        if filter_enabled && !p.name.to_lowercase().contains(&q) {
                            continue;
                        }
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
                                    ui.label(&p.name);
                                    if !p.model.is_empty() {
                                        ui.label(format!("({})", p.model));
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
