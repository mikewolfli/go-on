use crate::backend::{BackendClient, WorkflowRunRecord};
use crate::i18n::I18n;
use crate::views::autotune::AutoTuneView;
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
    runs: Vec<WorkflowRunRecord>,
    selected_run_id: String,
    selected_run_detail: Option<WorkflowRunRecord>,
    run_status_filter: String,
    runs_loading: bool,
    run_center_msg: String,
}

impl WorkflowView {
    fn status_label<'a>(i18n: &'a I18n, status: &'a str) -> std::borrow::Cow<'a, str> {
        match status {
            "all" => i18n.t("workflow.runStatus.all"),
            "queued" => i18n.t("workflow.runStatus.queued"),
            "running" => i18n.t("workflow.runStatus.running"),
            "paused" => i18n.t("workflow.runStatus.paused"),
            "succeeded" => i18n.t("workflow.runStatus.succeeded"),
            "failed" => i18n.t("workflow.runStatus.failed"),
            "cancelled" => i18n.t("workflow.runStatus.cancelled"),
            _ => std::borrow::Cow::Borrowed(status),
        }
    }

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
            runs: Vec::new(),
            selected_run_id: String::new(),
            selected_run_detail: None,
            run_status_filter: "all".to_string(),
            runs_loading: false,
            run_center_msg: String::new(),
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

    fn process_pending(&mut self, i18n: &I18n) {
        while let Ok(result) = self.pending_rx.try_recv() {
            if let Some(payload) = result.strip_prefix("__runs__:") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
                    let runs = v
                        .get("runs")
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    self.runs = runs
                        .into_iter()
                        .filter_map(|x| serde_json::from_value::<WorkflowRunRecord>(x).ok())
                        .collect();
                    self.runs_loading = false;
                    if self.selected_run_id.is_empty() {
                        if let Some(first) = self.runs.first() {
                            self.selected_run_id = first.run_id.clone();
                        }
                    }
                }
                continue;
            }
            if let Some(err) = result.strip_prefix("__runs_error__:") {
                self.runs_loading = false;
                self.run_center_msg = err.to_string();
                continue;
            }
            if let Some(payload) = result.strip_prefix("__run_detail__:") {
                match serde_json::from_str::<serde_json::Value>(payload)
                    .ok()
                    .and_then(|v| v.get("run").cloned().or(Some(v)))
                    .and_then(|v| serde_json::from_value::<WorkflowRunRecord>(v).ok())
                {
                    Some(run) => {
                        self.selected_run_detail = Some(run);
                        self.run_center_msg.clear();
                    }
                    None => {
                        self.run_center_msg = i18n.t("workflow.runCenter.decodeFailed").to_string();
                    }
                }
                continue;
            }
            if let Some(err) = result.strip_prefix("__run_detail_error__:") {
                self.run_center_msg = err.to_string();
                continue;
            }

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

    fn trigger_run(&mut self, i18n: &I18n, ctx: &egui::Context) {
        let steps: Vec<(String, String)> = self
            .state
            .steps
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.name.clone(), s.command.clone()))
            .collect();
        if steps.is_empty() {
            self.state.last_result = Some(i18n.t("workflow.noEnabledSteps").to_string());
            self.save_state();
            return;
        }

        self.running = true;
        self.state.last_result = Some(i18n.t("workflow.running").to_string());
        self.save_state();

        let tx = self.pending_tx.clone();
        let ctx_clone = ctx.clone();
        let step_label = i18n.t("workflow.step").to_string();
        let exec_error = i18n.t("workflow.executionError").to_string();
        let step_failure = i18n.t("workflow.stepFailure").to_string();
        let step_timeout = i18n.t("workflow.stepTimeout").to_string();
        let no_output = i18n.t("workflow.noOutput").to_string();
        tokio::spawn(async move {
            let mut lines = Vec::new();
            for (idx, (name, command)) in steps.iter().enumerate() {
                lines.push(format!(
                    "{} {} [{}]: {}",
                    step_label,
                    idx + 1,
                    name,
                    command
                ));
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
                        lines.push(exec_error.clone());
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
                                lines.push(step_failure.clone());
                                break;
                            }
                        }
                        Err(e) => {
                            lines.push(format!("  failed to wait process: {e}"));
                            lines.push(exec_error.clone());
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
                        lines.push(step_timeout.clone());
                        break;
                    }
                }
            }

            if lines.is_empty() {
                lines.push(no_output);
            }
            let _ = tx.send(lines.join("\n"));
            ctx_clone.request_repaint();
        });
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        ctx: &egui::Context,
        backend: &BackendClient,
        run_center_enabled: bool,
    ) {
egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        self.process_pending(i18n);

        ui.heading(i18n.t("tab.workflow"));
        let text = i18n.t("workflow.hint").to_string();
        let resp = ui.label(&text);
        resp.context_menu(|ui| {
            if ui.button("📋 Copy").clicked() {
                ui.ctx().copy_text(text.clone());
                ui.close_menu();
            }
        });
        ui.separator();

        if !run_center_enabled {
            ui.label(i18n.t("workflow.runCenter.hidden"));
            ui.add_space(6.0);
        }

        let security = security_prefs::load();

        ui.horizontal(|ui| {
            let text = i18n.t("workflow.step").to_string();
            let resp = ui.label(&text);
            resp.context_menu(|ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close_menu();
                }
            });
            ui.text_edit_singleline(&mut self.new_name);
            let text = i18n.t("workflow.command").to_string();
            let resp = ui.label(&text);
            resp.context_menu(|ui| {
                if ui.button("📋 Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close_menu();
                }
            });
            ui.text_edit_singleline(&mut self.new_command);
            if ui.button(i18n.t("workflow.add")).clicked() {
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
            let text = i18n.t("workflow.noSteps").to_string();
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
                        i18n.t("workflow.confirmDelete")
                    } else {
                        i18n.t("workflow.delete")
                    };
                    if ui.button(delete_label).clicked() {
                        if security.confirm_dangerous_actions
                            && self.pending_confirm_delete != Some(idx)
                        {
                            self.pending_confirm_delete = Some(idx);
                            self.state.last_result = Some(format!(
                                "{}",
                                i18n.t("workflow.deleteConfirmAgain")
                                    .replace("{name}", &step.name)
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
            i18n.t("workflow.confirmRun")
        } else {
            i18n.t("workflow.run")
        };
        if ui
            .add_enabled(!self.running, egui::Button::new(run_label))
            .clicked()
        {
            if security.confirm_dangerous_actions && !self.pending_confirm_run {
                self.pending_confirm_run = true;
                self.state.last_result = Some(i18n.t("workflow.runConfirmAgain").to_string());
                changed = true;
            } else {
                self.pending_confirm_run = false;
                let options = AutoTuneView::load_runtime_options();
                let task = self
                    .state
                    .steps
                    .iter()
                    .filter(|s| s.enabled)
                    .map(|s| format!("{}: {}", s.name, s.command))
                    .collect::<Vec<_>>()
                    .join("\n");
                let backend_clone = backend.clone();
                let ctx_clone = ctx.clone();
                tokio::spawn(async move {
                    let _ = backend_clone
                        .execute_workflow(&task, None, Some(options))
                        .await;
                    ctx_clone.request_repaint();
                });
                self.trigger_run(i18n, ctx);
            }
        }

        if run_center_enabled {
            ui.add_space(14.0);
            ui.separator();
            ui.heading(i18n.t("workflow.runCenter.title"));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("workflow_run_filter")
                    .selected_text(Self::status_label(i18n, &self.run_status_filter))
                    .show_ui(ui, |ui| {
                        for status in [
                            "all",
                            "queued",
                            "running",
                            "paused",
                            "succeeded",
                            "failed",
                            "cancelled",
                        ] {
                            ui.selectable_value(
                                &mut self.run_status_filter,
                                status.to_string(),
                                Self::status_label(i18n, status),
                            );
                        }
                    });
                if ui
                    .add_enabled(
                        !self.runs_loading,
                        egui::Button::new(i18n.t("workflow.runCenter.refresh")),
                    )
                    .clicked()
                {
                    self.runs_loading = true;
                    self.run_center_msg.clear();
                    let backend_clone = backend.clone();
                    let filter = self.run_status_filter.clone();
                    let tx = self.pending_tx.clone();
                    let ctx_clone = ctx.clone();
                    tokio::spawn(async move {
                        let status = if filter == "all" {
                            None
                        } else {
                            Some(filter.as_str())
                        };
                        // Add timeout to prevent hanging
                        let result = match tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            backend_clone.list_workflow_runs(50, 0, status),
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                eprintln!("Warning: list_workflow_runs timed out");
                                Err("timeout".to_string())
                            }
                        };
                        let msg = match result {
                            Ok(v) => format!("__runs__:{v}"),
                            Err(e) => format!("__runs_error__:{e}"),
                        };
                        let _ = tx.send(msg);
                        ctx_clone.request_repaint();
                    });
                }
            });

            if !self.run_center_msg.is_empty() {
                ui.colored_label(egui::Color32::RED, &self.run_center_msg);
            }

            for run in &self.runs {
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(self.selected_run_id == run.run_id, &run.run_id)
                        .clicked()
                    {
                        self.selected_run_id = run.run_id.clone();
                        self.selected_run_detail = None;
                        let run_id = run.run_id.clone();
                        let backend_clone = backend.clone();
                        let tx = self.pending_tx.clone();
                        let ctx_clone = ctx.clone();
                        tokio::spawn(async move {
                            // Add timeout to prevent hanging
                            let result = match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                backend_clone.get_workflow_run(&run_id),
                            )
                            .await
                            {
                                Ok(r) => r,
                                Err(_) => {
                                    eprintln!("Warning: get_workflow_run timed out");
                                    Err("timeout".to_string())
                                }
                            };
                            let msg = match result {
                                Ok(v) => format!("__run_detail__:{v}"),
                                Err(e) => format!("__run_detail_error__:{e}"),
                            };
                            let _ = tx.send(msg);
                            ctx_clone.request_repaint();
                        });
                    }
                    ui.label(format!(
                        "{} [{}]",
                        run.task,
                        Self::status_label(i18n, &run.status)
                    ));
                });
            }

            if !self.selected_run_id.is_empty() {
                ui.horizontal(|ui| {
                    for (label, action) in [
                        (i18n.t("workflow.pause"), "pause"),
                        (i18n.t("workflow.resume"), "resume"),
                        (i18n.t("workflow.cancel"), "cancel"),
                    ] {
                        if ui.button(label).clicked() {
                            let run_id = self.selected_run_id.clone();
                            let backend_clone = backend.clone();
                            let tx = self.pending_tx.clone();
                            let ctx_clone = ctx.clone();
                            let requested_tpl = i18n.t("workflow.runActionRequested").to_string();
                            let failed_tpl = i18n.t("workflow.runActionFailed").to_string();
                            tokio::spawn(async move {
                                // Add timeout to prevent hanging
                                let result = match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    backend_clone.transition_workflow_run(&run_id, action),
                                )
                                .await
                                {
                                    Ok(r) => r,
                                    Err(_) => {
                                        eprintln!("Warning: transition_workflow_run timed out");
                                        Err("timeout".to_string())
                                    }
                                };
                                let msg = match result {
                                    Ok(_) => requested_tpl
                                        .replace("{run_id}", &run_id)
                                        .replace("{action}", action),
                                    Err(e) => failed_tpl
                                        .replace("{run_id}", &run_id)
                                        .replace("{action}", action)
                                        .replace("{error}", &e.to_string()),
                                };
                                let _ = tx.send(msg);
                                ctx_clone.request_repaint();
                            });
                        }
                    }
                });

                if let Some(run) = &self.selected_run_detail {
                    ui.add_space(6.0);
                    ui.label(format!("{}: {}", i18n.t("workflow.phase"), run.phase));
                    if let Some(error) = &run.error {
                        ui.colored_label(
                            egui::Color32::RED,
                            format!("{}: {}", i18n.t("workflow.error"), error),
                        );
                    }
                    if !run.artifacts.is_empty() {
                        ui.label(i18n.t("workflow.artifacts"));
                        for artifact in &run.artifacts {
                            ui.label(format!("- {}", artifact));
                        }
                    }
                }
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
        });
    }
}
