use super::*;

impl ChatView {
    fn viewport_height(ui: &egui::Ui) -> f32 {
        let mut h = ui.ctx().screen_rect().height();
        if !h.is_finite() || h <= 0.0 {
            h = 720.0;
        }
        h.clamp(320.0, 1600.0)
    }

    fn bounded_panel_height(ui: &egui::Ui, min_height: f32) -> f32 {
        let mut height = ui.available_height();
        let mut local_height = ui.max_rect().height();
        let viewport_height = Self::viewport_height(ui);

        if !local_height.is_finite() || local_height <= 0.0 {
            local_height = viewport_height;
        }

        if !height.is_finite() || height <= 0.0 {
            height = local_height;
        }

        // Keep within current panel and viewport; this avoids pushing
        // the composer below the visible window when parent reports large height.
        height = height.min(local_height).min(viewport_height);

        height.max(min_height).min(viewport_height)
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
        autotune_chain_enabled: bool,
    ) {
        // Debug: timer to detect hangs
        let _start = Instant::now();
        let debug_enabled = Self::chat_debug_enabled();
        if debug_enabled && !self.debug_log_bootstrapped {
            self.debug_log_bootstrapped = true;
            let path = std::env::temp_dir().join("go-on-chat-debug.log");
            Self::chat_debug_log(&format!(
                "[CHAT_DEBUG_BOOT] logging enabled; file={}",
                path.display()
            ));
        }

        // Log entry
        Self::chat_debug_log("[CHAT_SHOW] Entry");

        // Process any pending async responses (non-blocking)
        self.process_pending(i18n);
        Self::chat_debug_log(&format!(
            "[CHAT_SHOW] process_pending done: {}ms",
            _start.elapsed().as_millis()
        ));

        // Bail out early if processing_pending took too long
        if _start.elapsed().as_millis() > 100 {
            eprintln!(
                "[CHAT_DEBUG] process_pending took {}ms",
                _start.elapsed().as_millis()
            );
        }

        // Lazy initialization of templates and name refresh
        // Capture before bootstrap so we can detect the very first run.
        let is_first_init = !self.templates_bootstrapped;
        Self::chat_debug_log(&format!(
            "[CHAT_SHOW] pre-bootstrap is_first_init={is_first_init}"
        ));
        if is_first_init {
            self.bootstrap_default_templates(i18n);
            // Refresh localized session names once at startup.
            self.refresh_default_session_names(i18n);
            Self::chat_debug_log("[CHAT_SHOW] bootstrap+refresh done");
        }
        self.sync_model_selection();
        Self::chat_debug_log("[CHAT_SHOW] sync_model_selection done");

        // Delayed loading: Schedule backend queries after first render to avoid UI freeze
        // Only schedule once to prevent repeated triggers
        if !self.phases_load_scheduled && !self.phases_loaded {
            self.phases_load_scheduled = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();

            tokio::spawn(async move {
                // Wait 100ms to let UI render first
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                // Add timeout to prevent hanging
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.config_baseline(),
                )
                .await
                {
                    Ok(Ok(baseline)) => {
                        let phases = baseline
                            .get("config")
                            .and_then(|c| c.get("flow"))
                            .and_then(|f| f.get("phases"))
                            .and_then(|p| p.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect::<Vec<_>>()
                            });
                        if let Some(list) = phases {
                            let _ = tx.send(PendingResponse::Phases(list));
                            ctx_clone.request_repaint();
                        }
                    }
                    _ => {
                        eprintln!("Warning: Failed to load phases from backend (timeout or error)");
                    }
                }
            });
        }

        if !self.models_loaded {
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();

            tokio::spawn(async move {
                // Wait 150ms (slightly after phases) to stagger requests
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;

                // Add timeout to prevent hanging
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.fetch_models(),
                )
                .await
                {
                    Ok(models) => {
                        let mut options = vec!["auto".to_string()];
                        let mut ids: Vec<String> = models
                            .into_values()
                            .flat_map(|ids| ids.into_iter())
                            .collect();
                        ids.sort();
                        ids.dedup();
                        options.extend(ids);
                        let _ = tx.send(PendingResponse::Models(options));
                        ctx_clone.request_repaint();
                    }
                    Err(_) => {
                        eprintln!("Warning: Failed to load models from backend (timeout)");
                    }
                }
            });

            self.models_loaded = true;
        }

        let use_safe_mode = (CHAT_SAFE_MODE && CHAT_ISOLATION_STAGE < 6)
            || (CHAT_ISOLATION_STAGE == 6 && CHAT_STAGE6_FORCE_SAFE_LAYOUT_SKELETON);
        Self::chat_debug_log(&format!("[CHAT_SHOW] use_safe_mode={use_safe_mode}"));
        if use_safe_mode {
            let (sidebar_ms, messages_ms, composer_ms) =
                self.show_safe_chat_layout(ui, i18n, backend, ctx, autotune_chain_enabled);

            if debug_enabled {
                let total_ms = _start.elapsed().as_millis();
                Self::push_perf_sample(&mut self.perf_total_samples, total_ms);
                Self::push_perf_sample(&mut self.perf_sidebar_samples, sidebar_ms);
                Self::push_perf_sample(&mut self.perf_messages_samples, messages_ms);
                Self::push_perf_sample(&mut self.perf_composer_samples, composer_ms);
                self.perf_frame_counter = self.perf_frame_counter.saturating_add(1);

                if total_ms > 40 || messages_ms > 30 || composer_ms > 25 || sidebar_ms > 20 {
                    Self::chat_debug_log(&format!(
                        "[CHAT_PERF_SAFE] total={}ms sidebar={}ms messages={}ms composer={}ms sessions={} msgs={}",
                        total_ms,
                        sidebar_ms,
                        messages_ms,
                        composer_ms,
                        self.sessions.len(),
                        self.messages().len()
                    ));
                }
            }
            return;
        }

        // ── Layout: left sidebar (200px) + right content ──────────────
        let mut sidebar_ms: u128 = 0;
        let mut messages_ms: u128 = 0;
        let mut composer_ms: u128 = 0;
        let total_w = ui.available_width();
        let total_h = Self::bounded_panel_height(ui, 320.0);
        let mut sidebar_w = (total_w / 3.0).clamp(180.0, 360.0);
        // Keep enough room for right pane in the same row to avoid wrapping.
        let min_right_w = 420.0;
        let separator_w = 9.0;
        let max_sidebar_w = (total_w - min_right_w - separator_w).max(120.0);
        sidebar_w = sidebar_w.min(max_sidebar_w);

        ui.horizontal_top(|ui| {
            Self::chat_debug_log("[CHAT_SHOW] normal-layout horizontal_top enter");
            ui.allocate_ui(egui::vec2(sidebar_w, total_h), |ui| {
                let t_sidebar = Instant::now();
                ui.set_min_width(sidebar_w);
                ui.set_max_width(sidebar_w);
                self.show_sidebar(ui, i18n);
                sidebar_ms = t_sidebar.elapsed().as_millis();
                Self::chat_debug_log("[CHAT_SHOW] sidebar rendered");
            });
            ui.separator();
            let content_w = ui.available_width().max(120.0);
            ui.allocate_ui(egui::vec2(content_w, total_h), |ui| {
                Self::chat_debug_log("[CHAT_SHOW] right-pane allocate enter");
                let dark_mode = ui.visuals().dark_mode;
                let panel_bg = if dark_mode {
                    egui::Color32::from_rgb(36, 38, 44)
                } else {
                    egui::Color32::from_rgb(240, 242, 245)
                };
                let panel_text = if dark_mode {
                    egui::Color32::from_rgb(220, 224, 234)
                } else {
                    egui::Color32::from_rgb(34, 34, 34)
                };

                let t_composer = Instant::now();
                let mut should_send_with_enter = false;
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    // Show error if present (bottom-most)
                    if !self.error.is_empty() {
                        ui.colored_label(egui::Color32::RED, &self.error);
                    }

                    if CHAT_STAGE6_ENABLE_MODEL_PICKER_WINDOW && self.show_model_picker {
                        egui::Window::new(i18n.t("chat.chooseModels"))
                            .id(egui::Id::new("chat_model_picker_window"))
                            .collapsible(false)
                            .resizable(true)
                            .default_width(360.0)
                            .show(ui.ctx(), |ui| {
                                ui.label(i18n.t("chat.multiModelHint"));
                                ui.separator();
                                let available_models = self.available_models.clone();
                                for model in &available_models {
                                    let mut checked =
                                        self.selected_models.iter().any(|m| m == model);
                                    if ui.checkbox(&mut checked, model).changed() {
                                        if checked {
                                            self.selected_models.push(model.clone());
                                        } else {
                                            self.selected_models.retain(|m| m != model);
                                        }
                                        self.sync_model_selection();
                                    }
                                }
                                ui.separator();
                                ui.horizontal(|ui| {
                                    if ui.button(i18n.t("chat.modelAutoOnly")).clicked() {
                                        self.selected_models = vec!["auto".to_string()];
                                        self.sync_model_selection();
                                    }
                                    if ui.button(i18n.t("chat.close")).clicked() {
                                        self.show_model_picker = false;
                                    }
                                });
                            });
                    }

                    // ── Button row ─────────────────────────────────────────
                    ui.horizontal(|ui| {
                        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS
                            && ui
                                .button("📎")
                                .on_hover_text(i18n.t("chat.attach"))
                                .clicked()
                        {
                            if let Some(files) = rfd::FileDialog::new().pick_files() {
                                for f in files {
                                    let n = f
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("file")
                                        .to_string();
                                    self.attachments.push(Attachment {
                                        name: n,
                                        mime: Self::guess_mime(&f),
                                        data: f.display().to_string(),
                                    });
                                }
                                self.error.clear();
                            }
                        }
                        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS
                            && ui
                                .button("📝")
                                .on_hover_text(i18n.t("chat.externalEditor"))
                                .clicked()
                        {
                            let p = std::env::temp_dir().join("go_on_chat_input.txt");
                            let _ = std::fs::write(&p, &self.input);
                            for e in &["zed", "code", "gedit", "vim", "nano"] {
                                if std::process::Command::new(e).arg(&p).spawn().is_ok() {
                                    break;
                                }
                            }
                        }
                        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS
                            && ui
                                .button("💡")
                                .on_hover_text(i18n.t("chat.promptTemplates"))
                                .clicked()
                        {
                            self.show_prompts = !self.show_prompts;
                        }
                        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS && self.show_prompts {
                            egui::Window::new(i18n.t("chat.promptTemplates"))
                                .id(egui::Id::new("quick_prompts_window"))
                                .collapsible(false)
                                .resizable(true)
                                .default_width(520.0)
                                .anchor(egui::Align2::LEFT_TOP, egui::vec2(0.0, 0.0))
                                .show(ui.ctx(), |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut self.template_search_query,
                                            )
                                            .hint_text(i18n.t("chat.searchTemplates"))
                                            .desired_width(220.0),
                                        );
                                        if ui.button(i18n.t("chat.templateNew")).clicked() {
                                            self.selected_template_idx = None;
                                            self.template_name_buf.clear();
                                            self.template_command_buf.clear();
                                            self.template_content_buf.clear();
                                        }
                                    });
                                    ui.separator();
                                    ui.columns(2, |columns| {
                                        columns[0].vertical(|ui| {
                                            let query =
                                                self.template_search_query.to_ascii_lowercase();
                                            let mut pick_idx = None;
                                            for (idx, template) in
                                                self.prompt_templates.iter().enumerate()
                                            {
                                                if !query.is_empty()
                                                    && !template
                                                        .name
                                                        .to_ascii_lowercase()
                                                        .contains(&query)
                                                    && !template
                                                        .command
                                                        .to_ascii_lowercase()
                                                        .contains(&query)
                                                {
                                                    continue;
                                                }
                                                let label = format!(
                                                    "{}  {}",
                                                    template.command, template.name
                                                );
                                                if ui
                                                    .selectable_label(
                                                        self.selected_template_idx == Some(idx),
                                                        label,
                                                    )
                                                    .clicked()
                                                {
                                                    pick_idx = Some(idx);
                                                }
                                            }
                                            if let Some(idx) = pick_idx {
                                                self.load_template_into_editor(idx);
                                            }
                                        });

                                        columns[1].vertical(|ui| {
                                            ui.label(i18n.t("chat.templateName"));
                                            ui.text_edit_singleline(&mut self.template_name_buf);
                                            ui.label(i18n.t("chat.templateCommand"));
                                            ui.text_edit_singleline(&mut self.template_command_buf);
                                            ui.label(i18n.t("chat.templateBody"));
                                            ui.add(
                                                egui::TextEdit::multiline(
                                                    &mut self.template_content_buf,
                                                )
                                                .desired_rows(10)
                                                .desired_width(ui.available_width()),
                                            );
                                            ui.label(i18n.t("chat.templatePlaceholderHint"));
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .button(i18n.t("chat.templateInsert"))
                                                    .clicked()
                                                {
                                                    self.input = self.template_content_buf.clone();
                                                    self.show_prompts = false;
                                                }
                                                if ui.button(i18n.t("chat.templateSave")).clicked()
                                                {
                                                    let name = self.template_name_buf.trim();
                                                    let command = Self::normalize_command(
                                                        &self.template_command_buf,
                                                    );
                                                    let content = self.template_content_buf.trim();
                                                    if name.is_empty()
                                                        || command.is_empty()
                                                        || content.is_empty()
                                                    {
                                                        self.error = i18n
                                                            .t("chat.templateValidation")
                                                            .to_string();
                                                    } else if self
                                                        .prompt_templates
                                                        .iter()
                                                        .enumerate()
                                                        .any(|(idx, t)| {
                                                            t.command == command
                                                                && Some(idx)
                                                                    != self.selected_template_idx
                                                        })
                                                    {
                                                        self.error = i18n
                                                            .t("chat.templateDuplicate")
                                                            .to_string();
                                                    } else {
                                                        let template = PromptTemplate {
                                                            id: self
                                                                .selected_template_idx
                                                                .and_then(|idx| {
                                                                    self.prompt_templates
                                                                        .get(idx)
                                                                        .map(|t| t.id.clone())
                                                                })
                                                                .unwrap_or_else(|| {
                                                                    format!(
                                                                        "tpl_{}",
                                                                        self.prompt_templates.len()
                                                                            + 1
                                                                    )
                                                                }),
                                                            name: name.to_string(),
                                                            command,
                                                            content: content.to_string(),
                                                        };
                                                        if let Some(idx) =
                                                            self.selected_template_idx
                                                        {
                                                            self.prompt_templates[idx] = template;
                                                        } else {
                                                            self.prompt_templates.push(template);
                                                            self.selected_template_idx = Some(
                                                                self.prompt_templates.len() - 1,
                                                            );
                                                        }
                                                        self.save_templates_to_disk();
                                                        self.error.clear();
                                                    }
                                                }
                                                if ui
                                                    .button(i18n.t("chat.templateDelete"))
                                                    .clicked()
                                                {
                                                    if let Some(idx) =
                                                        self.selected_template_idx.take()
                                                    {
                                                        if idx < self.prompt_templates.len() {
                                                            self.prompt_templates.remove(idx);
                                                            self.save_templates_to_disk();
                                                            self.template_name_buf.clear();
                                                            self.template_command_buf.clear();
                                                            self.template_content_buf.clear();
                                                        }
                                                    }
                                                }
                                            });
                                        });
                                    });
                                    if ui.button(i18n.t("chat.close")).clicked() {
                                        self.show_prompts = false;
                                    }
                                });
                        }
                        ui.add_space(8.0);
                        let send_hint_key = if cfg!(target_os = "linux") {
                            "chat.sendShortcutHintLinux"
                        } else {
                            "chat.sendShortcutHint"
                        };
                        ui.label(egui::RichText::new(i18n.t(send_hint_key)).small().weak());

                        if self.sending && self.ai_status == AiStatus::Thinking {
                            let stop_btn = egui::Button::new(format!("⏹ {}", i18n.t("chat.stop")))
                                .fill(egui::Color32::RED)
                                .min_size(egui::vec2(80.0, 28.0));
                            if ui.add(stop_btn).clicked() {
                                self.stop_sending();
                            }
                        } else {
                            let (icon, col) = match self.ai_status {
                                AiStatus::Idle => (
                                    i18n.t("chat.send").to_string(),
                                    egui::Color32::from_rgb(40, 120, 220),
                                ),
                                AiStatus::Thinking => {
                                    ("...".to_string(), egui::Color32::from_rgb(200, 160, 60))
                                }
                                AiStatus::Error => {
                                    (i18n.t("chat.retry").to_string(), egui::Color32::RED)
                                }
                            };
                            let snd = egui::Button::new(format!("▶ {}", icon))
                                .fill(col)
                                .min_size(egui::vec2(80.0, 28.0));
                            if ui.add_enabled(!self.sending, snd).clicked() {
                                self.send_message(backend, ctx, autotune_chain_enabled);
                            }
                        }
                    });

                    // ── Input box ────────────────────────────────────────────
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut self.input)
                            .hint_text(i18n.t("chat.input"))
                            .desired_rows(3)
                            .desired_width(ui.available_width()),
                    );
                    should_send_with_enter = ui.input(|i| {
                        if !resp.has_focus()
                            || !i.key_pressed(egui::Key::Enter)
                            || i.modifiers.shift
                        {
                            return false;
                        }
                        #[cfg(target_os = "linux")]
                        {
                            i.modifiers.ctrl || i.modifiers.command
                        }
                        #[cfg(not(target_os = "linux"))]
                        {
                            true
                        }
                    });

                    // ── Input attachments ───────────────────────────────────
                    if !self.attachments.is_empty() {
                        ui.horizontal(|ui| {
                            for att in &self.attachments {
                                let icon = if att.mime.starts_with("image/") {
                                    "🖼️"
                                } else {
                                    "📎"
                                };
                                ui.label(format!("{} {}", icon, att.name));
                            }
                            if ui.button("✕").clicked() {
                                self.attachments.clear();
                            }
                        });
                    }

                    ui.add_space(4.0);

                    // ── Mode selector row ──────────────────────────────────
                    if CHAT_STAGE6_ENABLE_MODE_ROW {
                        egui::Frame::new()
                            .fill(panel_bg)
                            .corner_radius(6.0)
                            .inner_margin(egui::Margin::symmetric(10i8, 6i8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(i18n.t("chat.mode"))
                                            .color(panel_text)
                                            .strong(),
                                    );
                                    ui.add_space(6.0);
                                    egui::ComboBox::from_id_salt("mode_sel")
                                        .selected_text(
                                            i18n.t(&format!("mode.{}", self.selected_mode)),
                                        )
                                        .show_ui(ui, |ui| {
                                            let modes =
                                                ["ask", "plan", "edit", "safeguard", "full_auto"];
                                            for val in &modes {
                                                ui.selectable_value(
                                                    &mut self.selected_mode,
                                                    val.to_string(),
                                                    i18n.t(&format!("mode.{val}")),
                                                );
                                            }
                                        });

                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(i18n.t("chat.model")).color(panel_text),
                                    );
                                    if ui
                                        .button(i18n.t("chat.chooseModels"))
                                        .on_hover_text(i18n.t("chat.multiModelHint"))
                                        .clicked()
                                    {
                                        self.show_model_picker = true;
                                    }
                                    ui.add_space(12.0);
                                    ui.label(
                                        egui::RichText::new(self.selected_models_summary(i18n))
                                            .color(panel_text)
                                            .size(12.0),
                                    );
                                });
                            });
                    }
                    if CHAT_STAGE6_ENABLE_METADATA_SYNC {
                        let mut metadata_changed = false;
                        if self.active_session < self.sessions.len() {
                            let session = &mut self.sessions[self.active_session];
                            if session.mode != self.selected_mode {
                                session.mode = self.selected_mode.clone();
                                metadata_changed = true;
                            }
                            if session.phase != self.selected_phase {
                                session.phase = self.selected_phase.clone();
                                metadata_changed = true;
                            }
                            if session.model != self.selected_model {
                                session.model = self.selected_model.clone();
                                metadata_changed = true;
                            }
                            if session.models != self.selected_models {
                                session.models = self.selected_models.clone();
                                metadata_changed = true;
                            }
                        }
                        if metadata_changed {
                            self.save_sessions_to_disk();
                        }
                    }

                    ui.separator();

                    // ── Top: search + conversation messages ───────────────
                    if CHAT_STAGE6_ENABLE_SEARCH_ROW {
                        ui.horizontal(|ui| {
                            ui.label(i18n.t("chat.search"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.message_search_query)
                                    .hint_text(i18n.t("chat.searchMessages"))
                                    .desired_width(ui.available_width()),
                            );
                        });
                        ui.add_space(4.0);
                    }

                    let t_messages = Instant::now();
                    egui::ScrollArea::vertical()
                        .max_height(ui.available_height().max(120.0))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.show_messages(ui, i18n);
                        });
                    messages_ms = t_messages.elapsed().as_millis();
                });

                if should_send_with_enter {
                    self.send_message(backend, ctx, autotune_chain_enabled);
                }

                composer_ms = t_composer.elapsed().as_millis();
            });
        });

        if debug_enabled {
            let total_ms = _start.elapsed().as_millis();
            Self::push_perf_sample(&mut self.perf_total_samples, total_ms);
            Self::push_perf_sample(&mut self.perf_sidebar_samples, sidebar_ms);
            Self::push_perf_sample(&mut self.perf_messages_samples, messages_ms);
            Self::push_perf_sample(&mut self.perf_composer_samples, composer_ms);
            self.perf_frame_counter = self.perf_frame_counter.saturating_add(1);

            if total_ms > 40 || messages_ms > 30 || composer_ms > 25 || sidebar_ms > 20 {
                Self::chat_debug_log(&format!(
                    "[CHAT_PERF] total={}ms sidebar={}ms messages={}ms composer={}ms sessions={} msgs={}",
                    total_ms,
                    sidebar_ms,
                    messages_ms,
                    composer_ms,
                    self.sessions.len(),
                    self.messages().len()
                ));
            }

            if self
                .perf_frame_counter
                .is_multiple_of(CHAT_PERF_SUMMARY_INTERVAL)
            {
                Self::chat_debug_log(&format!(
                    "[CHAT_PERF_SUMMARY] window={} avg(total/sidebar/messages/composer)={}/{}/{}/{}ms p95(total/messages)={}/{}ms max(total/messages)={}/{}ms",
                    self.perf_total_samples.len(),
                    Self::perf_avg(&self.perf_total_samples),
                    Self::perf_avg(&self.perf_sidebar_samples),
                    Self::perf_avg(&self.perf_messages_samples),
                    Self::perf_avg(&self.perf_composer_samples),
                    Self::perf_p95(&self.perf_total_samples),
                    Self::perf_p95(&self.perf_messages_samples),
                    Self::perf_max(&self.perf_total_samples),
                    Self::perf_max(&self.perf_messages_samples),
                ));
            }
        }
    }

    fn show_safe_chat_layout(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
        autotune_chain_enabled: bool,
    ) -> (u128, u128, u128) {
        let mut sidebar_ms: u128 = 0;
        let mut messages_ms: u128 = 0;
        let mut composer_ms: u128 = 0;

        let enable_input_widget = CHAT_ISOLATION_STAGE >= 2;
        let enable_enter_send = CHAT_ISOLATION_STAGE >= 3;
        let enable_show_messages = CHAT_ISOLATION_STAGE >= 4;
        let enable_sidebar = CHAT_ISOLATION_STAGE >= 5;

        let total_w = ui.available_width();
        let total_h = Self::bounded_panel_height(ui, 320.0);
        // Sidebar: fixed 1/3 of total width, clamped to [180, 360]
        let separator_w = 9.0;
        let sidebar_w = (total_w / 3.0).clamp(180.0, 360.0);
        // Right content: remaining width
        let content_w = (total_w - sidebar_w - separator_w).max(120.0);

        ui.horizontal_top(|ui| {
            ui.allocate_ui(egui::vec2(sidebar_w, total_h), |ui| {
                if enable_sidebar {
                    let t_sidebar = Instant::now();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.show_sidebar(ui, i18n);
                        });
                    sidebar_ms = t_sidebar.elapsed().as_millis();
                } else {
                    ui.label("Sidebar disabled");
                }
            });

            ui.separator();

            // Right panel: messages on top, controls on bottom.
            // Use ui.allocate_ui_with_layout to vertically split:
            //   top = messages area (fills remaining)
            //   bottom = composer (fixed height)
            ui.allocate_ui(egui::vec2(content_w, total_h), |ui| {
                let dark_mode = ui.visuals().dark_mode;
                let panel_bg = if dark_mode {
                    egui::Color32::from_rgb(36, 38, 44)
                } else {
                    egui::Color32::from_rgb(240, 242, 245)
                };
                let panel_text = if dark_mode {
                    egui::Color32::from_rgb(220, 224, 234)
                } else {
                    egui::Color32::from_rgb(34, 34, 34)
                };

                egui::Frame::new()
                    .fill(panel_bg)
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(120, 120, 120, 72),
                    ))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(10i8, 10i8))
                    .show(ui, |ui| {
                        // ── Mode row (fixed height at top) ──────────────
                        if CHAT_STAGE6_ENABLE_MODE_ROW {
                            egui::Frame::new()
                                .fill(panel_bg)
                                .corner_radius(6.0)
                                .inner_margin(egui::Margin::symmetric(10i8, 6i8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(i18n.t("chat.mode"))
                                                .color(panel_text)
                                                .strong(),
                                        );
                                        ui.add_space(6.0);
                                        egui::ComboBox::from_id_salt("mode_sel_safe")
                                            .selected_text(
                                                i18n.t(&format!("mode.{}", self.selected_mode)),
                                            )
                                            .show_ui(ui, |ui| {
                                                let modes = [
                                                    "ask",
                                                    "plan",
                                                    "edit",
                                                    "safeguard",
                                                    "full_auto",
                                                ];
                                                for val in &modes {
                                                    ui.selectable_value(
                                                        &mut self.selected_mode,
                                                        val.to_string(),
                                                        i18n.t(&format!("mode.{val}")),
                                                    );
                                                }
                                            });

                                        ui.add_space(8.0);
                                        ui.label(
                                            egui::RichText::new(i18n.t("chat.model"))
                                                .color(panel_text),
                                        );
                                        if ui
                                            .button(i18n.t("chat.chooseModels"))
                                            .on_hover_text(i18n.t("chat.multiModelHint"))
                                            .clicked()
                                        {
                                            self.show_model_picker = true;
                                        }
                                        ui.add_space(12.0);
                                        ui.label(
                                            egui::RichText::new(self.selected_models_summary(i18n))
                                                .color(panel_text)
                                                .size(12.0),
                                        );
                                    });
                                });

                            if CHAT_STAGE6_ENABLE_METADATA_SYNC {
                                let mut metadata_changed = false;
                                if self.active_session < self.sessions.len() {
                                    let session = &mut self.sessions[self.active_session];
                                    if session.mode != self.selected_mode {
                                        session.mode = self.selected_mode.clone();
                                        metadata_changed = true;
                                    }
                                    if session.phase != self.selected_phase {
                                        session.phase = self.selected_phase.clone();
                                        metadata_changed = true;
                                    }
                                    if session.model != self.selected_model {
                                        session.model = self.selected_model.clone();
                                        metadata_changed = true;
                                    }
                                    if session.models != self.selected_models {
                                        session.models = self.selected_models.clone();
                                        metadata_changed = true;
                                    }
                                }
                                if metadata_changed {
                                    self.save_sessions_to_disk();
                                }
                            }

                            ui.add_space(4.0);
                        }

                        // ── Split remaining area: messages (80%) + composer (20%, i.e. 1/4 of messages) ──
                        let avail = ui.available_height();
                        let gap = 8.0;
                        let messages_height = ((avail - gap) * 0.8).max(80.0);
                        let composer_height = ((avail - gap) * 0.2).clamp(100.0, 260.0);

                        // Top: Messages area
                        let t_messages = Instant::now();
                        ui.allocate_ui(egui::vec2(ui.available_width(), messages_height), |ui| {
                            egui::Frame::new()
                                .fill(panel_bg)
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    egui::Color32::from_rgba_unmultiplied(120, 120, 120, 72),
                                ))
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::symmetric(10i8, 10i8))
                                .show(ui, |ui| {
                                    if CHAT_STAGE6_ENABLE_SEARCH_ROW {
                                        ui.horizontal(|ui| {
                                            ui.label(i18n.t("chat.search"));
                                            ui.add(
                                                egui::TextEdit::singleline(
                                                    &mut self.message_search_query,
                                                )
                                                .hint_text(i18n.t("chat.searchMessages"))
                                                .desired_width(ui.available_width()),
                                            );
                                        });
                                        ui.add_space(4.0);
                                    }

                                    egui::ScrollArea::vertical()
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            if enable_show_messages {
                                                self.show_messages(ui, i18n);
                                            } else {
                                                const MAX_SHOWN: usize = 20;
                                                let msgs = self.messages();
                                                let start = msgs.len().saturating_sub(MAX_SHOWN);
                                                for msg in msgs.iter().skip(start) {
                                                    let role =
                                                        if msg.role == "user" { "U" } else { "A" };
                                                    let mut text = if CHAT_DISABLE_MARKDOWN_RENDER {
                                                        Self::markdown_to_plain_text(&msg.content)
                                                    } else {
                                                        msg.content.clone()
                                                    };
                                                    if text.chars().count() > 240 {
                                                        text = text
                                                            .chars()
                                                            .take(240)
                                                            .collect::<String>()
                                                            + "...";
                                                    }
                                                    ui.label(format!("[{}] {}", role, text));
                                                }
                                            }
                                        });
                                });
                        });
                        messages_ms = t_messages.elapsed().as_millis();

                        ui.add_space(gap);

                        // Bottom: Composer
                        let t_composer = Instant::now();
                        ui.allocate_ui(egui::vec2(ui.available_width(), composer_height), |ui| {
                            egui::Frame::new()
                                .fill(panel_bg)
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    egui::Color32::from_rgba_unmultiplied(120, 120, 120, 72),
                                ))
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                                .show(ui, |ui| {
                                    let mut input_has_focus = false;

                                    ui.horizontal(|ui| {
                                        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS {
                                            if ui
                                                .button("📎")
                                                .on_hover_text(i18n.t("chat.attach"))
                                                .clicked()
                                            {
                                                if let Some(files) =
                                                    rfd::FileDialog::new().pick_files()
                                                {
                                                    for f in files {
                                                        let n = f
                                                            .file_name()
                                                            .and_then(|s| s.to_str())
                                                            .unwrap_or("file")
                                                            .to_string();
                                                        self.attachments.push(Attachment {
                                                            name: n,
                                                            mime: Self::guess_mime(&f),
                                                            data: f.display().to_string(),
                                                        });
                                                    }
                                                    self.error.clear();
                                                }
                                            }
                                            if ui
                                                .button("📝")
                                                .on_hover_text(i18n.t("chat.externalEditor"))
                                                .clicked()
                                            {
                                                let p = std::env::temp_dir()
                                                    .join("go_on_chat_input.txt");
                                                let _ = std::fs::write(&p, &self.input);
                                                for e in &["zed", "code", "gedit", "vim", "nano"] {
                                                    if std::process::Command::new(e)
                                                        .arg(&p)
                                                        .spawn()
                                                        .is_ok()
                                                    {
                                                        break;
                                                    }
                                                }
                                            }
                                            if ui
                                                .button("💡")
                                                .on_hover_text(i18n.t("chat.promptTemplates"))
                                                .clicked()
                                            {
                                                self.show_prompts = !self.show_prompts;
                                            }
                                        }

                                        ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if self.sending && self.ai_status == AiStatus::Thinking
                                            {
                                                if ui
                                                    .add(
                                                        egui::Button::new(format!(
                                                            "⏹ {}",
                                                            i18n.t("chat.stop")
                                                        ))
                                                        .fill(egui::Color32::RED),
                                                    )
                                                    .clicked()
                                                {
                                                    self.stop_sending();
                                                }
                                            } else if ui
                                                .add_enabled(
                                                    !self.sending,
                                                    egui::Button::new(format!(
                                                        "▶ {}",
                                                        i18n.t("chat.send")
                                                    ))
                                                    .fill(egui::Color32::from_rgb(40, 120, 220)),
                                                )
                                                .clicked()
                                            {
                                                self.send_message(
                                                    backend,
                                                    ctx,
                                                    autotune_chain_enabled,
                                                );
                                            }
                                            ui.label(
                                                egui::RichText::new(
                                                    i18n.t("chat.sendShortcutHint"),
                                                )
                                                .small()
                                                .weak(),
                                            );
                                        },
                                    );
                                    });

                                    if !self.attachments.is_empty() {
                                        ui.horizontal_wrapped(|ui| {
                                            for att in &self.attachments {
                                                let icon = if att.mime.starts_with("image/") {
                                                    "🖼️"
                                                } else {
                                                    "📎"
                                                };
                                                ui.label(format!("{} {}", icon, att.name));
                                            }
                                            if ui.button("✕").clicked() {
                                                self.attachments.clear();
                                            }
                                        });
                                    }

                                    if enable_input_widget {
                                        let input_resp = ui.add_sized(
                                            [ui.available_width(), 74.0],
                                            egui::TextEdit::multiline(&mut self.input)
                                                .hint_text(i18n.t("chat.input")),
                                        );
                                        input_has_focus = input_resp.has_focus();
                                    }

                                    if !self.error.is_empty() {
                                        ui.colored_label(egui::Color32::RED, &self.error);
                                    }

                                    if enable_input_widget
                                        && enable_enter_send
                                        && ui.input(|i| {
                                            input_has_focus
                                                && i.key_pressed(egui::Key::Enter)
                                                && !i.modifiers.shift
                                        })
                                    {
                                        self.send_message(backend, ctx, autotune_chain_enabled);
                                    }
                                });
                        });
                        composer_ms = t_composer.elapsed().as_millis();
                    });
            }); // end allocate_ui (right panel)
        }); // end ui.horizontal

        (sidebar_ms, messages_ms, composer_ms)
    }

    // ── Sidebar: session list ───────────────────────────────────
    fn show_sidebar(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(i18n.t("chat.title"));
                // Feature 8: export button
                if ui
                    .button("📤")
                    .on_hover_text(i18n.t("chat.export"))
                    .clicked()
                {
                    let msgs = self.messages();
                    let mut md = String::new();
                    md.push_str(&format!("# {}\n\n", i18n.t("chat.exportTitle")));
                    let exported_at = format_absolute_time(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                    md.push_str(&format!(
                        "_{}_\n\n",
                        i18n.t("chat.exportedAt").replace("{time}", &exported_at)
                    ));
                    for msg in msgs {
                        let role_label = if msg.role == "user" {
                            format!("**{}**", i18n.t("chat.exportRoleYou"))
                        } else {
                            format!("**{}**", i18n.t("chat.exportRoleAssistant"))
                        };
                        md.push_str(&format!(
                            "{} ({})\n\n",
                            role_label,
                            format_absolute_time(msg.timestamp)
                        ));
                        if !msg.model.is_empty() {
                            md.push_str(&format!(
                                "_{}_\n\n",
                                i18n.t("chat.exportModel").replace("{model}", &msg.model)
                            ));
                        }
                        md.push_str(&format!("{}\n\n", msg.content));
                        if !msg.thinking.is_empty() {
                            md.push_str(&format!(
                                "> {}\n\n",
                                i18n.t("chat.exportThinking")
                                    .replace("{thinking}", &msg.thinking)
                            ));
                        }
                    }
                    let default_name = self
                        .sessions
                        .get(self.active_session)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "chat-export".to_string())
                        .replace('/', "-");
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("{default_name}.md"))
                        .save_file()
                    {
                        match std::fs::write(&path, md) {
                            Ok(()) => {
                                self.error = i18n
                                    .t("chat.exportSuccess")
                                    .replace("{path}", &path.display().to_string());
                            }
                            Err(e) => {
                                self.error = i18n
                                    .t("chat.exportFailed")
                                    .replace("{error}", &e.to_string());
                            }
                        }
                    }
                }
                if ui
                    .button("＋")
                    .on_hover_text(i18n.t("chat.newSession"))
                    .clicked()
                {
                    self.new_session();
                    self.refresh_default_session_names(i18n);
                }
            });
            // Feature 9: search field
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.session_search_query)
                    .hint_text(i18n.t("chat.searchSessions"))
                    .desired_width(ui.available_width()),
            );
            ui.separator();
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .max_height(ui.available_height().max(100.0))
                .show(ui, |ui| {
                    let mut to_remove: Option<usize> = None;
                    // Feature 9: filter by search query
                    let filtered_sessions: Vec<(usize, String, String, String, Vec<String>)> =
                        if self.session_search_query.is_empty() {
                            self.sessions
                                .iter()
                                .enumerate()
                                .map(|(idx, s)| {
                                    (
                                        idx,
                                        s.name.clone(),
                                        s.mode.clone(),
                                        s.phase.clone(),
                                        s.models.clone(),
                                    )
                                })
                                .collect()
                        } else {
                            let q = self.session_search_query.to_lowercase();
                            self.sessions
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| s.name.to_lowercase().contains(&q))
                                .map(|(idx, s)| {
                                    (
                                        idx,
                                        s.name.clone(),
                                        s.mode.clone(),
                                        s.phase.clone(),
                                        s.models.clone(),
                                    )
                                })
                                .collect()
                        };
                    for (idx, session_name, session_mode, session_phase, session_models) in
                        filtered_sessions
                    {
                        let selected = idx == self.active_session;
                        let dark_mode = ui.visuals().dark_mode;
                        let bg = if selected {
                            if dark_mode {
                                egui::Color32::from_rgb(52, 96, 170)
                            } else {
                                egui::Color32::from_rgb(40, 100, 200)
                            }
                        } else {
                            if dark_mode {
                                egui::Color32::from_rgb(40, 42, 48)
                            } else {
                                egui::Color32::from_rgb(86, 90, 98)
                            }
                        };

                        egui::Frame::NONE
                            .fill(bg)
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(egui::Margin::same(6i8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.set_min_width(160.0);
                                    // Feature 9: highlight matching text
                                    if self.session_search_query.is_empty() {
                                        if ui.selectable_label(selected, &session_name).clicked() {
                                            self.active_session = idx;
                                            self.selected_mode = session_mode.clone();
                                            self.selected_phase = session_phase.clone();
                                            self.selected_models = if session_models.is_empty() {
                                                vec!["auto".to_string()]
                                            } else {
                                                session_models.clone()
                                            };
                                            self.sync_model_selection();
                                            self.ai_status = AiStatus::Idle;
                                            self.edit_msg_idx = None;
                                            self.edit_msg_buf.clear();
                                        }
                                    } else {
                                        // Highlight matched text
                                        let label = ui.selectable_label(selected, "").clicked();
                                        let _resp = ui.label(
                                            egui::RichText::new(&session_name)
                                                .color(egui::Color32::WHITE),
                                        );
                                        // Reuse the click from the selectable_label
                                        if label {
                                            self.active_session = idx;
                                            self.selected_mode = session_mode.clone();
                                            self.selected_phase = session_phase.clone();
                                            self.selected_models = if session_models.is_empty() {
                                                vec!["auto".to_string()]
                                            } else {
                                                session_models.clone()
                                            };
                                            self.sync_model_selection();
                                            self.ai_status = AiStatus::Idle;
                                            self.edit_msg_idx = None;
                                            self.edit_msg_buf.clear();
                                        }
                                        // Highlight using painter - simpler approach
                                        let q = self.session_search_query.to_lowercase();
                                        if let Some(_start) = session_name.to_lowercase().find(&q) {
                                            let painter = ui.painter();
                                            // Highlight the entire label area as a colored rect
                                            let min_rect = ui.min_rect();
                                            painter.rect_filled(
                                                min_rect,
                                                2.0,
                                                egui::Color32::from_rgba_premultiplied(
                                                    255, 255, 0, 60,
                                                ),
                                            );
                                        }
                                    }
                                    // Right-click context or delete button
                                    if ui.button("✕").on_hover_text(i18n.t("chat.clear")).clicked()
                                    {
                                        to_remove = Some(idx);
                                    }
                                });
                                // Show mode/phase indicator
                                ui.label(format!(
                                    "{} | {}",
                                    i18n.t(&format!("mode.{}", session_mode)),
                                    i18n.t(&format!("phase.{}", session_phase)),
                                ))
                                .highlight();
                            });
                        ui.add_space(2.0);
                    }

                    if let Some(idx) = to_remove {
                        if self.sessions.len() > 1 {
                            self.sessions.remove(idx);
                            if idx < self.active_session {
                                self.active_session -= 1;
                            } else if self.active_session >= self.sessions.len() {
                                self.active_session = self.sessions.len() - 1;
                            }
                            if self.active_session < self.sessions.len() {
                                self.selected_mode =
                                    self.sessions[self.active_session].mode.clone();
                                self.selected_phase =
                                    self.sessions[self.active_session].phase.clone();
                                self.selected_model =
                                    self.sessions[self.active_session].model.clone();
                                self.selected_models =
                                    if self.sessions[self.active_session].models.is_empty() {
                                        vec![self.selected_model.clone()]
                                    } else {
                                        self.sessions[self.active_session].models.clone()
                                    };
                                self.sync_model_selection();
                            }
                            self.save_sessions_to_disk();
                        }
                    }
                });
        });
    }

    // ── Messages area (Cherry Studio style) ─────────────────────
    fn show_messages(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        let render_start = std::time::Instant::now();

        // Immediate bootstrap on function entry
        if !self.debug_log_bootstrapped {
            self.debug_log_bootstrapped = true;
            Self::chat_debug_log("[CHAT_DEBUG_ENTER] show_messages() called");
        }

        let msgs = self.messages().to_vec();
        let total_msgs = msgs.len();

        if total_msgs == 0 {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(i18n.t("chat.noMessages"))
                        .color(egui::Color32::from_rgb(140, 142, 150))
                        .size(16.0),
                );
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(180, 182, 188), i18n.t("chat.hint"));
            });
            return;
        }

        // Calculate pagination
        let pages = total_msgs.div_ceil(self.const_messages_per_page);
        if self.messages_page >= pages {
            self.messages_page = if pages > 0 { pages - 1 } else { 0 };
        }

        let start_idx = self.messages_page * self.const_messages_per_page;
        let end_idx = (start_idx + self.const_messages_per_page).min(total_msgs);
        let msgs_to_show = &msgs[start_idx..end_idx];

        // Pagination controls
        ui.horizontal(|ui| {
            if ui.button("◀ Prev").clicked() && self.messages_page > 0 {
                self.messages_page -= 1;
            }
            ui.label(format!(
                "Page {} / {} ({}-{})",
                self.messages_page + 1,
                pages,
                start_idx + 1,
                end_idx
            ));
            if ui.button("Next ▶").clicked() && self.messages_page + 1 < pages {
                self.messages_page += 1;
            }
        });
        ui.separator();

        // Render only current page messages
        let dark_mode = ui.visuals().dark_mode;
        for msg in msgs_to_show.iter() {
            let is_user = msg.role == "user";
            let bubble_color = if is_user {
                if dark_mode {
                    egui::Color32::from_rgb(32, 112, 210)
                } else {
                    egui::Color32::from_rgb(10, 106, 255)
                }
            } else {
                if dark_mode {
                    egui::Color32::from_rgb(42, 44, 50)
                } else {
                    egui::Color32::from_rgb(240, 241, 245)
                }
            };
            let text_color = if is_user {
                egui::Color32::WHITE
            } else {
                if dark_mode {
                    egui::Color32::from_rgb(232, 236, 244)
                } else {
                    egui::Color32::from_rgb(28, 28, 32)
                }
            };

            if is_user {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    ui.label(egui::RichText::new("👤").size(20.0));
                    const MAX_DISPLAY: usize = 500;
                    let display_text = if CHAT_DISABLE_MARKDOWN_RENDER {
                        Self::markdown_to_plain_text(&msg.content)
                    } else {
                        msg.content.clone()
                    };
                    let preview = if display_text.len() > MAX_DISPLAY {
                        let safe_str: String = display_text.chars().take(MAX_DISPLAY).collect();
                        format!("{}...", safe_str)
                    } else {
                        display_text
                    };
                    egui::Frame::new()
                        .fill(bubble_color)
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(preview).color(text_color));
                        });
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("🤖").size(20.0));
                    let bubble_w = (ui.available_width() - 10.0).max(0.0);
                    let bubble_h = ui.available_height().max(0.0);
                    ui.allocate_ui(egui::vec2(bubble_w, bubble_h), |ui| {
                        egui::Frame::new()
                            .fill(bubble_color)
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                            .show(ui, |ui| {
                                const MAX_DISPLAY: usize = 500;
                                let display_text = if CHAT_DISABLE_MARKDOWN_RENDER {
                                    Self::markdown_to_plain_text(&msg.content)
                                } else {
                                    msg.content.clone()
                                };
                                let preview = if display_text.len() > MAX_DISPLAY {
                                    let safe_str: String =
                                        display_text.chars().take(MAX_DISPLAY).collect();
                                    format!("{}...", safe_str)
                                } else {
                                    display_text
                                };
                                ui.label(egui::RichText::new(preview).color(text_color));
                            });
                    });
                });
            }
            ui.add_space(4.0);
        }

        // Feature 5: show aggregate token estimate below last AI message
        if self.last_token_estimate > 0 {
            let msgs = self.messages();
            if let Some(last) = msgs.last() {
                if last.role == "assistant" {
                    ui.horizontal(|ui| {
                        ui.add_space(36.0);
                        ui.colored_label(
                            egui::Color32::from_rgb(140, 142, 150),
                            format!(
                                "⚡ {}",
                                i18n.t("chat.tokenSummary")
                                    .replace("{input}", &self.input_token_estimate.to_string())
                                    .replace("{output}", &self.output_token_estimate.to_string())
                                    .replace("{total}", &self.last_token_estimate.to_string())
                            ),
                        );
                    });
                }
            }
        }

        // Log performance
        let total_ms = render_start.elapsed().as_millis();
        Self::chat_debug_log(&format!(
            "[CHAT_PERF_PAGINATED] total={}ms page={}/{} messages_shown={}",
            total_ms,
            self.messages_page + 1,
            pages,
            msgs_to_show.len()
        ));
    }
}
