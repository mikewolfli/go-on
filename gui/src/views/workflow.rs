use crate::views::security_prefs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

const STEP_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowStep {
    name: String,
    command: String,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WorkflowState {
    steps: Vec<WorkflowStep>,
    last_run_at: Option<u64>,
    last_result: Option<String>,
}

pub struct WorkflowView {
    state: WorkflowState,
    new_name: String,
    new_command: String,
    running: bool,
    pending_confirm_run: bool,
    pending_confirm_delete: Option<usize>,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::Sender<String>,
}

impl WorkflowView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::channel();
        Self {
            state: Self::load_state(),
            new_name: String::new(),
            new_command: String::new(),
            running: false,
            pending_confirm_run: false,
            pending_confirm_delete: None,
            pending_rx,
            pending_tx,
        }
    }

    fn state_path() -> PathBuf {
        if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
            dirs.config_dir().join("workflow_state.json")
        } else {
            PathBuf::from("workflow_state.json")
        }
    }

    fn load_state() -> WorkflowState {
        let path = Self::state_path();
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => WorkflowState::default(),
        }
    }

    fn save_state(&self) {
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("Failed to create workflow state directory: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(&self.state) {
            Ok(content) => {
                if let Err(e) = std::fs::write(&path, content) {
                    eprintln!("Failed to write workflow state {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("Failed to serialize workflow state: {e}"),
        }
    }

    fn process_pending(&mut self) {
        while let Ok(result) = self.pending_rx.try_recv() {
            self.state.last_run_at = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
            self.state.last_result = Some(result);
            self.running = false;
            self.save_state();
        }
    }

    fn trigger_run(&mut self, ctx: &egui::Context) {
        let steps: Vec<(String, String)> = self
            .state
            .steps
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.name.clone(), s.command.clone()))
            .collect();
        if steps.is_empty() {
            self.state.last_result = Some("No enabled steps to run.".to_string());
            self.save_state();
            return;
        }

        self.running = true;
        self.state.last_result = Some("Running workflow...".to_string());
        self.save_state();

        let tx = self.pending_tx.clone();
        let ctx_clone = ctx.clone();
        tokio::spawn(async move {
            let mut lines = Vec::new();
            for (idx, (name, command)) in steps.iter().enumerate() {
                lines.push(format!("Step {} [{}]: {}", idx + 1, name, command));
                #[cfg(target_os = "windows")]
                let child_spawn = tokio::process::Command::new("cmd")
                    .arg("/C")
                    .arg(command)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                #[cfg(not(target_os = "windows"))]
                let child_spawn = tokio::process::Command::new("sh")
                    .arg("-lc")
                    .arg(command)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                let mut child = match child_spawn {
                    Ok(c) => c,
                    Err(e) => {
                        lines.push(format!("  failed to spawn: {e}"));
                        lines.push("Workflow stopped due to execution error.".to_string());
                        break;
                    }
                };

                let stdout_task = child.stdout.take().map(|mut stdout| {
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut buf).await;
                        buf
                    })
                });
                let stderr_task = child.stderr.take().map(|mut stderr| {
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf).await;
                        buf
                    })
                });

                let timed = tokio::time::timeout(
                    std::time::Duration::from_secs(STEP_TIMEOUT_SECS),
                    child.wait(),
                )
                .await;

                match timed {
                    Ok(wait_result) => match wait_result {
                        Ok(status) => {
                            let code = status.code().unwrap_or(-1);
                            let stdout = if let Some(task) = stdout_task {
                                task.await
                                    .map(|b| String::from_utf8_lossy(&b).trim().to_string())
                                    .unwrap_or_default()
                            } else {
                                String::new()
                            };
                            let stderr = if let Some(task) = stderr_task {
                                task.await
                                    .map(|b| String::from_utf8_lossy(&b).trim().to_string())
                                    .unwrap_or_default()
                            } else {
                                String::new()
                            };
                            lines.push(format!("  exit={code}"));
                            if !stdout.is_empty() {
                                lines.push(format!("  stdout: {stdout}"));
                            }
                            if !stderr.is_empty() {
                                lines.push(format!("  stderr: {stderr}"));
                            }
                            if !status.success() {
                                lines.push("Workflow stopped due to step failure.".to_string());
                                break;
                            }
                        }
                        Err(e) => {
                            lines.push(format!("  failed to wait process: {e}"));
                            lines.push("Workflow stopped due to execution error.".to_string());
                            break;
                        }
                    },
                    Err(_) => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        lines.push(format!(
                            "  timed out after {}s and process was terminated",
                            STEP_TIMEOUT_SECS
                        ));
                        lines.push("Workflow stopped due to step timeout.".to_string());
                        break;
                    }
                }
            }

            if lines.is_empty() {
                lines.push("No output.".to_string());
            }
            let _ = tx.send(lines.join("\n"));
            ctx_clone.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.process_pending();

        ui.heading("Workflow");
        let text = "Create reusable multi-step workflow presets and run enabled steps.".to_string();
        let resp = ui.label(&text);
        resp.context_menu(|ui| {
            if ui.button("📋 Copy").clicked() {
                ui.ctx().copy_text(text.clone());
                ui.close_menu();
            }
        });
        ui.separator();

        let security = security_prefs::load();

        ui.horizontal(|ui| {
            let text = "Step".to_string();
            let resp = ui.label(&text);
            resp.context_menu(|ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close_menu();
                }
            });
            ui.text_edit_singleline(&mut self.new_name);
            let text = "Command".to_string();
            let resp = ui.label(&text);
            resp.context_menu(|ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close_menu();
                }
            });
            ui.text_edit_singleline(&mut self.new_command);
            if ui.button("Add").clicked() {
                let name = self.new_name.trim();
                let cmd = self.new_command.trim();
                if !name.is_empty() && !cmd.is_empty() {
                    self.state.steps.push(WorkflowStep {
                        name: name.to_string(),
                        command: cmd.to_string(),
                        enabled: true,
                    });
                    self.new_name.clear();
                    self.new_command.clear();
                    self.save_state();
                }
            }
        });

        ui.add_space(8.0);
        if self.state.steps.is_empty() {
            let text = "No steps yet.".to_string();
            let resp = ui.label(&text);
            resp.context_menu(|ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close_menu();
                }
            });
        }

        let mut changed = false;
        let mut remove_idx = None;
        for (idx, step) in self.state.steps.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut step.enabled, "").changed() {
                        changed = true;
                    }
                    let text = step.name.clone();
                    let resp = ui.label(&text);
                    resp.context_menu(|ui| {
                        if ui.button("📋 Copy").clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close_menu();
                        }
                    });
                    ui.separator();
                    let text = step.command.clone();
                    let resp = ui.label(&text);
                    resp.context_menu(|ui| {
                        if ui.button("📋 Copy").clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close_menu();
                        }
                    });
                    let delete_label = if self.pending_confirm_delete == Some(idx) {
                        "Confirm Delete"
                    } else {
                        "Delete"
                    };
                    if ui.button(delete_label).clicked() {
                        if security.confirm_dangerous_actions
                            && self.pending_confirm_delete != Some(idx)
                        {
                            self.pending_confirm_delete = Some(idx);
                            self.state.last_result = Some(format!(
                                "Click delete again to remove step '{}'.",
                                step.name
                            ));
                        } else {
                            remove_idx = Some(idx);
                            self.pending_confirm_delete = None;
                        }
                    }
                });
            });
            ui.add_space(4.0);
        }

        if let Some(idx) = remove_idx {
            self.state.steps.remove(idx);
            changed = true;
        }

        ui.add_space(8.0);
        let run_label = if self.pending_confirm_run {
            "Confirm Run Enabled Steps"
        } else {
            "Run Enabled Steps"
        };
        if ui
            .add_enabled(!self.running, egui::Button::new(run_label))
            .clicked()
        {
            if security.confirm_dangerous_actions && !self.pending_confirm_run {
                self.pending_confirm_run = true;
                self.state.last_result = Some("Click run again to confirm.".to_string());
                changed = true;
            } else {
                self.pending_confirm_run = false;
                self.trigger_run(ctx);
            }
        }

        if let Some(result) = &self.state.last_result {
            let text = result.clone();
            let resp = ui.label(&text);
            resp.context_menu(|ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close_menu();
                }
            });
        }

        if changed {
            self.save_state();
        }
    }
}
