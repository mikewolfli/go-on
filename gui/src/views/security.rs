use crate::backend::BackendClient;
use crate::views::security_prefs::{self, SecurityPrefs};
use std::sync::mpsc;

pub struct SecurityView {
    state: SecurityPrefs,
    status: String,
    sending: bool,
    pending_restart_confirmation: bool,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::Sender<String>,
}

impl SecurityView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::channel();
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

    pub fn show(&mut self, ui: &mut egui::Ui, backend: &BackendClient, ctx: &egui::Context) {
        self.process_pending();

        ui.heading("Security");
        let text = "Manage client-side safety policies and runtime restart controls.".to_string();
        let resp = ui.label(&text);
        resp.context_menu(|ui| {
            if ui.button("📋 Copy").clicked() {
                ui.ctx().copy_text(text.clone());
                ui.close_menu();
            }
        });
        ui.separator();

        let mut changed = false;
        changed |= ui
            .checkbox(
                &mut self.state.confirm_dangerous_actions,
                "Require confirmation for dangerous actions",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut self.state.redact_api_keys_in_ui,
                "Redact API keys in UI",
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut self.state.block_external_urls,
                "Block external URL imports",
            )
            .changed();

        if changed {
            security_prefs::save(&self.state);
            self.status = "Security settings saved.".to_string();
            self.pending_restart_confirmation = false;
        }

        ui.add_space(8.0);
        let restart_label = if self.pending_restart_confirmation {
            "Confirm Restart Runtime"
        } else {
            "Restart Backend Runtime"
        };
        if ui
            .add_enabled(!self.sending, egui::Button::new(restart_label))
            .clicked()
        {
            if self.state.confirm_dangerous_actions && !self.pending_restart_confirmation {
                self.pending_restart_confirmation = true;
                self.status = "Click restart again to confirm.".to_string();
                return;
            }

            self.sending = true;
            self.status.clear();
            self.pending_restart_confirmation = false;
            let tx = self.pending_tx.clone();
            let backend_clone = backend.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                let msg = match backend_clone.restart_backend().await {
                    Ok(_) => "Runtime restart requested.".to_string(),
                    Err(e) => format!("Restart failed: {e}"),
                };
                let _ = tx.send(msg);
                ctx_clone.request_repaint();
            });
        }

        if !self.status.is_empty() {
            let text = self.status.clone();
            let resp = ui.label(&text);
            resp.context_menu(|ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close_menu();
                }
            });
        }
    }
}
