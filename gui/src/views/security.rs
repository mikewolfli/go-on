use super::try_send_msg;
use crate::backend::BackendClient;
use crate::i18n::I18n;
use crate::views::security_prefs::{self, SecurityPrefs};
use std::sync::mpsc;

pub struct SecurityView {
    state: SecurityPrefs,
    status: String,
    sending: bool,
    pending_restart_confirmation: bool,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::SyncSender<String>,
}

impl SecurityView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::sync_channel(256);
        Self {
            state: security_prefs::load(),
            status: String::new(),
            sending: false,
            pending_restart_confirmation: false,
            pending_rx,
            pending_tx,
        }
    }

    fn process_pending(&mut self) {
        while let Ok(msg) = self.pending_rx.try_recv() {
            self.status = msg;
            self.sending = false;
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
    ) {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.process_pending();

                ui.heading(i18n.t("tab.security"));
                ui.label(i18n.t("security.hint"));
                ui.separator();

                {
                    let mut changed = false;
                    changed |= ui
                        .checkbox(
                            &mut self.state.confirm_dangerous_actions,
                            i18n.t("security.confirmDangerousActions"),
                        )
                        .changed();
                    changed |= ui
                        .checkbox(
                            &mut self.state.redact_api_keys_in_ui,
                            i18n.t("security.redactApiKeys"),
                        )
                        .changed();
                    changed |= ui
                        .checkbox(
                            &mut self.state.block_external_urls,
                            i18n.t("security.blockExternalUrls"),
                        )
                        .changed();

                    if changed {
                        security_prefs::save(&self.state);
                        self.status = i18n.t("security.saved").to_string();
                        self.pending_restart_confirmation = false;
                    }

                    ui.add_space(8.0);
                    let restart_label = if self.pending_restart_confirmation {
                        i18n.t("security.confirmRestart")
                    } else {
                        i18n.t("security.restart")
                    };
                    if ui
                        .add_enabled(!self.sending, egui::Button::new(restart_label))
                        .clicked()
                    {
                        if self.state.confirm_dangerous_actions
                            && !self.pending_restart_confirmation
                        {
                            self.pending_restart_confirmation = true;
                            self.status = i18n.t("security.confirmAgain").to_string();
                            return;
                        }

                        self.sending = true;
                        self.status.clear();
                        self.pending_restart_confirmation = false;
                        let tx = self.pending_tx.clone();
                        let backend_clone = backend.clone();
                        let ctx_clone = ctx.clone();
                        let restart_requested = i18n.t("security.restartRequested").to_string();
                        let restart_failed = i18n.t("security.restartFailed").to_string();
                        tokio::spawn(async move {
                            // Add timeout to prevent hanging
                            let result = match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                backend_clone.restart_backend(),
                            )
                            .await
                            {
                                Ok(r) => r,
                                Err(_) => {
                                    #[cfg(debug_assertions)]
                                    eprintln!("Warning: restart_backend timed out");
                                    Err("timeout".to_string())
                                }
                            };
                            let msg = match result {
                                Ok(_) => restart_requested,
                                Err(e) => format!("{}: {e}", restart_failed),
                            };
                            try_send_msg(&tx, msg);
                            ctx_clone.request_repaint();
                        });
                    }

                    if !self.status.is_empty() {
                        if self.status.contains("failed")
                            || self.status.contains("error")
                            || self.status.contains("fail")
                        {
                            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), &self.status);
                        } else if self.status.contains("success") || self.status.contains("ok") {
                            ui.colored_label(egui::Color32::from_rgb(60, 180, 80), &self.status);
                        } else {
                            ui.colored_label(egui::Color32::from_rgb(200, 160, 60), &self.status);
                        }
                    }
                }
            });
    }
}
