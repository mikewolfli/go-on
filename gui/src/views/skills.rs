use crate::backend::{BackendClient, SkillRecord};
use crate::i18n::I18n;
use crate::views::security_prefs;
use std::sync::mpsc;
use std::time::Duration;

enum SkillsUpdate {
    List(Vec<SkillRecord>, Option<String>),
    Create(Result<SkillRecord, String>),
    Import(Result<SkillRecord, String>),
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
    initialized: bool,
    pending_rx: mpsc::Receiver<SkillsUpdate>,
    pending_tx: mpsc::Sender<SkillsUpdate>,
}

impl SkillsView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::channel();
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
            initialized: false,
            pending_rx,
            pending_tx,
        }
    }

    fn upsert_skill(&mut self, skill: SkillRecord) {
        let key = skill.name.clone();
        if let Some(name) = key {
            if let Some(existing) = self
                .skills
                .iter_mut()
                .find(|s| s.name.as_deref() == Some(name.as_str()))
            {
                *existing = skill;
                return;
            }
        }
        self.skills.push(skill);
    }

    fn trigger_refresh(&mut self, backend: &BackendClient, ctx: &egui::Context) {
        self.loading = true;
        self.error.clear();
        let tx = self.pending_tx.clone();
        let backend_clone = backend.clone();
        let ctx_clone = ctx.clone();
        tokio::spawn(async move {
            let result = backend_clone.list_skills().await;
            match result {
                Ok(val) => {
                    let items = val.as_array().cloned().unwrap_or_default();
                    let skills: Vec<SkillRecord> = items
                        .iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect();
                    let _ = tx.send(SkillsUpdate::List(skills, None));
                }
                Err(e) => {
                    let _ = tx.send(SkillsUpdate::List(
                        Vec::new(),
                        Some(format!("Failed to fetch skills: {}", e)),
                    ));
                }
            }
            ctx_clone.request_repaint();
        });
    }

    /// Drain any pending async results
    fn process_pending(&mut self, i18n: &I18n) {
        while let Ok(update) = self.pending_rx.try_recv() {
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
                            self.error.clear();
                            self.upsert_skill(skill);
                            self.create_name.clear();
                            self.create_desc.clear();
                            self.create_prompt.clear();
                            self.show_create = false;
                            self.success = i18n.t("skills.create.success").to_string();
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
                        }
                        Err(e) => {
                            self.success.clear();
                            self.error = e;
                        }
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
    ) {
        self.process_pending(i18n);
        if !self.initialized {
            self.initialized = true;
            self.trigger_refresh(backend, ctx);
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
            ui.colored_label(egui::Color32::RED, &self.error);
            ui.add_space(4.0);
        }

        // Success message
        if !self.success.is_empty() {
            ui.colored_label(egui::Color32::GREEN, &self.success);
            ui.add_space(4.0);
        }

        // Action buttons
        ui.horizontal(|ui| {
            if ui
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
            if ui
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
                self.trigger_refresh(backend, ctx);
            }
        });

        // Create dialog – calls skill.create RPC
        if self.show_create {
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
                        self.error = "Input schema must be a valid JSON object.".to_string();
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
                        tokio::spawn(async move {
                            let result = backend_clone
                                .create_skill(
                                    &name_clone,
                                    &desc_clone,
                                    &prompt_clone,
                                    &schema_clone,
                                )
                                .await;
                            let _ = tx.send(match result {
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
                                Err(e) => SkillsUpdate::Create(Err(format!("RPC error: {}", e))),
                            });
                            ctx_clone.request_repaint();
                        });
                    }
                }
            });
        }

        // Import dialog – currently stores locally (can be extended for remote import)
        if self.show_import {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(i18n.t("skills.import.title"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.import_url)
                            .hint_text(i18n.t("skills.import.placeholder")),
                    );
                    if ui
                        .add_enabled(
                            !self.sending,
                            egui::Button::new(i18n.t("skills.import.btn")),
                        )
                        .clicked()
                    {
                        let security = security_prefs::load();
                        if security.block_external_urls {
                            self.error =
                                "External URL import is blocked by security settings.".to_string();
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
                            let fallback_name = i18n.t("skills.import.unnamed").to_string();
                            let imported_from_tpl =
                                i18n.t("skills.import.importedFrom").to_string();

                            tokio::spawn(async move {
                                let result = async {
                                    if !(url_clone.starts_with("http://")
                                        || url_clone.starts_with("https://"))
                                    {
                                        return Err(
                                            "Invalid URL: must start with http:// or https://"
                                                .to_string(),
                                        );
                                    }

                                    let http = reqwest::Client::builder()
                                        .timeout(Duration::from_secs(15))
                                        .build()
                                        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

                                    let resp = http
                                        .get(&url_clone)
                                        .send()
                                        .await
                                        .map_err(|e| format!("Failed to fetch URL: {e}"))?;
                                    let resp = resp
                                        .error_for_status()
                                        .map_err(|e| format!("HTTP status error: {e}"))?;
                                    let manifest: serde_json::Value = resp
                                        .json()
                                        .await
                                        .map_err(|e| format!("Invalid JSON manifest: {e}"))?;

                                    let name = manifest
                                        .get("name")
                                        .and_then(serde_json::Value::as_str)
                                        .or_else(|| {
                                            url_clone
                                                .split('/')
                                                .next_back()
                                                .filter(|s| !s.is_empty())
                                        })
                                        .unwrap_or(&fallback_name)
                                        .to_string();

                                    let description = manifest
                                        .get("description")
                                        .and_then(serde_json::Value::as_str)
                                        .map(ToString::to_string)
                                        .unwrap_or_else(|| {
                                            imported_from_tpl.replace("{}", &url_clone)
                                        });

                                    let prompt_template = manifest
                                        .get("prompt_template")
                                        .or_else(|| manifest.get("prompt"))
                                        .and_then(serde_json::Value::as_str)
                                        .ok_or_else(|| {
                                            "Manifest missing required field: prompt_template"
                                                .to_string()
                                        })?
                                        .to_string();

                                    let input_schema = manifest
                                        .get("input_schema")
                                        .cloned()
                                        .unwrap_or_else(|| serde_json::json!({"query":"string"}));
                                    let input_schema_str = serde_json::to_string(&input_schema)
                                        .map_err(|e| {
                                            format!("Failed to serialize input_schema: {e}")
                                        })?;

                                    backend_clone
                                        .create_skill(
                                            &name,
                                            &description,
                                            &prompt_template,
                                            &input_schema_str,
                                        )
                                        .await
                                        .map_err(|e| format!("RPC error: {e}"))?;

                                    Ok(SkillRecord {
                                        name: Some(name),
                                        description: Some(description),
                                        version: manifest
                                            .get("version")
                                            .and_then(serde_json::Value::as_str)
                                            .map(ToString::to_string)
                                            .or_else(|| Some("1".to_string())),
                                        enabled: Some(true),
                                        imported_at: Some(
                                            std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs(),
                                        ),
                                    })
                                }
                                .await;

                                let _ = tx.send(SkillsUpdate::Import(result));
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
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(i18n.t("skills.none")).size(14.0).color(egui::Color32::from_rgb(140, 142, 150)));
                ui.add_space(16.0);
                // Default Skill Creator suggestion
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(235, 243, 255))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(16i8, 12i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("\u{1f9e0}");
                            ui.label(
                                egui::RichText::new("Skill Creator")
                                    .color(egui::Color32::from_rgb(0, 106, 255))
                                    .strong(),
                            );
                        });
                        ui.label(
                            egui::RichText::new("Create and manage your own AI skills using natural language. Describe what you want, and this skill will help you build it.")
                                .color(egui::Color32::from_rgb(60, 60, 70))
                                .size(13.0),
                        );
                        ui.add_space(8.0);
                        let btn = egui::Button::new("\u{2795} Create Default Skill")
                            .fill(egui::Color32::from_rgb(0, 106, 255))
                            .min_size(egui::vec2(180.0, 32.0));
                        if ui.add(btn).clicked()
                        {
                            self.sending = true;
                            let tx = self.pending_tx.clone();
                            let backend_clone = backend.clone();
                            let ctx_clone = ctx.clone();
                            tokio::spawn(async move {
                                let result = backend_clone.create_skill(
                                        "create-a-skill",
                                        "Helps you create and manage AI skills through natural language conversation",
                                        "You are a Skill Creator assistant. Your role is to help the user design, create, and manage AI skills.\n\nWhen the user describes a task they want to automate:\n1. Understand the core objective\n2. Suggest a skill name and description\n3. Help them define the input schema\n4. Generate an effective prompt template\n\nAsk clarifying questions to refine the skill design before finalizing.",
                                        r#"{"query": "string", "context": "string"}"#,
                                    ).await;
                                match result {
                                    Ok(_) => {
                                        let _ = tx.send(SkillsUpdate::Create(Ok(SkillRecord {
                                            name: Some("create-a-skill".to_string()),
                                            description: Some("Helps you create and manage AI skills through natural language conversation".to_string()),
                                            version: Some("1".to_string()),
                                            enabled: Some(true),
                                            imported_at: Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()),
                                        })));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(SkillsUpdate::Create(Err(e)));
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

        for skill in &self.skills {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let name = skill.name.as_deref().unwrap_or("unnamed");
                    ui.colored_label(egui::Color32::from_rgb(100, 150, 255), name);
                    if let Some(enabled) = skill.enabled {
                        let (color, label) = if enabled {
                            (egui::Color32::GREEN, "●")
                        } else {
                            (egui::Color32::GRAY, "○")
                        };
                        ui.colored_label(color, label);
                    }
                });
                if let Some(desc) = &skill.description {
                    ui.label(desc);
                }
            });
            ui.add_space(4.0);
        }
    }
}
