use crate::backend::{BackendClient, WorkflowRunRecord};
use crate::i18n::I18n;
use crate::views::autotune::AutoTuneView;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

enum WorkflowEvent {
    RunsResult {
        request_id: u64,
        result: Result<Vec<WorkflowRunRecord>, String>,
    },
    RunDetailResult {
        request_id: u64,
        run_id: String,
        result: Result<WorkflowRunRecord, String>,
    },
    WorkflowExecuteDone(String),
    UiMessage(String),
}

pub struct WorkflowView {
    state: WorkflowState,
    new_name: String,
    new_command: String,
    running: bool,
    pending_confirm_run: bool,
    pending_confirm_delete: Option<usize>,
    pending_rx: mpsc::Receiver<WorkflowEvent>,
    pending_tx: mpsc::Sender<WorkflowEvent>,
    runs: Vec<WorkflowRunRecord>,
    selected_run_id: String,
    selected_run_detail: Option<WorkflowRunRecord>,
    run_status_filter: String,
    runs_loading: bool,
    runs_request_seq: u64,
    run_detail_in_flight: bool,
    run_detail_request_seq: u64,
    last_requested_run_detail_id: String,
    run_center_msg: String,
    last_run_center_poll: Instant,
    /// Cached security prefs — reloaded at most once per 10s to avoid per-frame disk reads.
    cached_security: crate::views::security_prefs::SecurityPrefs,
    security_last_load: Instant,
}

impl WorkflowView {
    fn estimated_progress(run: &WorkflowRunRecord) -> f32 {
        match run.status.as_str() {
            "queued" => 0.05,
            "running" | "paused" => {
                let elapsed = Self::run_duration_secs(run).unwrap_or(0) as f32;
                (elapsed / 300.0).clamp(0.08, 0.92)
            }
            "succeeded" | "failed" | "cancelled" => 1.0,
            _ => 0.0,
        }
    }

    fn estimated_remaining_secs(run: &WorkflowRunRecord) -> Option<i64> {
        if !matches!(run.status.as_str(), "queued" | "running" | "paused") {
            return None;
        }

        let elapsed = Self::run_duration_secs(run).unwrap_or(0);
        Some((300 - elapsed).max(0))
    }

    fn format_local_ts(ts: i64) -> String {
        use chrono::{Local, TimeZone};
        Local
            .timestamp_opt(ts, 0)
            .single()
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| ts.to_string())
    }

    fn run_duration_secs(run: &WorkflowRunRecord) -> Option<i64> {
        let started = run.started_at?;
        let end = run.ended_at.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64
        });
        Some((end - started).max(0))
    }

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

    fn status_color(status: &str) -> egui::Color32 {
        match status {
            "queued" => egui::Color32::from_rgb(240, 180, 70),
            "running" => egui::Color32::from_rgb(80, 180, 120),
            "paused" => egui::Color32::from_rgb(140, 145, 160),
            "succeeded" => egui::Color32::from_rgb(70, 175, 110),
            "failed" => egui::Color32::from_rgb(220, 90, 90),
            "cancelled" => egui::Color32::from_rgb(170, 130, 130),
            _ => egui::Color32::from_rgb(140, 145, 160),
        }
    }

    fn request_runs(&mut self, backend: &BackendClient, ctx: &egui::Context) {
        self.runs_request_seq = self.runs_request_seq.wrapping_add(1);
        let request_id = self.runs_request_seq;
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
            let result: Result<Vec<WorkflowRunRecord>, String> = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                backend_clone.list_workflow_runs_typed(50, 0, status),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => {
                    eprintln!("Warning: list_workflow_runs timed out");
                    Err("timeout".to_string())
                }
            };
            let _ = tx.send(WorkflowEvent::RunsResult { request_id, result });
            ctx_clone.request_repaint();
        });
    }

    fn request_run_detail(&mut self, run_id: &str, backend: &BackendClient, ctx: &egui::Context) {
        if run_id.is_empty() {
            return;
        }
        if self.run_detail_in_flight && self.last_requested_run_detail_id == run_id {
            return;
        }
        self.run_detail_request_seq = self.run_detail_request_seq.wrapping_add(1);
        let request_id = self.run_detail_request_seq;
        self.run_detail_in_flight = true;
        self.last_requested_run_detail_id = run_id.to_string();

        let run_id = run_id.to_string();
        let backend_clone = backend.clone();
        let tx = self.pending_tx.clone();
        let ctx_clone = ctx.clone();
        tokio::spawn(async move {
            let result: Result<WorkflowRunRecord, String> = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                backend_clone.get_workflow_run_typed(&run_id),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => {
                    eprintln!("Warning: get_workflow_run timed out");
                    Err("timeout".to_string())
                }
            };
            let _ = tx.send(WorkflowEvent::RunDetailResult {
                request_id,
                run_id,
                result,
            });
            ctx_clone.request_repaint();
        });
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
            runs_request_seq: 0,
            run_detail_in_flight: false,
            run_detail_request_seq: 0,
            last_requested_run_detail_id: String::new(),
            run_center_msg: String::new(),
            last_run_center_poll: Instant::now(),
            cached_security: crate::views::security_prefs::load(),
            security_last_load: Instant::now(),
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

    fn process_pending(&mut self, _i18n: &I18n) {
        // Limit event processing per frame to prevent UI freeze
        const MAX_EVENTS_PER_FRAME: usize = 8;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Ok(result) = self.pending_rx.try_recv() else {
                break;
            };
            match result {
                WorkflowEvent::RunsResult { request_id, result } => {
                    if request_id != self.runs_request_seq {
                        continue;
                    }
                    self.runs_loading = false;
                    match result {
                        Ok(runs) => {
                            self.runs = runs;
                            self.run_center_msg.clear();
                            if self.selected_run_id.is_empty() {
                                if let Some(first) = self.runs.first() {
                                    self.selected_run_id = first.run_id.clone();
                                }
                            } else if !self.runs.iter().any(|r| r.run_id == self.selected_run_id) {
                                self.selected_run_id.clear();
                                self.selected_run_detail = None;
                            }
                        }
                        Err(err) => {
                            self.run_center_msg = err;
                        }
                    }
                }
                WorkflowEvent::RunDetailResult {
                    request_id,
                    run_id,
                    result,
                } => {
                    if request_id != self.run_detail_request_seq {
                        continue;
                    }
                    if self.last_requested_run_detail_id == run_id {
                        self.run_detail_in_flight = false;
                    }
                    if run_id != self.selected_run_id {
                        continue;
                    }
                    match result {
                        Ok(run) => {
                            self.selected_run_detail = Some(run);
                            self.run_center_msg.clear();
                        }
                        Err(err) => {
                            self.run_center_msg = err;
                        }
                    }
                }
                WorkflowEvent::WorkflowExecuteDone(payload) => {
                    self.state.last_result = Some(payload);
                    self.state.last_run_at = Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                    self.running = false;
                    self.pending_confirm_run = false;
                    self.save_state();
                }
                WorkflowEvent::UiMessage(msg) => {
                    self.state.last_run_at = Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                    self.state.last_result = Some(msg);
                    self.running = false;
                    self.save_state();
                }
            }
        }
    }

    fn trigger_run(&mut self, i18n: &I18n, ctx: &egui::Context, backend: &BackendClient) {
        let task = self
            .state
            .steps
            .iter()
            .filter(|s| s.enabled)
            .map(|s| format!("{}: {}", s.name, s.command))
            .collect::<Vec<_>>()
            .join("\n");
        if task.trim().is_empty() {
            self.state.last_result = Some(i18n.t("workflow.noEnabledSteps").to_string());
            self.save_state();
            return;
        }

        self.running = true;
        self.state.last_result = Some(i18n.t("workflow.running").to_string());
        self.save_state();

        let tx = self.pending_tx.clone();
        let ctx_clone = ctx.clone();
        let backend = backend.clone();
        tokio::spawn(async move {
            let payload = match tokio::time::timeout(
                std::time::Duration::from_secs(20),
                backend.execute_workflow(&task, None, Some(AutoTuneView::load_runtime_options())),
            )
            .await
            {
                Ok(Ok(result)) => {
                    serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
                }
                Ok(Err(err)) => format!("workflow.execute failed: {err}"),
                Err(_) => "workflow.execute timed out".to_string(),
            };
            let _ = tx.send(WorkflowEvent::WorkflowExecuteDone(payload));
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
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.process_pending(i18n);

                ui.heading(i18n.t("tab.workflow"));
                let text = i18n.t("workflow.hint").to_string();
                let resp = ui.label(&text);
                resp.context_menu(|ui| {
                    if ui.button(i18n.t("common.copyButton")).clicked() {
                        ui.ctx().copy_text(text.clone());
                        ui.close_menu();
                    }
                });
                ui.separator();

                if !run_center_enabled {
                    ui.label(i18n.t("workflow.runCenter.hidden"));
                    ui.add_space(6.0);
                }

                // Reload security prefs at most once per 10 seconds to avoid per-frame disk reads.
                if self.security_last_load.elapsed() >= std::time::Duration::from_secs(10) {
                    self.cached_security = crate::views::security_prefs::load();
                    self.security_last_load = Instant::now();
                }
                // Copy the needed bool to avoid holding a borrow over closures.
                let confirm_dangerous = self.cached_security.confirm_dangerous_actions;

                ui.horizontal(|ui| {
                    let text = i18n.t("workflow.step").to_string();
                    let resp = ui.label(&text);
                    resp.context_menu(|ui| {
                        if ui.button(i18n.t("common.copyButton")).clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close_menu();
                        }
                    });
                    ui.text_edit_singleline(&mut self.new_name);
                    let text = i18n.t("workflow.command").to_string();
                    let resp = ui.label(&text);
                    resp.context_menu(|ui| {
                        if ui.button(i18n.t("common.copyButton")).clicked() {
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
                        if ui.button(i18n.t("common.copyButton")).clicked() {
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
                                if ui.button(i18n.t("common.copyButton")).clicked() {
                                    ui.ctx().copy_text(text.clone());
                                    ui.close_menu();
                                }
                            });
                            ui.separator();
                            let text = step.command.clone();
                            let resp = ui.label(&text);
                            resp.context_menu(|ui| {
                                if ui.button(i18n.t("common.copyButton")).clicked() {
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
                                if confirm_dangerous && self.pending_confirm_delete != Some(idx) {
                                    self.pending_confirm_delete = Some(idx);
                                    self.state.last_result = Some(
                                        i18n.t("workflow.deleteConfirmAgain")
                                            .replace("{name}", &step.name)
                                            .to_string(),
                                    );
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
                    if confirm_dangerous && !self.pending_confirm_run {
                        self.pending_confirm_run = true;
                        self.state.last_result =
                            Some(i18n.t("workflow.runConfirmAgain").to_string());
                        changed = true;
                    } else {
                        self.pending_confirm_run = false;
                        self.trigger_run(i18n, ctx, backend);
                    }
                }

                if run_center_enabled {
                    // Auto-refresh run center while there is active work, so operators don't have to spam refresh.
                    let active_selected = self
                        .selected_run_detail
                        .as_ref()
                        .map(|r| matches!(r.status.as_str(), "queued" | "running" | "paused"))
                        .or_else(|| {
                            self.runs
                                .iter()
                                .find(|r| r.run_id == self.selected_run_id)
                                .map(|r| {
                                    matches!(r.status.as_str(), "queued" | "running" | "paused")
                                })
                        })
                        .unwrap_or(false);
                    if active_selected
                        && !self.runs_loading
                        && self.last_run_center_poll.elapsed() >= std::time::Duration::from_secs(3)
                    {
                        self.request_runs(backend, ctx);
                        if !self.selected_run_id.is_empty() {
                            let run_id = self.selected_run_id.clone();
                            self.request_run_detail(&run_id, backend, ctx);
                        }
                        self.last_run_center_poll = Instant::now();
                    }

                    // Cross-platform: Ctrl+R (Win/Linux) or Command+R (macOS) to refresh run center.
                    let mut quick_refresh = false;
                    ui.input_mut(|i| {
                        if i.consume_key(egui::Modifiers::CTRL, egui::Key::R)
                            || i.consume_key(egui::Modifiers::COMMAND, egui::Key::R)
                        {
                            quick_refresh = true;
                        }
                    });
                    if quick_refresh && !self.runs_loading {
                        self.request_runs(backend, ctx);
                        if !self.selected_run_id.is_empty() {
                            let run_id = self.selected_run_id.clone();
                            self.request_run_detail(&run_id, backend, ctx);
                        }
                        self.last_run_center_poll = Instant::now();
                    }

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
                            self.request_runs(backend, ctx);
                            self.last_run_center_poll = Instant::now();
                        }
                    });

                    if !self.run_center_msg.is_empty() {
                        ui.colored_label(egui::Color32::RED, &self.run_center_msg);
                    }

                    let mut pending_detail_run_id: Option<String> = None;
                    for run in &self.runs {
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(self.selected_run_id == run.run_id, &run.run_id)
                                .clicked()
                            {
                                self.selected_run_id = run.run_id.clone();
                                self.selected_run_detail = None;
                                pending_detail_run_id = Some(run.run_id.clone());
                            }
                            ui.colored_label(Self::status_color(&run.status), "●");
                            ui.label(format!(
                                "{} [{}]",
                                run.task,
                                Self::status_label(i18n, &run.status)
                            ));
                            if let Some(duration_secs) = Self::run_duration_secs(run) {
                                ui.label(format!("{}s", duration_secs));
                            }
                        });
                    }
                    if let Some(run_id) = pending_detail_run_id {
                        self.request_run_detail(&run_id, backend, ctx);
                    }

                    if !self.selected_run_id.is_empty() {
                        // Determine the current status of the selected run from the list or detail.
                        let selected_status = self
                            .selected_run_detail
                            .as_ref()
                            .map(|r| r.status.as_str())
                            .or_else(|| {
                                self.runs
                                    .iter()
                                    .find(|r| r.run_id == self.selected_run_id)
                                    .map(|r| r.status.as_str())
                            })
                            .unwrap_or("");

                        // Build a context-sensitive list of actions based on run status.
                        let available_actions: Vec<(std::borrow::Cow<str>, &str)> =
                            match selected_status {
                                "running" => vec![
                                    (i18n.t("workflow.pause"), "pause"),
                                    (i18n.t("workflow.cancel"), "cancel"),
                                ],
                                "paused" => vec![
                                    (i18n.t("workflow.resume"), "resume"),
                                    (i18n.t("workflow.cancel"), "cancel"),
                                ],
                                "queued" => vec![(i18n.t("workflow.cancel"), "cancel")],
                                _ => vec![],
                            };

                        if !available_actions.is_empty() {
                            ui.horizontal(|ui| {
                                for (label, action) in available_actions {
                                    if ui.button(label).clicked() {
                                        let run_id = self.selected_run_id.clone();
                                        self.run_detail_request_seq =
                                            self.run_detail_request_seq.wrapping_add(1);
                                        let detail_request_id = self.run_detail_request_seq;
                                        self.run_detail_in_flight = true;
                                        self.last_requested_run_detail_id = run_id.clone();
                                        let backend_clone = backend.clone();
                                        let tx = self.pending_tx.clone();
                                        let ctx_clone = ctx.clone();
                                        let requested_tpl =
                                            i18n.t("workflow.runActionRequested").to_string();
                                        let failed_tpl =
                                            i18n.t("workflow.runActionFailed").to_string();
                                        tokio::spawn(async move {
                                            // Add timeout to prevent hanging
                                            let result = match tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone
                                                    .transition_workflow_run(&run_id, action),
                                            )
                                            .await
                                            {
                                                Ok(r) => r,
                                                Err(_) => {
                                                    eprintln!(
                                                    "Warning: transition_workflow_run timed out"
                                                );
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
                                            let _ = tx.send(WorkflowEvent::UiMessage(msg));

                                            // Pull latest run detail after action so UI reflects new state quickly.
                                            if let Ok(Ok(detail)) = tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone.get_workflow_run_typed(&run_id),
                                            )
                                            .await
                                            {
                                                let _ = tx.send(WorkflowEvent::RunDetailResult {
                                                    request_id: detail_request_id,
                                                    run_id: run_id.clone(),
                                                    result: Ok(detail),
                                                });
                                            }
                                            ctx_clone.request_repaint();
                                        });
                                    }
                                }
                            });
                        } // end if !available_actions.is_empty()

                        if let Some(run) = &self.selected_run_detail {
                            ui.add_space(6.0);
                            let estimated_progress = Self::estimated_progress(run);
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.label(i18n.t("workflow.activeSummary"));
                                ui.add(
                                    egui::ProgressBar::new(estimated_progress)
                                        .desired_width(ui.available_width())
                                        .show_percentage(),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    if let Some(duration_secs) = Self::run_duration_secs(run) {
                                        ui.label(format!(
                                            "{}: {}s",
                                            i18n.t("workflow.duration"),
                                            duration_secs
                                        ));
                                    }
                                    if let Some(remaining_secs) =
                                        Self::estimated_remaining_secs(run)
                                    {
                                        ui.label(format!(
                                            "{}: {}s",
                                            i18n.t("workflow.estimatedRemaining"),
                                            remaining_secs
                                        ));
                                    }
                                });
                            });
                            ui.add_space(6.0);
                            ui.label(format!(
                                "{}: {}",
                                i18n.t("workflow.status"),
                                Self::status_label(i18n, &run.status)
                            ));
                            ui.label(format!("{}: {}", i18n.t("workflow.phase"), run.phase));
                            ui.label(format!(
                                "{}: {}",
                                i18n.t("workflow.createdAt"),
                                Self::format_local_ts(run.created_at)
                            ));
                            if let Some(started_at) = run.started_at {
                                ui.label(format!(
                                    "{}: {}",
                                    i18n.t("workflow.startedAt"),
                                    Self::format_local_ts(started_at)
                                ));
                            }
                            if let Some(ended_at) = run.ended_at {
                                ui.label(format!(
                                    "{}: {}",
                                    i18n.t("workflow.endedAt"),
                                    Self::format_local_ts(ended_at)
                                ));
                            }
                            if let Some(duration_secs) = Self::run_duration_secs(run) {
                                ui.label(format!(
                                    "{}: {}s",
                                    i18n.t("workflow.duration"),
                                    duration_secs
                                ));
                            }
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
                        if ui.button(i18n.t("common.copyButton")).clicked() {
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
