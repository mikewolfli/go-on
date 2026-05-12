use super::*;

impl ChatView {
    const MAX_RENDERED_MESSAGES: usize = 250;

    /// Draw a colored circle avatar with the role initial letter.
    /// User gets a blue circle with "U", AI gets a green circle with "A".
    fn draw_role_avatar(ui: &mut egui::Ui, is_user: bool) {
        let size = 28.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let painter = ui.painter();
        let dark = ui.visuals().dark_mode;
        let color = if is_user {
            if dark {
                egui::Color32::from_rgb(32, 112, 210)
            } else {
                egui::Color32::from_rgb(10, 106, 255)
            }
        } else {
            if dark {
                egui::Color32::from_rgb(20, 90, 60)
            } else {
                egui::Color32::from_rgb(20, 120, 70)
            }
        };
        painter.circle_filled(rect.center(), size / 2.0, color);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if is_user { "U" } else { "A" },
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );
    }

    fn render_token_stats(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        if !self.show_token_details || self.model_stats.is_empty() {
            return;
        }

        let Some(stats) = self.model_stats.get(&self.selected_model) else {
            return;
        };

        let success_count = stats.success_count as f64;
        let total_count = success_count + stats.error_count as f64;
        let success_rate = if total_count > 0.0 {
            (success_count / total_count * 100.0).round() as u32
        } else {
            0
        };

        let time_color = if stats.response_time_ms < 2_000 {
            egui::Color32::from_rgb(76, 175, 80)
        } else if stats.response_time_ms < 5_000 {
            egui::Color32::from_rgb(255, 193, 7)
        } else {
            egui::Color32::from_rgb(244, 67, 54)
        };

        egui::Frame::new()
            .fill(ui.visuals().window_fill().gamma_multiply(0.8))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8i8, 4i8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(i18n.t("chat.tokenStats"))
                            .strong()
                            .size(11.0),
                    );
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: {} ms",
                            i18n.t("chat.responseTime"),
                            stats.response_time_ms
                        ))
                        .color(time_color)
                        .size(11.0),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: {}",
                            i18n.t("chat.tokens"),
                            stats.token_count
                        ))
                        .size(11.0),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: {}%",
                            i18n.t("chat.successRate"),
                            success_rate
                        ))
                        .size(11.0),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: {:.0}",
                            i18n.t("chat.tokensPerMinute"),
                            stats.avg_tokens_per_minute
                        ))
                        .size(11.0)
                        .weak(),
                    );
                });
            });
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
        autotune_chain_enabled: bool,
        runtime_config: ChatUiRuntimeConfig,
    ) {
        self.apply_stability_settings(
            runtime_config.repaint_interval_ms,
            runtime_config.stream_chunk_flush_ms,
            runtime_config.max_pending_events_per_frame,
        );

        // Process any pending async responses (non-blocking)
        self.process_pending(i18n);

        // Keep streaming repaint cadence stable (~30 FPS) to avoid high-frequency jitter.
        if self.sending {
            ctx.request_repaint_after(self.stream_repaint_interval);
        }

        // Lazy initialization of templates and name refresh
        let is_first_init = !self.templates_bootstrapped;
        if is_first_init {
            self.bootstrap_default_templates(i18n);
            self.refresh_default_session_names(i18n);
        }
        self.sync_model_selection();

        // Delayed loading: Schedule backend queries after first render to avoid UI freeze
        if !self.phases_load_scheduled && !self.phases_loaded {
            self.phases_load_scheduled = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();

            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

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
                            ctx_clone.request_repaint_after(std::time::Duration::from_millis(16));
                        }
                    }
                    _ => {
                        #[cfg(debug_assertions)]
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
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;

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
                        ctx_clone.request_repaint_after(std::time::Duration::from_millis(16));
                    }
                    Err(_) => {
                        eprintln!("Warning: Failed to load models from backend (timeout)");
                    }
                }
            });

            self.models_loaded = true;
        }

        // Delegate to the stable layout (uses SidePanel/CentralPanel, no bottom_up).
        // The old horizontal_top/bottom_up layout caused UI freeze on tab switch.
        self.show_safe_chat_layout(ui, i18n, backend, ctx, autotune_chain_enabled);
    }

    fn show_safe_chat_layout(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
        autotune_chain_enabled: bool,
    ) {
        // Use a single vertical layout with SidePanel for the sidebar.
        // CentralPanel is NOT used because it fights with SidePanel for space
        // and can swallow bottom widgets. Instead we manually lay out:
        //   [SidePanel left] + [vertical: mode_row | scroll(messages) | input_area]

        // ── Model picker window (floating) ───────────────────
        if self.show_model_picker {
            egui::Window::new(i18n.t("chat.chooseModels"))
                .id(egui::Id::new("chat_model_picker_window"))
                .collapsible(false)
                .resizable(true)
                .default_width(360.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ui.ctx(), |ui| {
                    ui.label(i18n.t("chat.multiModelHint"));
                    ui.separator();
                    let available = self.available_models.clone();
                    for model in &available {
                        let mut checked = self.selected_models.iter().any(|m| m == model);
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

        // ── Prompt templates window (floating) ───────────────
        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS && self.show_prompts {
            egui::Window::new(i18n.t("chat.promptTemplates"))
                .id(egui::Id::new("quick_prompts_window"))
                .collapsible(false)
                .resizable(true)
                .default_width(520.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ui.ctx(), |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.template_search_query)
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
                            let query = self.template_search_query.to_ascii_lowercase();
                            let mut pick_idx = None;
                            for (idx, tpl) in self.prompt_templates.iter().enumerate() {
                                if !query.is_empty()
                                    && !tpl.name.to_ascii_lowercase().contains(&query)
                                    && !tpl.command.to_ascii_lowercase().contains(&query)
                                {
                                    continue;
                                }
                                if ui
                                    .selectable_label(
                                        self.selected_template_idx == Some(idx),
                                        format!("{}  {}", tpl.command, tpl.name),
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
                                egui::TextEdit::multiline(&mut self.template_content_buf)
                                    .desired_rows(10)
                                    .desired_width(ui.available_width()),
                            );
                            ui.label(i18n.t("chat.templatePlaceholderHint"));
                            ui.horizontal(|ui| {
                                if ui.button(i18n.t("chat.templateInsert")).clicked() {
                                    self.input = self.template_content_buf.clone();
                                    self.show_prompts = false;
                                }
                                if ui.button(i18n.t("chat.templateSave")).clicked() {
                                    let name = self.template_name_buf.trim().to_string();
                                    let command =
                                        Self::normalize_command(&self.template_command_buf);
                                    let content = self.template_content_buf.trim().to_string();
                                    if name.is_empty() || command.is_empty() || content.is_empty() {
                                        self.error = i18n.t("chat.templateValidation").to_string();
                                    } else if self.prompt_templates.iter().enumerate().any(
                                        |(idx, t)| {
                                            t.command == command
                                                && Some(idx) != self.selected_template_idx
                                        },
                                    ) {
                                        self.error = i18n.t("chat.templateDuplicate").to_string();
                                    } else {
                                        let tpl = PromptTemplate {
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
                                                        self.prompt_templates.len() + 1
                                                    )
                                                }),
                                            name,
                                            command,
                                            content,
                                        };
                                        if let Some(idx) = self.selected_template_idx {
                                            self.prompt_templates[idx] = tpl;
                                        } else {
                                            self.prompt_templates.push(tpl);
                                            self.selected_template_idx =
                                                Some(self.prompt_templates.len() - 1);
                                        }
                                        self.save_templates_to_disk();
                                        self.error.clear();
                                    }
                                }
                                if ui.button(i18n.t("chat.templateDelete")).clicked() {
                                    if let Some(idx) = self.selected_template_idx.take() {
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

        // ── Main layout: SidePanel + vertical right ──────────
        egui::SidePanel::left("chat_sidebar_panel")
            .default_width(220.0)
            .resizable(true)
            .min_width(140.0)
            .max_width(400.0)
            .show_inside(ui, |ui| {
                self.show_sidebar(ui, i18n);
            });

        // Right side: use the remaining space with a manual vertical layout.
        let avail = ui.available_size();
        let right_w = avail.x.max(200.0);
        let right_h = avail.y.max(200.0);
        // Reserve input area (260px) up front for stable height calculation
        // Includes: error(~20px) + attachments(~20px) + input_scroll(100px) + counter(20px) + buttons(40px) + spacing(~40px)
        let msg_h = (right_h - 260.0).max(80.0);

        ui.allocate_ui(egui::vec2(right_w, right_h), |ui| {
            // ── Mode/Model row (top, fixed) ────────────
            if CHAT_STAGE6_ENABLE_MODE_ROW {
                let dark = ui.visuals().dark_mode;
                let bg = if dark {
                    egui::Color32::from_rgb(36, 38, 44)
                } else {
                    egui::Color32::from_rgb(240, 242, 245)
                };
                let fg = if dark {
                    egui::Color32::from_rgb(220, 224, 234)
                } else {
                    egui::Color32::from_rgb(34, 34, 34)
                };
                egui::Frame::new()
                    .fill(bg)
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(10i8, 6i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(i18n.t("chat.mode")).color(fg).strong());
                            ui.add_space(6.0);
                            egui::ComboBox::from_id_salt("mode_sel")
                                .selected_text(i18n.t(&format!("mode.{}", self.selected_mode)))
                                .show_ui(ui, |ui| {
                                    for m in &["ask", "plan", "edit", "safeguard", "full_auto"] {
                                        ui.selectable_value(
                                            &mut self.selected_mode,
                                            m.to_string(),
                                            i18n.t(&format!("mode.{m}")),
                                        );
                                    }
                                });
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(i18n.t("chat.model")).color(fg));
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
                                    .color(fg)
                                    .size(12.0),
                            );
                            if !self.last_selected_agent.is_empty() {
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "agent: {}",
                                        self.last_selected_agent
                                    ))
                                    .color(fg)
                                    .size(11.0),
                                );
                            }
                            ui.add_space(12.0);
                            ui.checkbox(
                                &mut self.show_token_details,
                                i18n.t("chat.showTokenDetails"),
                            );
                        });
                    });
                ui.separator();
            }

            // ── Messages area: fixed height from outer pre-computed value ─
            if msg_h > 80.0 {
                egui::ScrollArea::vertical()
                    .id_salt("chat_messages_scroll")
                    .auto_shrink([false; 2])
                    .max_height(msg_h)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        self.show_messages(ui, i18n);
                    });
            }

            ui.separator();

            // ── Input area (bottom, fixed) ────────────
            if !self.error.is_empty() {
                ui.colored_label(egui::Color32::RED, &self.error);
            }

            self.render_token_stats(ui, i18n);

            if !self.attachments.is_empty() {
                ui.horizontal(|ui| {
                    for a in &self.attachments {
                        ui.label(format!(
                            "{} {}",
                            if a.mime.starts_with("image/") {
                                "🖼️"
                            } else {
                                "📎"
                            },
                            a.name
                        ));
                    }
                    if ui.button("✕").clicked() {
                        self.attachments.clear();
                    }
                });
            }

            // ── Paste event handling ────────────
            let pasted_atts = self.handle_paste_events(ui);
            if !pasted_atts.is_empty() {
                self.attachments.extend(pasted_atts);
                ctx.request_repaint_after(self.stream_repaint_interval);
            }

            // Fixed-height input area with scroll for long content.
            let input_resp = egui::ScrollArea::vertical()
                .id_salt("chat_input_scroll")
                .max_height(100.0)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_sized(
                        [ui.available_width(), 100.0],
                        egui::TextEdit::multiline(&mut self.input).hint_text(i18n.t("chat.input")),
                    )
                });
            // Check focus of the inner TextEdit via ScrollArea's inner response
            let input_focus = input_resp.inner.has_focus();

            // Character counter
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} chars", self.input.len()))
                        .weak()
                        .size(10.0),
                );
            });

            ui.horizontal(|ui| {
                if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS {
                    if ui
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
                    if ui
                        .button("📝")
                        .on_hover_text(i18n.t("chat.externalEditor"))
                        .clicked()
                    {
                        let p = std::env::temp_dir().join("go_on_chat_input.txt");
                        if let Err(e) = std::fs::write(&p, &self.input) {
                            self.error =
                                format!("Failed to write temp file for external editor: {e}");
                        }
                        #[cfg(target_os = "windows")]
                        let editors = &["notepad", "code", "zed"];
                        #[cfg(target_os = "macos")]
                        let editors = &["open", "code", "zed", "TextEdit"];
                        #[cfg(target_os = "linux")]
                        let editors = &["zed", "code", "gedit", "vim", "nano"];
                        #[cfg(not(any(
                            target_os = "windows",
                            target_os = "macos",
                            target_os = "linux"
                        )))]
                        let editors: &[&str] = &["code", "vim", "nano"];
                        for e in editors {
                            if let Ok(mut child) = std::process::Command::new(e).arg(&p).spawn() {
                                let _ = child.wait();
                                if let Ok(edited) = std::fs::read_to_string(&p) {
                                    let trimmed = edited.trim().to_string();
                                    if !trimmed.is_empty() {
                                        self.input = trimmed;
                                    }
                                }
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
                ui.label(
                    egui::RichText::new(i18n.t(if cfg!(target_os = "linux") {
                        "chat.sendShortcutHintLinux"
                    } else {
                        "chat.sendShortcutHint"
                    }))
                    .small()
                    .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.sending && self.ai_status == AiStatus::Thinking {
                        if ui
                            .add(
                                egui::Button::new(format!("⏹ {}", i18n.t("chat.stop")))
                                    .fill(egui::Color32::RED),
                            )
                            .clicked()
                        {
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
                        if ui
                            .add_enabled(
                                !self.sending,
                                egui::Button::new(format!("▶ {}", icon))
                                    .fill(col)
                                    .min_size(egui::vec2(80.0, 28.0)),
                            )
                            .clicked()
                        {
                            self.send_message(backend, ctx, autotune_chain_enabled);
                        }
                    }
                });
            });

            // Enter send
            // Enter to send (Ctrl+Enter on Linux to avoid accidental sends in terminal)
            if ui.input(|i| {
                if !input_focus || !i.key_pressed(egui::Key::Enter) || i.modifiers.shift {
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
            }) {
                self.send_message(backend, ctx, autotune_chain_enabled);
            }

            // ── Global keyboard shortcuts ─────────
            ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::CTRL, egui::Key::N)
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)
                {
                    self.new_session();
                    self.refresh_default_session_names(i18n);
                }
                if i.consume_key(egui::Modifiers::CTRL, egui::Key::L)
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::L)
                {
                    self.input.clear();
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                    self.show_prompts = false;
                    self.show_model_picker = false;
                }
            });
        });
    }

    // ── Sidebar: session list ───────────────────────────────────
    fn show_sidebar(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(i18n.t("chat.title"));
                // Export button
                if ui
                    .button("📤")
                    .on_hover_text(i18n.t("chat.exportSession"))
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
                    .button("📂")
                    .on_hover_text(i18n.t("chat.openConfigDir"))
                    .clicked()
                {
                    if let Some(dirs) = directories::ProjectDirs::from("com", "goon", "go-on-gui") {
                        let config_dir = dirs.config_dir();
                        #[cfg(target_os = "windows")]
                        if let Err(e) = std::process::Command::new("cmd")
                            .args(["/c", "start", "", &config_dir.display().to_string()])
                            .spawn()
                        {
                            eprintln!("Failed to open config directory: {e}");
                        }
                        #[cfg(target_os = "macos")]
                        if let Err(e) = std::process::Command::new("open").arg(config_dir).spawn() {
                            eprintln!("Failed to open config directory: {e}");
                        }
                        #[cfg(target_os = "linux")]
                        if let Err(e) = std::process::Command::new("xdg-open")
                            .arg(config_dir)
                            .spawn()
                        {
                            eprintln!("Failed to open config directory: {e}");
                        }
                        #[cfg(not(any(
                            target_os = "windows",
                            target_os = "macos",
                            target_os = "linux"
                        )))]
                        if let Err(e) = std::process::Command::new("xdg-open")
                            .arg(config_dir)
                            .spawn()
                        {
                            eprintln!("Failed to open config directory: {e}");
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
                if ui
                    .button("🗑")
                    .on_hover_text(i18n.t("chat.clearSession"))
                    .clicked()
                {
                    if let Some(session) = self.sessions.get_mut(self.active_session) {
                        session.messages.clear();
                        self.save_sessions_to_disk();
                    }
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
            // Generate Workflow button
            if ui
                .button("🔄 ".to_string() + &i18n.t("chat.generateWorkflow"))
                .on_hover_text(i18n.t("chat.generateWorkflowHint"))
                .clicked()
            {
                // Collect all user messages from current session
                let msgs = self.messages();
                let user_msgs: Vec<String> = msgs
                    .iter()
                    .filter(|m| m.role == "user")
                    .map(|m| m.content.clone())
                    .collect();

                if user_msgs.is_empty() {
                    self.error = i18n.t("chat.noMessagesForWorkflow").to_string();
                } else {
                    // The actual async call is in the process_pending flow
                    // For now, show feedback that workflow generation was triggered
                    self.error = i18n.t("chat.workflowGenerationStarted").to_string();
                    // In a full implementation, this would call workflow.generate_from_chat RPC
                }
            }
            ui.separator();
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut to_remove: Option<usize> = None;
                    // Feature 9: filter by search query
                    let filtered_indices: Vec<usize> = if self.session_search_query.is_empty() {
                        self.sessions
                            .iter()
                            .enumerate()
                            .map(|(idx, _)| idx)
                            .collect()
                    } else {
                        let q = self.session_search_query.to_lowercase();
                        self.sessions
                            .iter()
                            .enumerate()
                            .filter(|(_, s)| s.name.to_lowercase().contains(&q))
                            .map(|(idx, _)| idx)
                            .collect()
                    };
                    for idx in filtered_indices {
                        let selected = idx == self.active_session;

                        // Session row with rename support
                        let is_renaming = self.rename_session_idx == Some(idx);
                        if is_renaming {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.rename_session_buf)
                                        .hint_text(i18n.t("chat.sessionNamePlaceholder"))
                                        .desired_width(140.0),
                                );
                                if ui.button(i18n.t("chat.save")).clicked() {
                                    let new_name = self.rename_session_buf.trim().to_string();
                                    if !new_name.is_empty() {
                                        if let Some(s) = self.sessions.get_mut(idx) {
                                            s.name = new_name;
                                            self.save_sessions_to_disk();
                                        }
                                    }
                                    self.rename_session_idx = None;
                                    self.rename_session_buf.clear();
                                }
                                if ui.button(i18n.t("chat.cancel")).clicked() {
                                    self.rename_session_idx = None;
                                    self.rename_session_buf.clear();
                                }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                let resp = ui.selectable_label(selected, &self.sessions[idx].name);
                                if resp.double_clicked() {
                                    self.rename_session_idx = Some(idx);
                                    self.rename_session_buf = self.sessions[idx].name.clone();
                                } else if resp.clicked() {
                                    self.active_session = idx;
                                    self.selected_mode = self.sessions[idx].mode.clone();
                                    self.selected_phase = self.sessions[idx].phase.clone();
                                    self.selected_models = if self.sessions[idx].models.is_empty() {
                                        vec!["auto".to_string()]
                                    } else {
                                        self.sessions[idx].models.clone()
                                    };
                                    self.sync_model_selection();
                                    self.ai_status = AiStatus::Idle;
                                    self.edit_msg_idx = None;
                                    self.edit_msg_buf.clear();
                                    self.rename_session_idx = None;
                                    self.rename_session_buf.clear();
                                }
                                // Delete button
                                if ui
                                    .button("✕")
                                    .on_hover_text(i18n.t("chat.deleteSession"))
                                    .clicked()
                                {
                                    to_remove = Some(idx);
                                }
                            });
                        }
                        // Mode/phase indicator as a simple label
                        ui.label(format!(
                            "{} | {}",
                            i18n.t(&format!("mode.{}", self.sessions[idx].mode)),
                            i18n.t(&format!("phase.{}", self.sessions[idx].phase)),
                        ));
                        ui.add_space(4.0);
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
                        } else {
                            // Can't delete last session — show feedback
                            self.error = i18n.t("chat.cannotDeleteLastSession").to_string();
                        }
                    }
                });
        });
    }

    // ── Messages area ─────────────────────
    fn show_messages(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        let total_msgs = self.messages().len();
        let start_idx = total_msgs.saturating_sub(Self::MAX_RENDERED_MESSAGES);

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

        if start_idx > 0 {
            ui.colored_label(
                egui::Color32::from_rgb(140, 142, 150),
                i18n.t("chat.showingLatest")
                    .replace("{shown}", &(total_msgs - start_idx).to_string())
                    .replace("{total}", &total_msgs.to_string()),
            );
            ui.add_space(4.0);
        }

        // Show ALL messages (no pagination)
        let dark_mode = ui.visuals().dark_mode;
        let msgs = self.messages().to_vec();

        // Cache formatted timestamps to avoid re-allocating per message per frame
        let mut last_ts: u64 = 0;
        let mut last_time_str = String::new();
        for (msg_idx, msg) in msgs.iter().enumerate() {
            if msg_idx < start_idx {
                continue;
            }
            // ── Edit mode: show TextEdit instead of bubble ────────
            if self.edit_msg_idx == Some(msg_idx) {
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(255, 220, 100, 30))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                    .show(ui, |ui| {
                        let edit_title = i18n.t("chat.editTitle");
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("✏️ {edit_title}"))
                                    .strong()
                                    .size(13.0),
                            );
                        });
                        ui.add_space(4.0);

                        let _edit_resp = ui.add_sized(
                            [ui.available_width(), 100.0],
                            egui::TextEdit::multiline(&mut self.edit_msg_buf)
                                .hint_text(i18n.t("chat.editTitle")),
                        );

                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui
                                .button(
                                    egui::RichText::new(format!("💾 {}", i18n.t("chat.saveEdit")))
                                        .strong(),
                                )
                                .clicked()
                            {
                                let new_content = self.edit_msg_buf.trim().to_string();
                                if !new_content.is_empty() {
                                    if let Some(session) =
                                        self.sessions.get_mut(self.active_session)
                                    {
                                        if msg_idx < session.messages.len() {
                                            session.messages[msg_idx].content = new_content;
                                            self.save_sessions_to_disk();
                                        }
                                    }
                                }
                                self.edit_msg_idx = None;
                                self.edit_msg_buf.clear();
                            }
                            if ui
                                .button(format!("✕ {}", i18n.t("chat.cancelEdit")))
                                .clicked()
                            {
                                self.edit_msg_idx = None;
                                self.edit_msg_buf.clear();
                            }
                        });
                    });
                ui.add_space(8.0);
                continue;
            }

            let is_user = msg.role == "user";
            let (bubble_color, text_color) = if is_user {
                let bc = if dark_mode {
                    egui::Color32::from_rgb(32, 112, 210)
                } else {
                    egui::Color32::from_rgb(10, 106, 255)
                };
                (bc, egui::Color32::WHITE)
            } else {
                // Green background for AI messages (more prominent)
                let bc = if dark_mode {
                    egui::Color32::from_rgb(20, 90, 60)
                } else {
                    egui::Color32::from_rgb(200, 240, 210)
                };
                let tc = if dark_mode {
                    egui::Color32::from_rgb(240, 244, 248)
                } else {
                    egui::Color32::from_rgb(20, 20, 24)
                };
                (bc, tc)
            };

            let time_str = if msg.timestamp == last_ts {
                last_time_str.as_str()
            } else {
                last_ts = msg.timestamp;
                last_time_str = format_absolute_time(msg.timestamp);
                last_time_str.as_str()
            };
            let model_name = msg.model.clone();

            // Clone content once — used for display_text AND context menu (avoids borrowing msg in closures)
            let ctx_content = msg.content.clone();
            let _ctx_plain = Self::markdown_to_plain_text(&ctx_content);

            // Single clone: compute display_text once, keep content reference for context menu
            let display_text = if CHAT_DISABLE_MARKDOWN_RENDER {
                Self::markdown_to_plain_text(&ctx_content)
            } else {
                ctx_content.clone()
            };

            // Timestamp row
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(time_str)
                        .color(egui::Color32::from_rgb(160, 162, 170))
                        .size(11.0),
                );
                if !model_name.is_empty() {
                    ui.label(
                        egui::RichText::new(&model_name)
                            .color(egui::Color32::from_rgb(140, 142, 150))
                            .size(10.0),
                    );
                }
            });

            // Bubble content width: restrict to a reasonable max so text wraps
            // Leave space for avatar and margins (about 60px total)
            let max_bubble_width = (ui.available_width() - 60.0).clamp(200.0, 800.0);

            // Pre-compute the timestamp for context menu use
            let msg_timestamp = msg.timestamp;

            // Helper to find real message index in session messages
            let _find_in_messages =
                |msgs_slice: &[Message], ts: u64, content: &str| -> Option<usize> {
                    msgs_slice
                        .iter()
                        .position(|m| m.timestamp == ts && m.content == content)
                };

            // Pre-compute msg/self values before closures to avoid borrow conflicts
            // with mutable self access in context_menu (nested closure).
            let msg_thinking = msg.thinking.clone();
            let msg_has_thinking = !msg_thinking.is_empty() && !is_user;
            let _msg_timestamp_val = msg_timestamp;
            let _msg_model_val = msg.model.clone();
            let enable_markdown_val = self.enable_markdown;
            let _active_session_val = self.active_session;
            // All messages left-aligned — color differentiates user vs AI.
            ui.horizontal_top(|ui| {
                Self::draw_role_avatar(ui, is_user);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_max_width(max_bubble_width);
                    egui::Frame::new()
                        .fill(bubble_color)
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                        .show(ui, |ui| {
                            ui.set_max_width(max_bubble_width - 20.0);

                            // Inline action bar (no context_menu overlay that blocks clicks)
                            let ctx_session_msgs = self.messages().to_vec();
                            let _ctx_content = msg.content.clone();
                            let _ctx_plain = Self::markdown_to_plain_text(&msg.content);
                            let _copy = |s: &str| i18n.t(s).to_string();
                            ui.horizontal(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        if ui
                                            .button("📋")
                                            .on_hover_text(i18n.t("chat.copyMessage"))
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(msg.content.clone());
                                        }
                                        if ui
                                            .button("✏️")
                                            .on_hover_text(i18n.t("chat.edit"))
                                            .clicked()
                                        {
                                            if let Some(idx) =
                                                ctx_session_msgs.iter().position(|m| {
                                                    m.timestamp == msg_timestamp
                                                        && m.content == msg.content
                                                })
                                            {
                                                self.edit_msg_idx = Some(idx);
                                                self.edit_msg_buf = msg.content.clone();
                                            }
                                        }
                                        if ui
                                            .button("🗑")
                                            .on_hover_text(i18n.t("chat.delete"))
                                            .clicked()
                                        {
                                            if let Some(idx) =
                                                ctx_session_msgs.iter().position(|m| {
                                                    m.timestamp == msg_timestamp
                                                        && m.content == msg.content
                                                })
                                            {
                                                self.remove_message_at(idx);
                                                self.save_sessions_to_disk();
                                            }
                                        }
                                    },
                                );
                            });

                            let trunc_hint = i18n.t("chat.largeMessageTruncated").to_string();
                            Self::render_markdown(
                                ui,
                                &display_text,
                                &i18n.t("chat.copyCode"),
                                enable_markdown_val,
                                text_color,
                                &trunc_hint,
                            );

                            // ── Thinking section: Button toggle instead of CollapsingHeader ──
                            // (CollapsingHeader inside Frame may not receive clicks due to
                            // bubble context_menu overlay, so we use a manual toggle.)
                            if msg_has_thinking {
                                ui.add_space(6.0);

                                let is_expanded = self.show_thinking_idx == Some(msg_idx);
                                let toggle_icon = if is_expanded { "▼" } else { "▶" };
                                let thinking_label = format!(
                                    "{} {}  {}  ({})",
                                    toggle_icon,
                                    "💭",
                                    i18n.t("chat.thinkingLabel"),
                                    msg_thinking.chars().count()
                                );

                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new(&thinking_label)
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(180, 140, 60)),
                                        )
                                        .fill(egui::Color32::TRANSPARENT)
                                        .corner_radius(4.0)
                                        .min_size(egui::vec2(ui.available_width(), 18.0)),
                                    )
                                    .clicked()
                                {
                                    if is_expanded {
                                        self.show_thinking_idx = None;
                                    } else {
                                        self.show_thinking_idx = Some(msg_idx);
                                    }
                                }

                                if is_expanded {
                                    ui.add_space(4.0);
                                    egui::Frame::new()
                                        .fill(egui::Color32::from_rgba_premultiplied(
                                            60, 50, 20, 30,
                                        ))
                                        .corner_radius(6.0)
                                        .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                                        .show(ui, |ui| {
                                            Self::render_markdown(
                                                ui,
                                                &msg_thinking,
                                                &i18n.t("chat.copyCode"),
                                                enable_markdown_val,
                                                egui::Color32::from_rgb(160, 162, 170),
                                                &trunc_hint,
                                            );
                                            ui.horizontal(|ui| {
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if ui
                                                            .button(i18n.t("common.copyButton"))
                                                            .clicked()
                                                        {
                                                            ui.ctx()
                                                                .copy_text(msg_thinking.clone());
                                                        }
                                                    },
                                                );
                                            });
                                        });
                                }
                            }
                        });
                });
            });
            ui.add_space(6.0);
        }

        // Feature 5a: show AI thinking indicator while streaming
        if self.ai_status == AiStatus::Thinking && !self.sessions.is_empty() {
            if let Some(session) = self.sessions.get(self.active_session) {
                if let Some(last) = session.messages.last() {
                    if last.role == "assistant"
                        && last.content.is_empty()
                        && last.thinking.is_empty()
                    {
                        // Show thinking indicator after the last placeholder message
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            Self::draw_role_avatar(ui, false);
                            ui.add_space(6.0);
                            ui.add(egui::Spinner::new());
                            ui.colored_label(
                                egui::Color32::from_rgb(160, 162, 170),
                                i18n.t("chat.thinking"),
                            );
                        });
                        ui.add_space(6.0);
                    }
                }
            }
        }

        // Feature 5b: show aggregate token estimate below last AI message
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
    }
}
