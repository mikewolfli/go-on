use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use crate::backend::BackendClient;
use crate::i18n::I18n;
use crate::state_sync::SkillRecord;
use crate::views::security_prefs;

/// Send a SkillsUpdate over a SyncSender with a single try_send attempt.
/// If the channel is full the update is silently dropped — the cache will be
/// refreshed on the next poll cycle.
///
/// No retry loop with thread::sleep is used to avoid blocking the UI thread.
fn send_update(tx: &mpsc::SyncSender<SkillsUpdate>, msg: SkillsUpdate) {
    if tx.try_send(msg).is_err() {
        eprintln!("WARN: skills update dropped (channel full)");
    }
}

#[derive(Clone)]
enum SkillsUpdate {
    List(Vec<SkillRecord>, Option<String>),
    Create(Result<SkillRecord, String>),
    Import(Result<SkillRecord, String>),
    Versions {
        skill_name: String,
        versions: Vec<String>,
        err: Option<String>,
    },
    Message {
        text: String,
        is_error: bool,
    },
}

pub struct SkillsView {
    pub skills: Vec<SkillRecord>,
    pub loading: bool,
    pub error: String,
    pub success: String,
    pub show_create: bool,
    pub create_name: String,
    pub create_desc: String,
    pub create_prompt: String,
    pub create_input_schema: String,
    pub import_url: String,
    pub show_import: bool,
    pub sending: bool,
    pub selected_skill_name: String,
    pub edit_desc: String,
    pub edit_prompt: String,
    pub edit_schema: String,
    pub test_input: String,
    pub rollback_version: String,
    versions_for_skill: String,
    versions: Vec<String>,
    initialized: bool,
    pending_rx: mpsc::Receiver<SkillsUpdate>,
    pending_tx: mpsc::SyncSender<SkillsUpdate>,
    /// Cached security prefs to avoid synchronous disk I/O on the UI thread.
    cached_security: security_prefs::SecurityPrefs,
    /// Timestamp of the last security prefs load.
    security_last_load: Instant,
}

fn cmp_semver(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();
    let max_len = a_parts.len().max(b_parts.len());
    for i in 0..max_len {
        let a_val = a_parts
            .get(i)
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        let b_val = b_parts
            .get(i)
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        match a_val.cmp(&b_val) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    a_parts.len().cmp(&b_parts.len())
}

impl SkillsView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::sync_channel(256);
        Self {
            skills: Vec::new(),
            loading: false,
            error: String::new(),
            success: String::new(),
            show_create: false,
            create_name: String::new(),
            create_desc: String::new(),
            create_prompt: String::new(),
            create_input_schema: r#"{"query": "string"}"#.to_string(),
            import_url: String::new(),
            show_import: false,
            sending: false,
            selected_skill_name: String::new(),
            edit_desc: String::new(),
            edit_prompt: String::new(),
            edit_schema: r#"{"query":"string"}"#.to_string(),
            test_input: r#"{"query":"health check"}"#.to_string(),
            rollback_version: String::new(),
            versions_for_skill: String::new(),
            versions: Vec::new(),
            initialized: false,
            pending_rx,
            pending_tx,
            cached_security: security_prefs::load(),
            security_last_load: Instant::now(),
        }
    }

    fn upsert_skill(&mut self, skill: SkillRecord) {
        if let Some(ref name) = skill.name {
            if let Some(existing) = self
                .skills
                .iter_mut()
                .find(|s| s.name.as_deref() == Some(name))
            {
                *existing = skill;
                return;
            }
        }
        self.skills.push(skill);
    }

    fn load_skill_editor(&mut self, skill: &SkillRecord) {
        self.selected_skill_name = skill.name.clone().unwrap_or_default();
        self.edit_desc = skill.description.clone().unwrap_or_default();
        self.edit_prompt.clear();
        self.edit_schema = r#"{"query":"string"}"#.to_string();
        self.test_input = r#"{"query":"health check"}"#.to_string();
        self.rollback_version = skill.version.clone().unwrap_or_default();
        self.versions_for_skill.clear();
        self.versions.clear();
        self.error.clear();
        self.success.clear();
    }

    pub fn load_skill_editor_by_name(&mut self, name: &str) -> bool {
        if let Some(skill) = self
            .skills
            .iter()
            .find(|skill| skill.name.as_deref() == Some(name))
            .cloned()
        {
            self.load_skill_editor(&skill);
            return true;
        }
        false
    }

    fn trigger_refresh(&mut self, i18n: &I18n, backend: &BackendClient, ctx: &egui::Context) {
        self.loading = true;
        self.error.clear();
        let tx = self.pending_tx.clone();
        let backend_clone = backend.clone();
        let ctx_clone = ctx.clone();
        let fetch_failed = i18n.t("skills.fetchFailed").to_string();
        tokio::spawn(async move {
            // Add timeout to prevent hanging
            let result = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                backend_clone.list_skills(),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => {
                    #[cfg(debug_assertions)]
                    eprintln!("Warning: list_skills timed out");
                    send_update(
                        &tx,
                        SkillsUpdate::List(Vec::new(), Some(format!("{}: timeout", fetch_failed))),
                    );
                    ctx_clone.request_repaint();
                    return;
                }
            };
            match result {
                Ok(val) => {
                    let items = val
                        .get("skills")
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let skills: Vec<SkillRecord> = items
                        .iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect();
                    send_update(&tx, SkillsUpdate::List(skills, None));
                }
                Err(e) => {
                    send_update(
                        &tx,
                        SkillsUpdate::List(Vec::new(), Some(format!("{}: {}", fetch_failed, e))),
                    );
                }
            }
            ctx_clone.request_repaint();
        });
    }

    /// Drain any pending async results
    fn process_pending(&mut self, i18n: &I18n) {
        // Limit event processing per frame to prevent UI freeze
        const MAX_EVENTS_PER_FRAME: usize = 8;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Ok(update) = self.pending_rx.try_recv() else {
                break;
            };
            match update {
                SkillsUpdate::List(skills, err) => {
                    self.loading = false;
                    if let Some(e) = err {
                        self.error = e;
                    } else {
                        self.error.clear();
                        self.skills = skills;
                    }
                }
                SkillsUpdate::Create(result) => {
                    self.sending = false;
                    match result {
                        Ok(skill) => {
                            let is_default_creator =
                                skill.name.as_deref() == Some("create-a-skill");
                            self.error.clear();
                            self.upsert_skill(skill);
                            self.create_name.clear();
                            self.create_desc.clear();
                            self.create_prompt.clear();
                            self.show_create = false;
                            if is_default_creator
                                && self.load_skill_editor_by_name("create-a-skill")
                            {
                                self.success = i18n.t("skills.defaultCreator.loaded").to_string();
                            } else {
                                self.success = i18n.t("skills.create.success").to_string();
                            }
                        }
                        Err(e) => {
                            self.success.clear();
                            self.error = e;
                        }
                    }
                }
                SkillsUpdate::Import(result) => {
                    self.sending = false;
                    match result {
                        Ok(skill) => {
                            self.error.clear();
                            self.upsert_skill(skill);
                            self.import_url.clear();
                            self.show_import = false;
                            self.success = i18n.t("skills.import.success").to_string();
                            // Refresh the full list from backend to ensure consistency
                            self.initialized = false;
                        }
                        Err(e) => {
                            self.success.clear();
                            self.error = e;
                        }
                    }
                }
                SkillsUpdate::Message { text, is_error } => {
                    self.sending = false;
                    if is_error {
                        self.error = text;
                        self.success.clear();
                    } else {
                        self.success = text;
                        self.error.clear();
                    }
                }
                SkillsUpdate::Versions {
                    skill_name,
                    versions,
                    err,
                } => {
                    self.sending = false;
                    if let Some(err) = err {
                        self.error = err;
                        self.success.clear();
                    } else {
                        self.error.clear();
                        self.success = i18n
                            .t("skills.lifecycle.versionCount")
                            .replace("{name}", &skill_name)
                            .replace("{count}", &versions.len().to_string())
                            .to_string();
                        self.versions_for_skill = skill_name;
                        self.versions = versions;
                    }
                }
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
        lifecycle_enabled: bool,
    ) {
        self.process_pending(i18n);

        let _resp = egui::Frame::NONE.show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        if !self.initialized {
            self.initialized = true;
            self.trigger_refresh(i18n, backend, ctx);
        }

        ui.heading(i18n.t("tab.skills"));
        ui.separator();
        ui.add_space(4.0);

        // Loading indicator
        if self.loading {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(i18n.t("skills.loading"));
            });
            ui.add_space(4.0);
        }

        // Error message
        if !self.error.is_empty() {
            let text = self.error.clone();
            let resp = ui.colored_label(egui::Color32::RED, &text);
            resp.context_menu(|ui| {
                if ui.button(i18n.t("common.copyButton")).clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close();
                }
            });
            ui.add_space(4.0);
        }

        // Success message
        if !self.success.is_empty() {
            let text = self.success.clone();
            let resp = ui.colored_label(egui::Color32::from_rgb(20, 120, 70), &text);
            resp.context_menu(|ui| {
                if ui.button(i18n.t("common.copyButton")).clicked() {
                    ui.ctx().copy_text(text.clone());
                    ui.close();
                }
            });
            ui.add_space(4.0);
        }

        // Action buttons
        ui.horizontal(|ui| {
            if lifecycle_enabled
                && ui
                    .button("➕")
                    .on_hover_text(i18n.t("skills.create.title"))
                    .clicked()
            {
                self.show_create = !self.show_create;
                if self.show_create {
                    self.show_import = false;
                }
                self.error.clear();
                self.success.clear();
            }
            if lifecycle_enabled
                && ui
                    .button("📥")
                    .on_hover_text(i18n.t("skills.import.title"))
                    .clicked()
            {
                self.show_import = !self.show_import;
                if self.show_import {
                    self.show_create = false;
                }
                self.error.clear();
                self.success.clear();
            }
            // Refresh button – calls skill.list_imported via RPC
            if ui
                .add_enabled(!self.loading, egui::Button::new("🔄"))
                .on_hover_text(i18n.t("app.refresh"))
                .clicked()
            {
                self.success.clear();
                self.trigger_refresh(i18n, backend, ctx);
            }
        });

        if !lifecycle_enabled {
            ui.label(i18n.t("skills.lifecycle.hidden"));
        }

        // Create dialog – calls skill.create RPC
        if lifecycle_enabled && self.show_create {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(i18n.t("skills.create.title"));
                ui.horizontal(|ui| {
                    ui.label(i18n.t("skills.create.name"));
                    ui.text_edit_singleline(&mut self.create_name);
                });
                ui.horizontal(|ui| {
                    ui.label(i18n.t("skills.create.desc"));
                    ui.text_edit_singleline(&mut self.create_desc);
                });
                ui.label(i18n.t("skills.create.prompt"));
                ui.text_edit_multiline(&mut self.create_prompt);
                ui.label(i18n.t("skills.create.schema"));
                ui.text_edit_multiline(&mut self.create_input_schema);
                if self.sending && self.show_create {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(i18n.t("skills.create.loading"));
                    });
                }
                if ui
                    .add_enabled(
                        !self.sending,
                        egui::Button::new(i18n.t("skills.create.save")),
                    )
                    .clicked()
                {
                    let name = self.create_name.trim().to_string();
                    let desc = self.create_desc.trim().to_string();
                    let prompt = self.create_prompt.trim().to_string();
                    let schema = self.create_input_schema.trim().to_string();

                    if name.is_empty() {
                        self.error = format!(
                            "{} {}",
                            i18n.t("skills.create.error"),
                            i18n.t("skills.create.errorName")
                        );
                    } else if prompt.is_empty() {
                        self.error = format!(
                            "{} {}",
                            i18n.t("skills.create.error"),
                            i18n.t("skills.create.errorPrompt")
                        );
                    } else if serde_json::from_str::<serde_json::Value>(&schema)
                        .ok()
                        .is_none_or(|v| !v.is_object())
                    {
                        self.error = i18n.t("skills.error.invalidSchemaObject").to_string();
                    } else {
                        self.error.clear();
                        self.success.clear();
                        self.sending = true;

                        // Call backend skill.create RPC
                        let tx = self.pending_tx.clone();
                        let backend_clone = backend.clone();
                        let ctx_clone = ctx.clone();
                        let name_clone = name.clone();
                        let desc_clone = desc.clone();
                        let prompt_clone = prompt.clone();
                        let schema_clone = schema.clone();
                        let rpc_error = i18n.t("skills.error.rpc").to_string();
                        tokio::spawn(async move {
                            // Add timeout to prevent hanging
                            let result = match tokio::time::timeout(
                                std::time::Duration::from_secs(15),
                                backend_clone.create_skill(
                                    &name_clone,
                                    &desc_clone,
                                    &prompt_clone,
                                    &schema_clone,
                                ),
                            )
                            .await
                            {
                                Ok(r) => r,
                                Err(_) => {
                                    #[cfg(debug_assertions)]
                                    eprintln!("Warning: create_skill timed out");
                                    Err("timeout".to_string())
                                }
                            };
                            send_update(&tx, match result {
                                Ok(_) => SkillsUpdate::Create(Ok(SkillRecord {
                                    name: Some(name_clone),
                                    description: Some(desc_clone),
                                    version: Some("1".to_string()),
                                    enabled: Some(true),
                                    imported_at: Some(
                                        std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs(),
                                    ),
                                })),
                                Err(e) => {
                                    SkillsUpdate::Create(Err(format!("{}: {}", rpc_error, e)))
                                }
                            });
                            ctx_clone.request_repaint();
                        });
                    }
                }
            });
        }

        // Import dialog – currently stores locally (can be extended for remote import)
        if lifecycle_enabled && self.show_import {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(i18n.t("skills.import.title"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.import_url)
                            .hint_text(i18n.t("skills.import.placeholder")),
                    );
                    if self.sending && self.show_import {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(i18n.t("skills.import.loading"));
                        });
                    }
                    if ui
                        .add_enabled(
                            !self.sending,
                            egui::Button::new(i18n.t("skills.import.btn")),
                        )
                        .clicked()
                    {
                        // Reload security prefs at most once per 10 seconds (avoids synchronous I/O on UI thread)
                        if self.security_last_load.elapsed() >= Duration::from_secs(10) {
                            self.cached_security = security_prefs::load();
                            self.security_last_load = Instant::now();
                        }
                        if self.cached_security.block_external_urls {
                            self.error = i18n.t("skills.import.blockedBySecurity").to_string();
                            self.success.clear();
                            return;
                        }

                        let url = self.import_url.trim().to_string();
                        if url.is_empty() {
                            self.error = format!(
                                "{} {}",
                                i18n.t("skills.import.error"),
                                i18n.t("skills.import.errorUrl")
                            );
                        } else {
                            self.error.clear();
                            self.success.clear();
                            self.sending = true;

                            let tx = self.pending_tx.clone();
                            let backend_clone = backend.clone();
                            let ctx_clone = ctx.clone();
                            let url_clone = url.clone();
                            let rpc_error = i18n.t("skills.error.rpc").to_string();
                            let invalid_url = i18n.t("skills.import.invalidUrl").to_string();

                            tokio::spawn(async move {
                                let result = async {
                                    if !(url_clone.starts_with("http://")
                                        || url_clone.starts_with("https://"))
                                    {
                                        return Err(invalid_url);
                                    }

                                    // Determine import source: GitHub repo or direct URL
                                    let source = if let Some(repo_path) = url_clone
                                        .strip_prefix("https://github.com/")
                                        .or_else(|| url_clone.strip_prefix("http://github.com/"))
                                    {
                                        // GitHub repo URL: extract owner/repo
                                        let repo = repo_path
                                            .trim_start_matches('/')
                                            .split('/')
                                            .take(2)
                                            .collect::<Vec<_>>()
                                            .join("/");
                                        let repo = repo.trim_end_matches(".git").to_string();
                                        serde_json::json!({
                                            "kind": "github",
                                            "repo": repo,
                                            "ref": "main"
                                        })
                                    } else {
                                        // Direct URL
                                        serde_json::json!({
                                            "kind": "url",
                                            "url": url_clone
                                        })
                                    };

                                    // Use backend's skill.import RPC (handles GitHub and URLs)
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(30),
                                        backend_clone.import_skill(source),
                                    )
                                    .await
                                    {
                                        Ok(Ok(result)) => {
                                            // Prefer 'name' from backend response; fall back
                                            // to the repo/name portion of the URL for robustness.
                                            let fallback_name = url_clone
                                                .trim_end_matches('/')
                                                .rsplit('/')
                                                .next()
                                                .unwrap_or("imported")
                                                .to_string();
                                            let name = result
                                                .get("name")
                                                .and_then(serde_json::Value::as_str)
                                                .filter(|s| !s.is_empty())
                                                .unwrap_or(&fallback_name)
                                                .to_string();
                                            let description = result
                                                .get("description")
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or("")
                                                .to_string();
                                            let version = result
                                                .get("version")
                                                .and_then(serde_json::Value::as_str)
                                                .map(ToString::to_string);
                                            Ok(SkillRecord {
                                                name: Some(name),
                                                description: Some(description),
                                                version,
                                                enabled: Some(false),
                                                imported_at: Some(
                                                    std::time::SystemTime::now()
                                                        .duration_since(std::time::UNIX_EPOCH)
                                                        .unwrap_or_default()
                                                        .as_secs(),
                                                ),
                                            })
                                        }
                                        Ok(Err(e)) => {
                                            Err(format!("{}: {e}", rpc_error))
                                        }
                                        Err(_) => {
                                            Err(format!("{}: timeout", rpc_error))
                                        }
                                    }
                                }
                                .await;

                                send_update(&tx, SkillsUpdate::Import(result));
                                ctx_clone.request_repaint();
                            });
                        }
                    }
                });
            });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // Skills list / empty state with default skill suggestion
        if self.skills.is_empty() && !self.loading {
            let dark = ui.visuals().dark_mode;
            let muted_text = if dark {
                egui::Color32::from_rgb(140, 142, 150)
            } else {
                egui::Color32::from_rgb(110, 112, 120)
            };
            let link_color = if dark {
                egui::Color32::from_rgb(100, 170, 255)
            } else {
                egui::Color32::from_rgb(0, 106, 255)
            };
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(i18n.t("skills.none")).size(14.0).color(muted_text));
                ui.add_space(16.0);
                // Default Skill Creator suggestion
                let suggest_bg = if dark {
                    egui::Color32::from_rgb(30, 40, 60)
                } else {
                    egui::Color32::from_rgb(235, 243, 255)
                };
                let desc_color = if dark {
                    egui::Color32::from_rgb(200, 200, 210)
                } else {
                    egui::Color32::from_rgb(60, 60, 70)
                };
                egui::Frame::new()
                    .fill(suggest_bg)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(16i8, 12i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("\u{1f9e0}");
                            ui.label(
                                egui::RichText::new(i18n.t("skills.defaultCreator.title"))
                                    .color(link_color)
                                    .strong(),
                            );
                        });
                        ui.label(
                            egui::RichText::new(i18n.t("skills.defaultCreator.description"))
                                .color(desc_color)
                                .size(13.0),
                        );
                        ui.add_space(8.0);
                        let btn = egui::Button::new(i18n.t("skills.defaultCreator.button"))
                            .fill(link_color)
                            .min_size(egui::vec2(180.0, 32.0));
                        if ui.add(btn).clicked()
                        {
                            if self.load_skill_editor_by_name("create-a-skill") {
                                self.error.clear();
                                self.success = i18n.t("skills.defaultCreator.loaded").to_string();
                                return;
                            }
                            self.sending = true;
                            let tx = self.pending_tx.clone();
                            let backend_clone = backend.clone();
                            let ctx_clone = ctx.clone();
                            let seed_description =
                                i18n.t("skills.defaultCreator.description").to_string();
                            let default_loaded = i18n.t("skills.defaultCreator.loaded").to_string();
                            tokio::spawn(async move {
                                // Add timeout to prevent hanging
                                let result = match tokio::time::timeout(
                                    std::time::Duration::from_secs(15),
                                    backend_clone.create_skill(
                                        "create-a-skill",
                                        &seed_description,
                                        "You are a Skill Creator assistant. Your role is to help the user design, create, and manage AI skills.\n\nWhen the user describes a task they want to automate:\n1. Understand the core objective\n2. Suggest a skill name and description\n3. Help them define the input schema\n4. Generate an effective prompt template\n\nAsk clarifying questions to refine the skill design before finalizing.",
                                        r#"{"query": "string", "context": "string"}"#,
                                    )
                                ).await {
                                    Ok(r) => r,
                                    Err(_) => {
                                        #[cfg(debug_assertions)]
                                        eprintln!("Warning: create-a-skill creation timed out");
                                        Err("timeout".to_string())
                                    }
                                };
                                match result {
                                    Ok(_) => {
                                        send_update(&tx, SkillsUpdate::Create(Ok(SkillRecord {
                                            name: Some("create-a-skill".to_string()),
                                            description: Some(seed_description),
                                            version: Some("1".to_string()),
                                            enabled: Some(true),
                                            imported_at: Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()),
                                        })));
                                    }
                                    Err(e) => {
                                        if e.contains("already")
                                            || e.contains("已存在")
                                            || e.contains("已注册")
                                            || e.contains("已註冊")
                                        {
                                            send_update(&tx, SkillsUpdate::Message {
                                                text: default_loaded,
                                                is_error: false,
                                            });
                                        } else {
                                            send_update(&tx, SkillsUpdate::Create(Err(e)));
                                        }
                                    }
                                }
                                ctx_clone.request_repaint();
                            });
                        }
                    });
            });
            return;
        }

        if self.skills.is_empty() {
            return;
        }

        let mut open_editor_for: Option<usize> = None;
        for idx in 0..self.skills.len() {
            let (skill_name_opt, skill_enabled, skill_version, skill_description) = {
                let skill = &self.skills[idx];
                (
                    skill.name.clone(),
                    skill.enabled,
                    skill.version.clone(),
                    skill.description.clone(),
                )
            };
            egui::Frame::group(ui.style()).show(ui, |ui| {
                let dark = ui.visuals().dark_mode;
                let skill_color = if dark {
                    egui::Color32::from_rgb(100, 150, 255)
                } else {
                    egui::Color32::from_rgb(40, 80, 180)
                };
                let muted_text = if dark {
                    egui::Color32::from_rgb(140, 142, 150)
                } else {
                    egui::Color32::from_rgb(110, 112, 120)
                };
                ui.horizontal(|ui| {
                    let unnamed = i18n.t("skills.import.unnamed");
                    let name_text = skill_name_opt.as_deref().unwrap_or(&unnamed);
                    ui.colored_label(skill_color, name_text);
                    if let Some(enabled) = skill_enabled {
                        let (color, label) = if enabled {
                            (egui::Color32::from_rgb(20, 120, 70), "●")
                        } else {
                            (egui::Color32::GRAY, "○")
                        };
                        ui.colored_label(color, label);
                    }
                    if let Some(ver) = &skill_version {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("v{ver}"))
                                    .small()
                                    .color(muted_text),
                            );
                        });
                    }
                });
                if let Some(desc) = &skill_description {
                    ui.label(desc);
                }
                if lifecycle_enabled {
                    ui.horizontal(|ui| {
                        let name = skill_name_opt.clone().unwrap_or_default();
                        let is_enabled = skill_enabled.unwrap_or(true);
                        let toggle_label = if is_enabled {
                            i18n.t("skills.lifecycle.disable")
                        } else {
                            i18n.t("skills.lifecycle.enable")
                        };
                        if ui.button(i18n.t("skills.lifecycle.edit")).clicked() && !name.is_empty()
                        {
                            open_editor_for = Some(idx);
                        }
                        if ui.button(toggle_label).clicked() && !name.is_empty() {
                            self.sending = true;
                            let tx = self.pending_tx.clone();
                            let backend_clone = backend.clone();
                            let ctx_clone = ctx.clone();
                            let skill_name = name.clone();
                            let disabled_tpl = i18n.t("skills.lifecycle.disabled").to_string();
                            let enabled_tpl = i18n.t("skills.lifecycle.enabled").to_string();
                            let failed_tpl = i18n.t("skills.lifecycle.toggleFailed").to_string();
                            tokio::spawn(async move {
                                // Add timeout to prevent hanging
                                let result = if is_enabled {
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.disable_skill(&skill_name),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!("Warning: disable_skill timed out");
                                            Err("timeout".to_string())
                                        }
                                    }
                                } else {
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.enable_skill(&skill_name),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!("Warning: enable_skill timed out");
                                            Err("timeout".to_string())
                                        }
                                    }
                                };
                                let is_error = result.is_err();
                                let msg = match result {
                                    Ok(_) => {
                                        if is_enabled {
                                            disabled_tpl.replace("{name}", &skill_name)
                                        } else {
                                            enabled_tpl.replace("{name}", &skill_name)
                                        }
                                    }
                                    Err(e) => failed_tpl
                                        .replace("{name}", &skill_name)
                                        .replace("{error}", &e.to_string()),
                                };
                                send_update(&tx, SkillsUpdate::Message {
                                    text: msg,
                                    is_error,
                                });
                                ctx_clone.request_repaint();
                            });
                        }
                        if ui.button(i18n.t("skills.lifecycle.delete")).clicked()
                            && !name.is_empty()
                        {
                            self.sending = true;
                            let tx = self.pending_tx.clone();
                            let backend_clone = backend.clone();
                            let ctx_clone = ctx.clone();
                            let skill_name = name.clone();
                            let removed_tpl = i18n.t("skills.lifecycle.removed").to_string();
                            let failed_tpl = i18n.t("skills.lifecycle.removeFailed").to_string();
                            tokio::spawn(async move {
                                // Add timeout to prevent hanging
                                let result = match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    backend_clone.remove_skill(&skill_name),
                                )
                                .await
                                {
                                    Ok(r) => r,
                                    Err(_) => {
                                        #[cfg(debug_assertions)]
                                        eprintln!("Warning: remove_skill timed out");
                                        Err("timeout".to_string())
                                    }
                                };
                                let is_error = result.is_err();
                                let msg = match result {
                                    Ok(_) => removed_tpl.replace("{name}", &skill_name),
                                    Err(e) => failed_tpl
                                        .replace("{name}", &skill_name)
                                        .replace("{error}", &e.to_string()),
                                };
                                send_update(&tx, SkillsUpdate::Message {
                                    text: msg,
                                    is_error,
                                });
                                ctx_clone.request_repaint();
                            });
                        }
                        if ui.button(i18n.t("skills.lifecycle.versions")).clicked()
                            && !name.is_empty()
                        {
                            self.sending = true;
                            let tx = self.pending_tx.clone();
                            let backend_clone = backend.clone();
                            let ctx_clone = ctx.clone();
                            let skill_name = name.clone();
                            let failed_tpl = i18n.t("skills.lifecycle.versionsFailed").to_string();
                            tokio::spawn(async move {
                                // Add timeout to prevent hanging
                                let result = match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    backend_clone.list_skill_versions(&skill_name),
                                )
                                .await
                                {
                                    Ok(r) => r,
                                    Err(_) => {
                                        #[cfg(debug_assertions)]
                                        eprintln!("Warning: list_skill_versions timed out");
                                        Err("timeout".to_string())
                                    }
                                };
                                let is_error = result.is_err();
                                if is_error {
                                    let err_text = result.err().unwrap_or_else(|| "unknown".to_string());
                                    // If the skill simply hasn't been imported yet, show a friendly info instead of error.
                                    let err_lower = err_text.to_lowercase();
                                    if err_lower.contains("not found") || err_lower.contains("未找到") || err_lower.contains("not_found") {
                                        send_update(&tx, SkillsUpdate::Versions {
                                                    skill_name,
                                                    versions: Vec::new(),
                                                    err: None,
                                                });
                                    } else {
                                        let msg = failed_tpl
                                            .replace("{name}", &skill_name)
                                            .replace("{error}", &err_text);
                                        send_update(&tx, SkillsUpdate::Versions {
                                            skill_name,
                                            versions: Vec::new(),
                                            err: Some(msg),
                                        });
                                    }
                                } else {
                                    let mut versions = Vec::new();
                                    if let Ok(v) = result {
                                        if let Some(arr) =
                                            v.get("versions").and_then(serde_json::Value::as_array)
                                        {
                                            for item in arr {
                                                if let Some(version) = item.as_str() {
                                                    versions.push(version.to_string());
                                                } else if let Some(version) = item
                                                    .get("version")
                                                    .and_then(serde_json::Value::as_str)
                                                {
                                                    versions.push(version.to_string());
                                                }
                                            }
                                        }
                                    }
                                    if versions.is_empty() {
                                        send_update(&tx, SkillsUpdate::Versions {
                                            skill_name,
                                            versions: Vec::new(),
                                            err: None,
                                        });
                                    } else {
                                        versions.sort_by(|a, b| cmp_semver(b, a).then_with(|| b.cmp(a)));
                                        send_update(&tx, SkillsUpdate::Versions {
                                            skill_name,
                                            versions,
                                            err: None,
                                        });
                                    }

                                }
                                ctx_clone.request_repaint();
                            });
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }

        if let Some(idx) = open_editor_for {
            if let Some(skill) = self.skills.get(idx).cloned() {
                self.load_skill_editor(&skill);
            }
        }

        if lifecycle_enabled && !self.selected_skill_name.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(format!(
                    "{}: {}",
                    i18n.t("skills.lifecycle.editTitle"),
                    self.selected_skill_name
                ));
                ui.horizontal(|ui| {
                    ui.label(i18n.t("skills.create.desc"));
                    ui.text_edit_singleline(&mut self.edit_desc);
                });
                ui.label(i18n.t("skills.lifecycle.promptOverride"));
                ui.text_edit_multiline(&mut self.edit_prompt);
                ui.label(i18n.t("skills.lifecycle.inputSchema"));
                ui.text_edit_multiline(&mut self.edit_schema);
                ui.horizontal(|ui| {
                    if ui.button(i18n.t("skills.lifecycle.saveEdit")).clicked() {
                        let schema =
                            match serde_json::from_str::<serde_json::Value>(&self.edit_schema) {
                                Ok(schema) if schema.is_object() => schema,
                                Ok(_) => {
                                    self.error =
                                        i18n.t("skills.error.invalidSchemaObject").to_string();
                                    return;
                                }
                                Err(e) => {
                                    self.error =
                                        format!("{}: {e}", i18n.t("skills.error.invalidSchema"));
                                    return;
                                }
                            };

                        self.sending = true;
                        let tx = self.pending_tx.clone();
                        let backend_clone = backend.clone();
                        let ctx_clone = ctx.clone();
                        let skill_name = self.selected_skill_name.clone();
                        let desc = if self.edit_desc.trim().is_empty() {
                            None
                        } else {
                            Some(self.edit_desc.trim().to_string())
                        };
                        let prompt = if self.edit_prompt.trim().is_empty() {
                            None
                        } else {
                            Some(self.edit_prompt.trim().to_string())
                        };
                        let updated_tpl = i18n.t("skills.lifecycle.updated").to_string();
                        let failed_tpl = i18n.t("skills.lifecycle.updateFailed").to_string();
                        tokio::spawn(async move {
                            // Add timeout to prevent hanging
                            let result = match tokio::time::timeout(
                                std::time::Duration::from_secs(15),
                                backend_clone.update_skill(
                                    &skill_name,
                                    desc,
                                    prompt,
                                    Some(schema),
                                    None,
                                ),
                            )
                            .await
                            {
                                Ok(r) => r,
                                Err(_) => {
                                    #[cfg(debug_assertions)]
                                    eprintln!("Warning: update_skill timed out");
                                    Err("timeout".to_string())
                                }
                            };
                            let is_error = result.is_err();
                            let msg = match result {
                                Ok(_) => updated_tpl.replace("{name}", &skill_name),
                                Err(e) => failed_tpl
                                    .replace("{name}", &skill_name)
                                    .replace("{error}", &e.to_string()),
                            };
                            send_update(&tx, SkillsUpdate::Message {
                                    text: msg,
                                    is_error,
                                });
                                ctx_clone.request_repaint();
                        });
                    }

                    if ui.button(i18n.t("common.close")).clicked() {
                        self.selected_skill_name.clear();
                    }
                });

                ui.add_space(6.0);
                ui.label(i18n.t("skills.lifecycle.testInput"));
                ui.text_edit_multiline(&mut self.test_input);
                if ui.button(i18n.t("skills.lifecycle.test")).clicked() {
                    let input = match serde_json::from_str::<serde_json::Value>(&self.test_input) {
                        Ok(input) => input,
                        Err(e) => {
                            self.error =
                                format!("{}: {e}", i18n.t("skills.error.invalidTestInput"));
                            return;
                        }
                    };

                    self.sending = true;
                    let tx = self.pending_tx.clone();
                    let backend_clone = backend.clone();
                    let ctx_clone = ctx.clone();
                    let skill_name = self.selected_skill_name.clone();
                    let result_tpl = i18n.t("skills.lifecycle.testResult").to_string();
                    let failed_tpl = i18n.t("skills.lifecycle.testFailed").to_string();
                    tokio::spawn(async move {
                        // Add timeout to prevent hanging (30s for test_skill as it may take longer)
                        let result = match tokio::time::timeout(
                            std::time::Duration::from_secs(30),
                            backend_clone.test_skill(&skill_name, input),
                        )
                        .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                #[cfg(debug_assertions)]
                                eprintln!("Warning: test_skill timed out");
                                Err("timeout".to_string())
                            }
                        };
                        let is_error = result.is_err();
                        let msg = match result {
                            Ok(v) => result_tpl
                                .replace("{name}", &skill_name)
                                .replace("{result}", &v.to_string()),
                            Err(e) => failed_tpl
                                .replace("{name}", &skill_name)
                                .replace("{error}", &e.to_string()),
                        };
                        send_update(&tx, SkillsUpdate::Message {
                            text: msg,
                            is_error,
                        });
                        ctx_clone.request_repaint();
                    });
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(i18n.t("skills.lifecycle.rollbackVersion"));
                    ui.text_edit_singleline(&mut self.rollback_version);
                    if ui.button(i18n.t("skills.lifecycle.rollback")).clicked() {
                        let version = self.rollback_version.trim().to_string();
                        if version.is_empty() {
                            self.error = i18n.t("skills.lifecycle.rollbackRequired").to_string();
                            return;
                        }

                        self.sending = true;
                        let tx = self.pending_tx.clone();
                        let backend_clone = backend.clone();
                        let ctx_clone = ctx.clone();
                        let skill_name = self.selected_skill_name.clone();
                        let rolled_back_tpl = i18n.t("skills.lifecycle.rolledBack").to_string();
                        let failed_tpl = i18n.t("skills.lifecycle.rollbackFailed").to_string();
                        let version_count_tpl = i18n.t("skills.lifecycle.versionCount").to_string();
                        tokio::spawn(async move {
                            let result = match tokio::time::timeout(
                                std::time::Duration::from_secs(15),
                                backend_clone.rollback_skill_version(&skill_name, &version),
                            )
                            .await
                            {
                                Ok(r) => r,
                                Err(_) => {
                                    #[cfg(debug_assertions)]
                                    eprintln!("Warning: rollback_skill_version timed out");
                                    Err("timeout".to_string())
                                }
                            };
                            let is_error = result.is_err();
                            let msg = match result {
                                Ok(_) => {
                                    // Refresh versions right after rollback for immediate consistency.
                                    if let Ok(Ok(v)) = tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.list_skill_versions(&skill_name),
                                    )
                                    .await
                                    {
                                        let mut versions = Vec::new();
                                        if let Some(arr) =
                                            v.get("versions").and_then(serde_json::Value::as_array)
                                        {
                                            for item in arr {
                                                if let Some(ver) = item.as_str() {
                                                    versions.push(ver.to_string());
                                                } else if let Some(ver) = item
                                                    .get("version")
                                                    .and_then(serde_json::Value::as_str)
                                                {
                                                    versions.push(ver.to_string());
                                                }
                                            }
                                        }
                                        versions.sort_by(|a, b| cmp_semver(b, a).then_with(|| b.cmp(a)));
                                        send_update(&tx, SkillsUpdate::Versions {
                                            skill_name: skill_name.clone(),
                                            versions,
                                            err: None,
                                        });
                                    } else {
                                        send_update(&tx, SkillsUpdate::Message {
                                            text: version_count_tpl
                                                .replace("{name}", &skill_name)
                                                .replace("{count}", "0")
                                                .to_string(),
                                            is_error: false,
                                        });
                                    }
                                    rolled_back_tpl
                                        .replace("{name}", &skill_name)
                                        .replace("{version}", &version)
                                }
                                Err(e) => failed_tpl
                                    .replace("{name}", &skill_name)
                                    .replace("{error}", &e.to_string()),
                            };
                            send_update(&tx, SkillsUpdate::Message {
                                text: msg,
                                is_error,
                            });
                            ctx_clone.request_repaint();
                        });
                    }
                });

                if self.versions_for_skill == self.selected_skill_name && !self.versions.is_empty() {
                    ui.add_space(6.0);
                    ui.label(i18n.t("skills.lifecycle.versions"));
                    ui.horizontal_wrapped(|ui| {
                        for ver in &self.versions {
                            let selected = self.rollback_version == *ver;
                            if ui.selectable_label(selected, format!("v{}", ver)).clicked() {
                                self.rollback_version = ver.clone();
                            }
                        }
                    });
                }
            });
        }
        });
        });
    }
}
