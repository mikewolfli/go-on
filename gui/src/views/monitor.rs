use crate::backend::{BackendClient, HealthStatus, ProviderStatus};
use crate::i18n::I18n;

pub struct MonitorView {
    pub health: Option<HealthStatus>,
    providers: Vec<ProviderStatus>,
    error: String,
}

impl MonitorView {
    pub fn new() -> Self {
        Self {
            health: None,
            providers: Vec::new(),
            error: String::new(),
        }
    }

    pub async fn refresh(&mut self, backend: &BackendClient) {
        self.health = Some(backend.health().await);
        self.providers = backend.provider_status().await;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        ui.heading(i18n.t("monitor.health"));
        ui.separator();
        ui.add_space(8.0);

        if let Some(ref health) = self.health {
            // Status card
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (color, text) = if !health.connected {
                        (egui::Color32::RED, i18n.t("monitor.offline"))
                    } else if health.healthy {
                        (egui::Color32::GREEN, i18n.t("monitor.healthy"))
                    } else {
                        (egui::Color32::YELLOW, i18n.t("monitor.unhealthy"))
                    };
                    ui.colored_label(color, "⬤");
                    ui.label(text);
                });
                ui.label(format!(
                    "{}: {}",
                    i18n.t("monitor.latency"),
                    health.avg_latency_ms
                ));
            });

            ui.add_space(16.0);

            // Provider status
            ui.heading(i18n.t("monitor.providers"));
            ui.separator();
            ui.add_space(4.0);

            for p in &self.providers {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let color = if p.ready {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::RED
                            };
                            ui.colored_label(color, if p.ready { "●" } else { "○" });
                            ui.label(&p.name);
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
            }

            if self.providers.is_empty() {
                ui.label(i18n.t("monitor.notReady"));
            }
        } else {
            ui.label(i18n.t("app.connecting"));
        }

        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::RED, &self.error);
        }
    }
}
