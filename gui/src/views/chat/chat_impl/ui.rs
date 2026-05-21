use super::*;
use std::hash::{Hash, Hasher};

/// Map mode ID to its i18n key for display.
fn mode_display_key(mode_id: &str) -> String {
    match mode_id {
        "ask" => "mode.ask".to_string(),
        "plan" => "mode.plan".to_string(),
        "edit" => "mode.edit".to_string(),
        "safeguard" => "mode.safeguard".to_string(),
        "full_auto" => "mode.full_auto".to_string(),
        other => format!("mode.{other}"),
    }
}

impl ChatView {
    const MAX_RENDERED_MESSAGES: usize = 250;

    /// Draw a colored circle avatar with the role initial letter.
    /// User gets a blue circle with "U", AI gets a green circle with "A".
    fn draw_role_avatar(ui: &mut egui::Ui, is_user: bool) {
        let size = 28.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let painter = ui.painter();
        let dark = ui.visuals().dark_mode;
        // Zed-style: subtle avatar colors
        let color = if is_user {
            if dark {
                egui::Color32::from_rgb(40, 100, 200)
            } else {
                egui::Color32::from_rgb(0, 95, 240)
            }
        } else {
            if dark {
                egui::Color32::from_rgb(60, 64, 74)
            } else {
                egui::Color32::from_rgb(180, 183, 190)
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

        // Process any pending async responses — triggers ctx.request_repaint()
        // when new stream data arrives, providing data-driven frame timing.
        self.process_pending(i18n, ctx);

        // Data-driven repaint: process_pending() calls ctx.request_repaint()
        // when new stream data arrives.  No periodic timer — frame pacing
        // is handled entirely by GPU vsync, eliminating micro-jitter.
        // The sending-state transition below ensures the first frame after
        // send/stop is rendered immediately.
        if self.sending != self.last_sending {
            self.last_sending = self.sending;
            ctx.request_repaint();
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
                        // Try to load phases from backend config baseline response.
                        // The backend may not include "flow.phases", so fall back to
                        // the hardcoded default phases that the GUI always generates.
                        let phases: Vec<String> = baseline
                            .get("config")
                            .and_then(|c| c.get("flow"))
                            .and_then(|f| f.get("phases"))
                            .and_then(|p| p.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_else(|| {
                                vec![
                                    "planning".to_string(),
                                    "coding".to_string(),
                                    "review".to_string(),
                                    "delivery".to_string(),
                                ]
                            });
                        if let Err(e) = tx.try_send(PendingResponse::Phases(phases)) {
                            eprintln!("WARN: chat ui try_send failed: {:?}", e);
                        }
                        ctx_clone.request_repaint();
                    }
                    _ => {
                        #[cfg(debug_assertions)]
                        eprintln!("Warning: Failed to load phases from backend (timeout or error)");
                    }
                }
            });
        }

        if !self.models_loaded
            && self.last_models_fetch.elapsed() > std::time::Duration::from_secs(3)
        {
            self.last_models_fetch = std::time::Instant::now();
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
                        if let Err(e) = tx.try_send(PendingResponse::Models(models)) {
                            eprintln!("WARN: chat ui try_send failed: {:?}", e);
                        }
                        ctx_clone.request_repaint();
                    }
                    Err(_) => {
                        eprintln!("Warning: Failed to load models from backend (timeout)");
                        ctx_clone.request_repaint();
                    }
                }
            });
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
                .show(ctx, |ui| {
                    egui::Frame::NONE.show(ui, |ui| {
                        ui.label("Select a model (AUTO lets phase decide):");
                        ui.separator();

                        let mut models_list = self.available_models.clone();
                        if models_list.is_empty() {
                            models_list.push("auto".to_string());
                        }
                        if self.available_agent_models.contains_key("copilot") {
                            models_list.push(ChatView::COPILOT_AUTO_MODEL.to_string());
                        }

                        egui::ComboBox::from_label("Model")
                            .selected_text(if self.selected_model == ChatView::COPILOT_AUTO_MODEL {
                                "copilot/auto".to_string()
                            } else {
                                self.selected_model.clone()
                            })
                            .show_ui(ui, |ui| {
                                for model in &models_list {
                                    let label = if model == ChatView::COPILOT_AUTO_MODEL {
                                        "copilot/auto"
                                    } else {
                                        model.as_str()
                                    };
                                    ui.selectable_value(
                                        &mut self.selected_model,
                                        model.clone(),
                                        label,
                                    );
                                }
                            });

                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button(i18n.t("chat.close")).clicked() {
                                self.show_model_picker = false;
                                self.sync_model_selection();
                            }
                        });
                    });
                });
        }

        // ── Prompt Category Browser (Zed-style) ──────────────
        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS && self.show_prompts {
            let has_collection = !self.prompt_collection.is_empty();
            let has_custom = !self.prompt_templates.is_empty();
            if !has_collection && !has_custom {
                // Nothing to show — close the window
                self.show_prompts = false;
            } else {
                let cat_id = egui::Id::new("prompt_category_browser");
                let win_w = if has_collection { 640.0 } else { 400.0 };
                egui::Window::new(i18n.t("chat.promptTemplates"))
                    .id(cat_id)
                    .collapsible(false)
                    .resizable(true)
                    .default_width(win_w)
                    .default_height(420.0)
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        egui::Frame::NONE.show(ui, |ui| {
                            // Search bar
                            let dark = ui.visuals().dark_mode;
                            let muted = if dark {
                                egui::Color32::from_rgb(140, 142, 150)
                            } else {
                                egui::Color32::from_rgb(110, 112, 120)
                            };
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.template_search_query)
                                        .hint_text(i18n.t("chat.searchTemplates"))
                                        .desired_width(260.0),
                                );
                                if has_custom
                                    && ui
                                        .button("✚")
                                        .on_hover_text(i18n.t("chat.templateNew"))
                                        .clicked()
                                {
                                    self.selected_template_idx = None;
                                    self.template_name_buf.clear();
                                    self.template_command_buf.clear();
                                    self.template_content_buf.clear();
                                }
                            });
                            ui.separator();

                            let query = self.template_search_query.to_ascii_lowercase();

                        if has_collection {
                            // ── Two-column: categories | templates ──
                            ui.columns(2, |cols| {
                                // Left: category list
                                egui::ScrollArea::vertical()
                                    .id_salt("prompt_cat_list")
                                    .max_height(320.0)
                                    .show(&mut cols[0], |ui| {
                                        for cat in &self.prompt_collection {
                                            let cat_id_str = &cat.id;
                                            // Filter: count visible templates
                                            let visible_count = if query.is_empty() {
                                                cat.templates.len()
                                            } else {
                                                cat.templates.iter().filter(|t| {
                                                    t.title.to_ascii_lowercase().contains(&query)
                                                        || t.tags.iter().any(|tag| tag.to_ascii_lowercase().contains(&query))
                                                }).count()
                                            };
                                            if visible_count == 0 && !query.is_empty() {
                                                continue;
                                            }
                                            let is_sel = self.prompt_selected_category
                                                .as_deref() == Some(cat_id_str);
                                            let label = format!("{}  {} ({})", cat.icon, cat.name, visible_count);
                                            if ui.selectable_label(is_sel, &label).clicked() {
                                                self.prompt_selected_category = Some(cat_id_str.clone());
                                            }
                                        }
                                    });

                                // Right: template list for selected category
                                egui::ScrollArea::vertical()
                                    .id_salt("prompt_tpl_list")
                                    .max_height(320.0)
                                    .show(&mut cols[1], |ui| {
                                        let mut inserted = false;
                                        if let Some(ref sel_id) = self.prompt_selected_category {
                                            if let Some(cat) = self.prompt_collection.iter().find(|c| &c.id == sel_id) {
                                                for tpl in &cat.templates {
                                                    if !query.is_empty()
                                                        && !tpl.title.to_ascii_lowercase().contains(&query)
                                                        && !tpl.tags.iter().any(|t| t.to_ascii_lowercase().contains(&query))
                                                    {
                                                        continue;
                                                    }
                                                    let tpl_bg = if dark {
                                                        egui::Color32::from_rgb(36, 38, 44)
                                                    } else {
                                                        egui::Color32::from_rgb(245, 246, 248)
                                                    };
                                                    let response = egui::Frame::new()
                                                        .fill(tpl_bg)
                                                        .corner_radius(6.0)
                                                        .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                                                        .show(ui, |ui| {
                                                            ui.set_min_width(240.0);
                                                            ui.label(egui::RichText::new(&tpl.title).strong().size(13.0));
                                                            ui.add_space(2.0);
                                                            ui.label(egui::RichText::new(&tpl.description).size(11.0).color(muted));
                                                            if !tpl.tags.is_empty() {
                                                                ui.add_space(4.0);
                                                                ui.horizontal_wrapped(|ui| {
                                                                    for tag in &tpl.tags {
                                                                        ui.label(
                                                                            egui::RichText::new(format!("#{}", tag))
                                                                                .size(9.0)
                                                                                .color(egui::Color32::from_rgb(100, 140, 200)),
                                                                        );
                                                                    }
                                                                });
                                                            }
                                                        });
                                                    if response.response.clicked() {
                                                        self.input = tpl.content.clone();
                                                        self.show_prompts = false;
                                                        inserted = true;
                                                    }
                                                    if inserted { break; }
                                                    ui.add_space(4.0);
                                                }
                                            }
                                        } else {
                                            ui.colored_label(muted, i18n.t("chat.selectCategoryHint"));
                                        }
                                        if !inserted && query.is_empty() && self.prompt_selected_category.is_none() {
                                            // Auto-select first non-empty category
                                            if let Some(first) = self.prompt_collection.first() {
                                                self.prompt_selected_category = Some(first.id.clone());
                                            }
                                        }
                                    });
                            });
                        } else if has_custom {
                            // Only custom templates — show simple list
                            let mut pick_idx = None;
                            for (idx, tpl) in self.prompt_templates.iter().enumerate() {
                                if ui.selectable_label(
                                    self.selected_template_idx == Some(idx),
                                    format!("{}  {}", tpl.command, tpl.name),
                                ).clicked() {
                                    pick_idx = Some(idx);
                                }
                            }
                            if let Some(idx) = pick_idx {
                                self.load_template_into_editor(idx);
                            }
                            ui.separator();
                            ui.label(i18n.t("chat.templateName"));
                            ui.text_edit_singleline(&mut self.template_name_buf);
                            ui.label(i18n.t("chat.templateCommand"));
                            ui.text_edit_singleline(&mut self.template_command_buf);
                            ui.label(i18n.t("chat.templateBody"));
                            ui.add(
                                egui::TextEdit::multiline(&mut self.template_content_buf)
                                    .desired_rows(8)
                                    .desired_width(ui.available_width()),
                            );
                            ui.horizontal(|ui| {
                                if ui.button(i18n.t("chat.templateInsert")).clicked() {
                                    self.input = self.template_content_buf.clone();
                                    self.show_prompts = false;
                                }
                                if ui.button(i18n.t("chat.templateSave")).clicked() {
                                    let name = self.template_name_buf.trim().to_string();
                                    let cmd = Self::normalize_command(&self.template_command_buf);
                                    let content = self.template_content_buf.trim().to_string();
                                    if !name.is_empty() && !cmd.is_empty() && !content.is_empty() {
                                        self.prompt_templates.push(PromptTemplate {
                                            id: format!("tpl_{}", self.next_template_id),
                                            name, command: cmd, content,
                                        });
                                        self.next_template_id += 1;
                                        self.save_templates_to_disk();
                                        self.error.clear();
                                    }
                                }
                            });
                        }

                            ui.separator();
                            if ui.button(i18n.t("chat.close")).clicked() {
                                self.show_prompts = false;
                            }
                        });
                    });
            }
        }

        if CHAT_STAGE6_ENABLE_EXTRA_BUTTONS && self.show_risk_decision {
            egui::Window::new(i18n.t("chat.riskDecisionTitle"))
                .id(egui::Id::new("chat_risk_decision_window"))
                .collapsible(false)
                .resizable(true)
                .default_width(520.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("chat.riskDecisionState"));
                        ui.selectable_value(
                            &mut self.risk_is_high,
                            false,
                            i18n.t("chat.riskDecisionNormal"),
                        );
                        ui.selectable_value(
                            &mut self.risk_is_high,
                            true,
                            i18n.t("chat.riskDecisionHigh"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("chat.riskDecisionReview"));
                        ui.selectable_value(
                            &mut self.risk_review_required,
                            true,
                            i18n.t("chat.riskDecisionReviewRequired"),
                        );
                        ui.selectable_value(
                            &mut self.risk_review_required,
                            false,
                            i18n.t("chat.riskDecisionNoReview"),
                        );
                    });
                    ui.add_space(6.0);
                    ui.label(i18n.t("chat.riskDecisionStrategy"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.risk_strategy)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label(i18n.t("chat.riskDecisionReasons"));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.risk_reasons)
                            .desired_rows(5)
                            .desired_width(f32::INFINITY),
                    );

                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button(i18n.t("chat.templateInsert")).clicked() {
                            let state = if self.risk_is_high {
                                i18n.t("chat.riskDecisionHigh")
                            } else {
                                i18n.t("chat.riskDecisionNormal")
                            };
                            let review = if self.risk_review_required {
                                i18n.t("chat.riskDecisionReviewRequired")
                            } else {
                                i18n.t("chat.riskDecisionNoReview")
                            };
                            let block = format!(
                                "[{}]\n- {}: {}\n- {}: {}\n- {}: {}\n- {}: {}",
                                i18n.t("chat.riskDecisionTitle"),
                                i18n.t("chat.riskDecisionState"),
                                state,
                                i18n.t("chat.riskDecisionReview"),
                                review,
                                i18n.t("chat.riskDecisionStrategy"),
                                self.risk_strategy.trim(),
                                i18n.t("chat.riskDecisionReasons"),
                                self.risk_reasons.trim()
                            );
                            if self.input.trim().is_empty() {
                                self.input = block;
                            } else {
                                self.input = format!("{}\n\n{}", self.input, block);
                            }
                            self.show_risk_decision = false;
                        }
                        if ui.button(i18n.t("chat.close")).clicked() {
                            self.show_risk_decision = false;
                        }
                    });
                });
        }

        // ── Main layout: SidePanel + vertical right ──────────
        egui::SidePanel::left("chat_sidebar_panel")
            .default_width(220.0)
            .resizable(true)
            .min_width(140.0)
            .max_width(400.0)
            .show_inside(ui, |ui| {
                self.show_sidebar(ui, i18n, backend, ctx);
            });

        // Right side: use the remaining space with a manual vertical layout.
        let avail = ui.available_size();
        let right_w = avail.x.max(200.0);
        let right_h = avail.y.max(200.0);
        // Messages get remaining height after mode row (~50px) and input area (~260px)
        // Minimum 40px so messages are always visible even on small windows.
        let msg_h = (right_h - 310.0).max(40.0);

        ui.allocate_ui(egui::vec2(right_w, right_h), |ui| {
            // ── Mode/Model row (top, fixed) ────────────
            // Zed-style: compact mode row with subtle background
            if CHAT_STAGE6_ENABLE_MODE_ROW {
                let dark = ui.visuals().dark_mode;
                let bg = if dark {
                    egui::Color32::from_rgb(30, 32, 38)
                } else {
                    egui::Color32::from_rgb(245, 246, 248)
                };
                let fg = if dark {
                    egui::Color32::from_rgb(190, 194, 204)
                } else {
                    egui::Color32::from_rgb(80, 82, 90)
                };
                egui::Frame::new()
                    .fill(bg)
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(10i8, 6i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(i18n.t("chat.mode")).color(fg).strong());
                            ui.add_space(6.0);
                            let prev_mode = self.selected_mode.clone();
                            egui::ComboBox::from_id_salt("mode_sel")
                                .selected_text(i18n.t(&mode_display_key(&self.selected_mode)))
                                .show_ui(ui, |ui| {
                                    for m in &["ask", "plan", "edit", "safeguard", "full_auto"] {
                                        ui.selectable_value(
                                            &mut self.selected_mode,
                                            m.to_string(),
                                            i18n.t(&mode_display_key(m)),
                                        );
                                    }
                                });
                            if self.selected_mode != prev_mode {
                                // Mode changed — future: trigger model reload or state save
                            }
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(i18n.t("chat.model")).color(fg));
                            ui.add_space(6.0);
                            // ── Two-level model picker: Agent → Model ────
                            let agent_keys: Vec<String> =
                                self.available_agent_models.keys().cloned().collect();
                            let prev_model = self.selected_model.clone();

                            // Level 1: Agent (ComboBox)
                            let agent_text = if self.selected_agent.is_empty() {
                                "All Agents".to_string()
                            } else {
                                self.selected_agent.clone()
                            };
                            egui::ComboBox::from_id_salt("agent_sel")
                                .selected_text(&agent_text)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_value(
                                            &mut self.selected_agent,
                                            String::new(),
                                            "All Agents",
                                        )
                                        .clicked()
                                    {
                                        self.selected_model = "auto".to_string();
                                        self.sync_model_selection();
                                    }
                                    for agent in &agent_keys {
                                        if ui
                                            .selectable_value(
                                                &mut self.selected_agent,
                                                agent.clone(),
                                                agent.as_str(),
                                            )
                                            .clicked()
                                        {
                                            // Reset to AUTO when agent changes
                                            self.selected_model = "auto".to_string();
                                            self.sync_model_selection();
                                        }
                                    }
                                });

                            ui.add_space(4.0);

                            // Level 2: Model (ComboBox, filtered by selected agent)
                            // ── Copilot agent: model is fixed to "copilot-auto" ──
                            //   When the agent is "copilot", the model selection is frozen to
                            //   "copilot-auto" because VS Code / GitHub Copilot's own extension
                            //   handles model selection transparently.  The user can still change
                            //   the agent to see model options for other providers.
                            if self.selected_agent == "copilot" {
                                ui.label("copilot/auto");
                            } else {
                                let model_options: Vec<String> = if self.selected_agent.is_empty() {
                                    // Show all models from all agents
                                    let mut all: Vec<String> = self.available_models.clone();
                                    all.insert(0, "auto".to_string());
                                    // Inject "copilot-auto" as a selectable option so that
                                    // users can switch to the Copilot auto-select mode from
                                    // the "All Agents" view.
                                    if self.available_agent_models.contains_key("copilot") {
                                        all.push(ChatView::COPILOT_AUTO_MODEL.to_string());
                                    }
                                    all
                                } else {
                                    let mut agent_models = self
                                        .available_agent_models
                                        .get(&self.selected_agent)
                                        .cloned()
                                        .unwrap_or_default();
                                    agent_models.insert(0, "auto".to_string());
                                    agent_models
                                };
                                egui::ComboBox::from_id_salt("model_sel")
                                    .selected_text(if self.selected_model == "auto" {
                                        "AUTO".to_string()
                                    } else if self.selected_model == ChatView::COPILOT_AUTO_MODEL {
                                        "copilot/auto".to_string()
                                    } else {
                                        self.selected_model.clone()
                                    })
                                    .show_ui(ui, |ui| {
                                        for m in &model_options {
                                            let label = if m == "auto" {
                                                "AUTO"
                                            } else if m == ChatView::COPILOT_AUTO_MODEL {
                                                "copilot/auto"
                                            } else {
                                                m.as_str()
                                            };
                                            ui.selectable_value(
                                                &mut self.selected_model,
                                                m.to_string(),
                                                label,
                                            );
                                        }
                                    });
                            }
                            if self.selected_model != prev_model {
                                self.sync_model_selection();
                            }
                            ui.add_space(12.0);
                            let _tok_resp = ui.checkbox(
                                &mut self.show_token_details,
                                i18n.t("chat.showTokenDetails"),
                            );
                            // Token details checkbox changed — no additional action needed
                        });
                    });
                ui.separator();
            }

            // ── Messages area ─
            egui::ScrollArea::vertical()
                .id_salt("chat_messages_scroll")
                .auto_shrink([false; 2])
                .max_height(msg_h)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    self.show_messages(ui, i18n);
                });

            ui.separator();

            // ── Input area (bottom, fixed) ────────────
            if !self.error.is_empty() {
                ui.colored_label(egui::Color32::RED, &self.error);
            }
            if let Some(msg) = self.success_message.take() {
                let success_color = if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(100, 220, 100)
                } else {
                    egui::Color32::from_rgb(0, 140, 0)
                };
                ui.colored_label(success_color, &msg);
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
                    // Use a stable ID for the TextEdit so focus/IME state persists across frames
                    let input_id = egui::Id::new("chat_input_textedit");
                    let input_te = egui::TextEdit::multiline(&mut self.input)
                        .id(input_id)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3)
                        .hint_text(i18n.t("chat.input"));
                    let te_resp = ui.add_enabled(!self.sending, input_te);
                    // If nothing else grabbed focus and user has existing text,
                    // request focus so IME stays active
                    if !self.sending
                        && !te_resp.has_focus()
                        && !self.input.is_empty()
                        && !ui.ctx().is_using_pointer()
                    {
                        ui.memory_mut(|m| m.request_focus(input_id));
                    }
                    te_resp
                });
            let input_focus = input_resp.inner.has_focus();

            // Character counter
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} {}",
                        self.input.len(),
                        i18n.t("chat.charCount")
                    ))
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
                                let p_clone = p.clone();
                                let tx = self.pending_tx.clone();
                                std::thread::spawn(move || {
                                    let _ = child.wait();
                                    // Editor closed — read the file and send result back to UI thread
                                    if let Ok(edited) = std::fs::read_to_string(&p_clone) {
                                        let trimmed = edited.trim().to_string();
                                        if !trimmed.is_empty() {
                                            // Use blocking send to guarantee delivery.
                                            // In the rare case the channel is full, retry periodically.
                                            loop {
                                                match tx.send(
                                                    PendingResponse::ExternalEditorResult(
                                                        trimmed.clone(),
                                                    ),
                                                ) {
                                                    Ok(()) => break,
                                                    Err(std::sync::mpsc::SendError(_)) => {
                                                        std::thread::sleep(
                                                            std::time::Duration::from_millis(50),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                });
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
                    if ui
                        .button("⚠")
                        .on_hover_text(i18n.t("chat.riskDecisionTitle"))
                        .clicked()
                    {
                        self.show_risk_decision = !self.show_risk_decision;
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

            // ── Send on Enter / Ctrl+Enter ────────────────
            // On Linux (fcitx5 IME): Ctrl+Enter to send (IME-safe),
            // plain Enter is consumed by IME for composition commit.
            // On all platforms: Shift+Enter inserts a newline.
            #[cfg(target_os = "linux")]
            {
                // Linux: Ctrl+Enter sends; Enter and Shift+Enter insert newline.
                let mut do_send = false;
                ui.input_mut(|i| {
                    if input_focus && i.consume_key(egui::Modifiers::CTRL, egui::Key::Enter) {
                        do_send = true;
                    }
                });
                if do_send {
                    self.send_message(backend, ctx, autotune_chain_enabled);
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                // Non-Linux: Enter sends; Shift+Enter inserts newline.
                if ui.input(|i| {
                    !input_focus || !i.key_pressed(egui::Key::Enter) || i.modifiers.shift
                }) {
                    // Don't send
                } else {
                    self.send_message(backend, ctx, autotune_chain_enabled);
                }
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
                    let was_open = self.show_prompts || self.show_model_picker;
                    self.show_prompts = false;
                    self.show_model_picker = false;
                    if was_open {}
                }
            });
        });
    }

    // ── Sidebar: session list ───────────────────────────────────
    fn show_sidebar(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        backend: &BackendClient,
        ctx: &egui::Context,
    ) {
        egui::Frame::NONE.show(ui, |ui| {
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
                        let exported_at = format_absolute_time(crate::fs_util::epoch_secs());
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
                        if let Some(dirs) =
                            directories::ProjectDirs::from("com", "goon", "go-on-gui")
                        {
                            let config_dir = dirs.config_dir();
                            #[cfg(target_os = "windows")]
                            if let Err(e) = std::process::Command::new("cmd")
                                .args(["/c", "start", "", &config_dir.display().to_string()])
                                .spawn()
                            {
                                eprintln!("Failed to open config directory: {e}");
                            }
                            #[cfg(target_os = "macos")]
                            if let Err(e) =
                                std::process::Command::new("open").arg(config_dir).spawn()
                            {
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
                        // Build the task from conversation history
                        let task = user_msgs.join("\n---\n");
                        let backend_clone = backend.clone();
                        let tx = self.pending_tx.clone();
                        let ctx_clone = ctx.clone();
                        let success_tpl = i18n.t("chat.workflowGenerated").to_string();
                        let failed_tpl = i18n.t("chat.workflowGenError").to_string();
                        tokio::spawn(async move {
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(60),
                                backend_clone.execute_workflow(&task, None, None),
                            )
                            .await
                            {
                                Ok(Ok(value)) => {
                                    let msg = if let Some(id) =
                                        value.get("run_id").and_then(|v| v.as_str())
                                    {
                                        success_tpl.replace("{workflow}", id)
                                    } else {
                                        success_tpl.replace("{workflow}", "OK")
                                    };
                                    if let Err(e) = tx.try_send(PendingResponse::UiMessage(msg)) {
                                        eprintln!("WARN: chat ui try_send failed: {:?}", e);
                                    }
                                }
                                Ok(Err(e)) => {
                                    let msg = failed_tpl.replace("{error}", &e);
                                    if let Err(e) = tx.try_send(PendingResponse::UiMessage(msg)) {
                                        eprintln!("WARN: chat ui try_send failed: {:?}", e);
                                    }
                                }
                                Err(_) => {
                                    let msg = failed_tpl.replace("{error}", "timeout");
                                    if let Err(e) = tx.try_send(PendingResponse::UiMessage(msg)) {
                                        eprintln!("WARN: chat ui try_send failed: {:?}", e);
                                    }
                                }
                            }
                            ctx_clone.request_repaint();
                        });
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
                                    let resp = ui.selectable_label(
                                        selected,
                                        format!(
                                            "\u{200B}{}\u{200B}{}",
                                            idx, &self.sessions[idx].name
                                        ),
                                    );
                                    if resp.double_clicked() {
                                        self.rename_session_idx = Some(idx);
                                        self.rename_session_buf = self.sessions[idx].name.clone();
                                    } else if resp.clicked() {
                                        self.active_session = idx;
                                        self.selected_mode = self.sessions[idx].mode.clone();
                                        self.selected_phase = self.sessions[idx].phase.clone();
                                        self.selected_model = self.sessions[idx].model.clone();
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
                                    self.active_session = self.active_session.saturating_sub(1);
                                } else if self.active_session >= self.sessions.len() {
                                    self.active_session = self.sessions.len().saturating_sub(1);
                                }
                                if self.active_session < self.sessions.len() {
                                    self.selected_mode =
                                        self.sessions[self.active_session].mode.clone();
                                    self.selected_phase =
                                        self.sessions[self.active_session].phase.clone();
                                    self.selected_model =
                                        self.sessions[self.active_session].model.clone();
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
        });
    }

    // ── Messages area ─────────────────────
    fn show_messages(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        let dark = ui.visuals().dark_mode;
        let total_msgs = self.messages().len();
        let start_idx = total_msgs.saturating_sub(Self::MAX_RENDERED_MESSAGES);

        // Theme-aware muted/weak text colors — used throughout for contrast in all themes
        let muted_text = if dark {
            egui::Color32::from_rgb(140, 142, 150)
        } else {
            egui::Color32::from_rgb(110, 112, 120)
        };
        let weak_text = if dark {
            egui::Color32::from_rgb(160, 162, 170)
        } else {
            egui::Color32::from_rgb(130, 132, 140)
        };

        if total_msgs == 0 {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(i18n.t("chat.noMessages"))
                        .color(muted_text)
                        .size(16.0),
                );
                ui.add_space(6.0);
                ui.colored_label(weak_text, i18n.t("chat.hint"));
            });
            return;
        }

        if start_idx > 0 {
            ui.colored_label(
                muted_text,
                i18n.t("chat.showingLatest")
                    .replace("{shown}", &(total_msgs - start_idx).to_string())
                    .replace("{total}", &total_msgs.to_string()),
            );
            ui.add_space(4.0);
        }

        // ── Global thinking toggle (borrow-free check) ──
        {
            let msgs_ref = self.messages();
            let has_thinking = msgs_ref.iter().any(|m| !m.thinking.is_empty());
            if has_thinking {
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(
                            self.show_all_thinking,
                            format!("💭 {}", i18n.t("chat.showAllThinking")),
                        )
                        .clicked()
                    {
                        self.show_all_thinking = !self.show_all_thinking;
                        if !self.show_all_thinking {
                            self.show_thinking_idx = None;
                        }
                    }
                });
                ui.add_space(4.0);
            }
        }

        // Show ALL messages. Avoid full Vec clone — iterate by index directly.
        let dark_mode = ui.visuals().dark_mode;
        let mut last_ts: u64 = 0;
        let mut last_time_str = String::new();

        // ── Dirty-check: skip re-rendering unchanged messages ────────
        // Compare a running hash of each message's content+thinking against
        // the last-rendered hash.  If unchanged AND the message is not
        // the last assistant message (which may still be streaming), skip
        // the full markdown parse + layout to eliminate flicker.
        let msg_count = self.messages().len();
        self.rendered_content_hashes.resize(msg_count, 0);

        // The last assistant message (if sending or the most recent one) is
        // always re-rendered because it may be receiving streaming updates.
        // All earlier messages use hash comparison.
        let last_assistant_idx: Option<usize> = if self.sending {
            // During streaming, the last assistant message is actively updating
            Some(msg_count.saturating_sub(1))
        } else if msg_count > 0 && self.messages()[msg_count - 1].role != "user" {
            Some(msg_count - 1)
        } else {
            None
        };

        for msg_idx in start_idx..msg_count {
            // Compute content hash for dirty check
            {
                let msgs = self.messages();
                if msg_idx < msgs.len() {
                    let m = &msgs[msg_idx];
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    m.content.hash(&mut hasher);
                    m.thinking.hash(&mut hasher);
                    let current_hash = hasher.finish();
                    let prev_hash = self.rendered_content_hashes[msg_idx];
                    let is_last = last_assistant_idx == Some(msg_idx);
                    if current_hash == prev_hash && !is_last {
                        // Content unchanged, render a thin placeholder instead
                        // of re-running the full markdown pipeline.
                        let (is_user, timestamp, model_name) =
                            { (m.role == "user", m.timestamp, m.model.clone()) };
                        Self::render_collapsed_bubble(
                            ui,
                            i18n,
                            is_user,
                            timestamp,
                            &model_name,
                            muted_text,
                            weak_text,
                            dark_mode,
                        );
                        continue;
                    }
                    self.rendered_content_hashes[msg_idx] = current_hash;
                }
            }

            // Full render for changed or last message
            let (is_user, timestamp, model_name, content_text, has_thinking, thinking_text) = {
                let msgs = self.messages();
                if msg_idx >= msgs.len() {
                    continue;
                }
                let m = &msgs[msg_idx];
                (
                    m.role == "user",
                    m.timestamp,
                    m.model.clone(),
                    m.content.clone(),
                    !m.thinking.is_empty() && m.role != "user",
                    m.thinking.clone(),
                )
            };
            // ── Edit mode: show TextEdit instead of bubble ────────
            if self.edit_msg_idx == Some(msg_idx) {
                ui.add_space(4.0);
                let edit_bg = if dark {
                    egui::Color32::from_rgba_premultiplied(80, 60, 10, 50)
                } else {
                    egui::Color32::from_rgba_premultiplied(255, 220, 100, 30)
                };
                egui::Frame::new()
                    .fill(edit_bg)
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

            // Zed-style: subtle colors — user gets a clean accent, AI gets neutral
            let (bubble_color, text_color) = if is_user {
                let bc = if dark_mode {
                    egui::Color32::from_rgb(30, 100, 200)
                } else {
                    egui::Color32::from_rgb(0, 100, 250)
                };
                (bc, egui::Color32::WHITE)
            } else {
                let bc = if dark_mode {
                    egui::Color32::from_rgb(42, 46, 56)
                } else {
                    egui::Color32::from_rgb(235, 237, 241)
                };
                let tc = if dark_mode {
                    egui::Color32::from_rgb(212, 216, 226)
                } else {
                    egui::Color32::from_rgb(30, 32, 38)
                };
                (bc, tc)
            };

            let time_str = if timestamp == last_ts {
                last_time_str.as_str()
            } else {
                last_ts = timestamp;
                last_time_str = format_absolute_time(timestamp);
                last_time_str.as_str()
            };

            // Consecutive same-role grouping: hide avatar/name/timestamp
            let prev_same_role = msg_idx > 0
                && self
                    .messages()
                    .get(msg_idx - 1)
                    .map(|m| (m.role == "user") == is_user)
                    .unwrap_or(false);

            // Zed-style: show avatar + name header (hidden for consecutive same-role)
            let name_label = if is_user {
                i18n.t("chat.you").to_string()
            } else if !model_name.is_empty() {
                model_name.clone()
            } else {
                "AI".to_string()
            };
            if !prev_same_role {
                ui.horizontal(|ui| {
                    Self::draw_role_avatar(ui, is_user);
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(&name_label).strong().size(13.0));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(time_str).color(weak_text).size(10.0));
                });
                ui.add_space(2.0);
            }

            // Bubble content width
            let max_bubble_width = (ui.available_width() - 40.0).clamp(200.0, 800.0);

            let enable_markdown_val = self.enable_markdown;
            // All messages left-aligned — color differentiates user vs AI.
            // Zed-style: no extra avatar indent for consecutive, bubble with min spacing
            let indent = if prev_same_role { 28.0 } else { 0.0 };
            ui.horizontal_top(|ui| {
                ui.add_space(indent);
                ui.vertical(|ui| {
                    ui.set_max_width(max_bubble_width);
                    egui::Frame::new()
                        .fill(bubble_color)
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                        .show(ui, |ui| {
                            ui.set_max_width(max_bubble_width - 20.0);

                            // Inline action bar
                            ui.horizontal(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        if ui
                                            .button("📋")
                                            .on_hover_text(i18n.t("chat.copyMessage"))
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(content_text.clone());
                                        }
                                        if ui
                                            .button("✏️")
                                            .on_hover_text(i18n.t("chat.edit"))
                                            .clicked()
                                        {
                                            self.edit_msg_idx = Some(msg_idx);
                                            self.edit_msg_buf = content_text.clone();
                                        }
                                        if ui
                                            .button("🗑")
                                            .on_hover_text(i18n.t("chat.delete"))
                                            .clicked()
                                        {
                                            self.remove_message_at(msg_idx);
                                            self.save_sessions_to_disk();
                                        }
                                    },
                                );
                            });

                            // ── Content-hash cached markdown rendering ──
                            let hash = {
                                use std::hash::{Hash, Hasher};
                                let mut h = std::collections::hash_map::DefaultHasher::new();
                                content_text.hash(&mut h);
                                h.finish()
                            };
                            let cached = self
                                .rendered_content_hashes
                                .get(msg_idx)
                                .copied()
                                .unwrap_or(0);
                            let content_changed = cached != hash;
                            if msg_idx < self.rendered_content_hashes.len() {
                                self.rendered_content_hashes[msg_idx] = hash;
                            }

                            // ── Streaming cursor: append ▊ to last AI message during streaming ──
                            let is_streaming = self.sending
                                && !is_user
                                && msg_idx == self.messages().len().saturating_sub(1);
                            let display_content = if is_streaming && !content_text.is_empty() {
                                format!("{}▊", content_text)
                            } else {
                                content_text.clone()
                            };

                            if !content_changed && !content_text.is_empty() && !self.sending {
                                // Content unchanged and not streaming — skip markdown parse
                                ui.label(egui::RichText::new(&display_content).color(text_color));
                            } else {
                                let trunc_hint_msg =
                                    i18n.t("chat.largeMessageTruncated").to_string();
                                Self::render_markdown(
                                    ui,
                                    &display_content,
                                    &i18n.t("chat.copyCode"),
                                    enable_markdown_val,
                                    text_color,
                                    &trunc_hint_msg,
                                );
                            }

                            // ── Thinking section ──
                            if has_thinking {
                                ui.add_space(6.0);

                                let is_expanded = self.show_all_thinking
                                    || self.show_thinking_idx == Some(msg_idx);
                                let toggle_icon = if is_expanded { "▼" } else { "▶" };
                                let char_count = thinking_text.chars().count();
                                let trunc_hint = i18n.t("chat.largeMessageTruncated").to_string();

                                // Theme-aware colors for the thinking header
                                let (bar_bg, bar_border, bar_text, accent) = if dark {
                                    (
                                        egui::Color32::from_rgba_premultiplied(80, 60, 20, 50),
                                        egui::Color32::from_rgba_premultiplied(180, 140, 60, 80),
                                        egui::Color32::from_rgb(200, 170, 80),
                                        egui::Color32::from_rgb(220, 180, 70),
                                    )
                                } else {
                                    (
                                        egui::Color32::from_rgba_premultiplied(255, 245, 220, 120),
                                        egui::Color32::from_rgba_premultiplied(200, 170, 100, 100),
                                        egui::Color32::from_rgb(140, 100, 40),
                                        egui::Color32::from_rgb(180, 140, 50),
                                    )
                                };

                                // Header bar: clickable frame with toggle icon, emoji, label, stats
                                let header_id = ui.next_auto_id();
                                let header_rect = {
                                    let mut header_frame = egui::Frame::new()
                                        .fill(bar_bg)
                                        .stroke(egui::Stroke::new(1.0, bar_border))
                                        .corner_radius(6.0);
                                    if is_expanded {
                                        // Flat bottom when expanded so content blends
                                        header_frame =
                                            header_frame.corner_radius(egui::CornerRadius {
                                                nw: 6,
                                                ne: 6,
                                                sw: 0,
                                                se: 0,
                                            });
                                    }
                                    header_frame
                                        .inner_margin(egui::Margin::symmetric(10i8, 6i8))
                                        .show(ui, |ui| {
                                            ui.set_min_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                // Toggle icon + emoji + label
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{} 💭  {}",
                                                        toggle_icon,
                                                        i18n.t("chat.thinkingLabel")
                                                    ))
                                                    .size(12.0)
                                                    .color(bar_text)
                                                    .strong(),
                                                );
                                                // Spacer to push stats to the right
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "{} {} (~{} tokens)",
                                                                char_count,
                                                                i18n.t("chat.charCount"),
                                                                char_count / 4
                                                            ))
                                                            .size(10.0)
                                                            .color(accent),
                                                        );
                                                    },
                                                );
                                            });
                                        })
                                        .response
                                        .rect
                                };

                                // Click detection on the header area
                                let header_clicked = ui
                                    .interact(
                                        header_rect,
                                        header_id.with("think_toggle"),
                                        egui::Sense::click(),
                                    )
                                    .clicked();

                                if header_clicked {
                                    if self.show_all_thinking {
                                        // Clicking one while all are shown: switch to single-message mode
                                        self.show_all_thinking = false;
                                        self.show_thinking_idx = Some(msg_idx);
                                    } else if self.show_thinking_idx == Some(msg_idx) {
                                        self.show_thinking_idx = None;
                                    } else {
                                        self.show_thinking_idx = Some(msg_idx);
                                    }
                                }

                                // Expanded thinking content area
                                if is_expanded {
                                    let content_bg = if dark {
                                        egui::Color32::from_rgba_premultiplied(50, 40, 15, 40)
                                    } else {
                                        egui::Color32::from_rgba_premultiplied(255, 248, 230, 180)
                                    };
                                    let content_border = if dark {
                                        egui::Color32::from_rgba_premultiplied(180, 140, 60, 40)
                                    } else {
                                        egui::Color32::from_rgba_premultiplied(200, 170, 100, 60)
                                    };
                                    egui::Frame::new()
                                        .fill(content_bg)
                                        .stroke(egui::Stroke::new(1.0, content_border))
                                        .corner_radius(egui::CornerRadius {
                                            nw: 0,
                                            ne: 0,
                                            sw: 6,
                                            se: 6,
                                        })
                                        .inner_margin(egui::Margin::symmetric(12i8, 10i8))
                                        .show(ui, |ui| {
                                            Self::render_markdown(
                                                ui,
                                                &thinking_text,
                                                &i18n.t("chat.copyCode"),
                                                enable_markdown_val,
                                                weak_text,
                                                &trunc_hint,
                                            );
                                            ui.horizontal(|ui| {
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if ui
                                                            .button(
                                                                egui::RichText::new(
                                                                    i18n.t("common.copyButton"),
                                                                )
                                                                .size(11.0),
                                                            )
                                                            .clicked()
                                                        {
                                                            ui.ctx()
                                                                .copy_text(thinking_text.clone());
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
                            ui.colored_label(weak_text, i18n.t("chat.thinking"));
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
                            muted_text,
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

    /// Render a thin collapsed bubble for messages whose content has not changed
    /// since the last frame.  Skips the expensive markdown parse + layout pass
    /// to eliminate screen flickering on idle frames.
    #[allow(clippy::too_many_arguments)]
    fn render_collapsed_bubble(
        ui: &mut egui::Ui,
        i18n: &I18n,
        is_user: bool,
        timestamp: u64,
        model_name: &str,
        muted_text: egui::Color32,
        weak_text: egui::Color32,
        dark_mode: bool,
    ) {
        let (bubble_color, text_color) = if is_user {
            let bc = if dark_mode {
                egui::Color32::from_rgb(30, 100, 200)
            } else {
                egui::Color32::from_rgb(0, 100, 250)
            };
            (bc, egui::Color32::WHITE)
        } else {
            let bc = if dark_mode {
                egui::Color32::from_rgb(42, 46, 56)
            } else {
                egui::Color32::from_rgb(235, 237, 241)
            };
            let tc = if dark_mode {
                egui::Color32::from_rgb(212, 216, 226)
            } else {
                egui::Color32::from_rgb(30, 32, 38)
            };
            (bc, tc)
        };

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(if is_user { 60.0 } else { 8.0 });
            Self::draw_role_avatar(ui, is_user);
            ui.add_space(6.0);

            egui::Frame::new()
                .fill(bubble_color)
                .corner_radius(12.0)
                .inner_margin(egui::Margin::symmetric(14i8, 6i8))
                .show(ui, |ui| {
                    // Model name + timestamp line
                    if !model_name.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("🤖 {}", model_name))
                                    .size(11.0)
                                    .color(weak_text),
                            );
                            if timestamp > 0 {
                                let ts = chrono::DateTime::from_timestamp(timestamp as i64, 0)
                                    .map(|dt| dt.format("%H:%M").to_string())
                                    .unwrap_or_default();
                                ui.label(egui::RichText::new(ts).size(10.0).color(muted_text));
                            }
                        });
                        ui.add_space(2.0);
                    }
                    // Placeholder text
                    ui.label(
                        egui::RichText::new(if is_user {
                            i18n.t("chat.userMessagePlaceholder")
                        } else {
                            i18n.t("chat.assistantMessagePlaceholder")
                        })
                        .color(text_color)
                        .size(11.0),
                    );
                });
            ui.add_space(if is_user { 8.0 } else { 60.0 });
        });
        ui.add_space(4.0);
    }
}
