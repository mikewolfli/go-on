use crate::backend::HealthStatus;
use crate::i18n::I18n;
use crate::widgets::cache::CachedView;

pub struct AboutView {
    cached_view: CachedView,
}

impl AboutView {
    pub fn new() -> Self {
        Self {
            cached_view: CachedView::new(),
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        health: Option<&HealthStatus>,
        backend_pid: Option<u32>,
    ) {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let hash = 0_u64;

                self.cached_view.check_or_render(ui, "about", hash, |ui| {
                    ui.heading(i18n.t("about.title"));
                    ui.label(i18n.t("about.subtitle"));
                    ui.separator();

                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        let gui_version = env!("CARGO_PKG_VERSION");
                        ui.label(format!("{}: {}", i18n.t("about.guiVersion"), gui_version));

                        let backend_status = if health.is_some_and(|h| h.connected) {
                            i18n.t("status.connected")
                        } else {
                            i18n.t("status.disconnected")
                        };
                        ui.label(format!(
                            "{}: {}",
                            i18n.t("about.backendStatus"),
                            backend_status
                        ));

                        let gui_ver = format!("v{}", env!("CARGO_PKG_VERSION"));
                        let backend_version = health
                            .and_then(|h| h.backend_version.as_deref())
                            .filter(|v| !v.is_empty())
                            .unwrap_or(&gui_ver);
                        ui.label(format!(
                            "{}: {}",
                            i18n.t("about.backendVersion"),
                            backend_version
                        ));

                        let release_fallback = i18n.t("about.backendRelease").to_string();
                        let backend_build = health
                            .and_then(|h| h.backend_build.as_deref())
                            .filter(|v| !v.is_empty())
                            .unwrap_or(&release_fallback);
                        ui.label(format!(
                            "{}: {}",
                            i18n.t("about.backendBuild"),
                            backend_build
                        ));

                        let pid_text = match backend_pid {
                            Some(pid) => pid.to_string(),
                            None if health.is_some_and(|h| h.connected) => {
                                i18n.t("about.external").to_string()
                            }
                            None => i18n.t("about.unknown").to_string(),
                        };
                        ui.label(format!("{}: {}", i18n.t("about.backendPid"), pid_text));
                    });

                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(i18n.t("about.improvedTitle")).strong());
                    ui.add_space(4.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        let items = [
                            i18n.t("about.improved.monitor"),
                            i18n.t("about.improved.workflow"),
                            i18n.t("about.improved.providers"),
                            i18n.t("about.improved.skills"),
                            i18n.t("about.improved.i18n"),
                        ];
                        for (idx, item) in items.iter().enumerate() {
                            ui.label(format!("{}. {}", idx + 1, item));
                        }
                    });

                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(i18n.t("about.projectTitle")).strong());
                    ui.add_space(4.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.label(i18n.t("about.projectDescription"));
                        ui.add(egui::Label::new(
                            egui::RichText::new(i18n.t("about.githubLink")).size(13.0),
                        ));
                        ui.hyperlink("https://github.com/mikewolfli/go-on");
                    });
                });
            });
    }
}
