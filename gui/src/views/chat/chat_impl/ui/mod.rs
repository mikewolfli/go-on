//! Chat UI rendering module.
//!
//! This module was extracted from the monolithic `ui.rs` (2,043 lines).
//! Sub-modules have been created to split concerns:
//!
//! - `messages` - message bubble rendering, token stats, avatars
//! - `input` - input area, send button, keyboard shortcuts
//! - `model_picker` - agent/model selection combos
//! - `attachments` - file attachment display and handling
//!
//! ## Current status (as of this writing)
//!
//! The sub-modules contain rendering *helpers* that are called from within
//! the `impl ChatView` blocks defined directly in this file. The old
//! `old_ui_content.rs` has been fully decomposed into sub-modules.
//!
//! ## Migration plan (incremental)
//!
//! Phase 1 (current): Pull pure rendering helpers into sub-modules ✓ DONE
//!   - `messages::draw_role_avatar`, `render_token_stats`, `render_collapsed_bubble`
//!   - `attachments::render_attachments`
//!   - `input::render_send_button`, `render_mode_row`, `handle_input_shortcuts`
//!   - `model_picker::render_model_picker`
//!
//! Phase 2 (done): Extract `show_messages()` into `messages.rs` ✓
//!   - `ChatView::show_messages()` → `messages::show_messages()`
//!
//! Phase 3 (done): Extract `show_sidebar()` into `sidebar.rs` ✓
//!   - `ChatView::show_sidebar()` → `sidebar::show_sidebar()`
//!
//! Phase 4 (done): Extract `show()` and `show_safe_chat_layout()` into `mod.rs` directly ✓
//!   - `include!("old_ui_content.rs")` removed
//!   - `old_ui_content.rs` deleted

pub mod attachments;
pub mod input;
pub mod messages;
pub mod model_picker;
pub mod sidebar;

// Re-export public sub-module items that don't conflict with the legacy content.
// Note: mode_display_key, draw_role_avatar, render_token_stats, render_collapsed_bubble
// are now wired from their respective sub-modules.

use super::*;

impl ChatView {
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
            runtime_config.stream_token_flush_ms,
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
        let is_first_init = !self.template_state.templates_bootstrapped;
        if is_first_init {
            self.bootstrap_default_templates(i18n);
            self.refresh_default_session_names(i18n);
        }
        self.sync_model_selection();

        // Delayed loading: Schedule backend queries after first render to avoid UI freeze
        if !self.phases_load_scheduled && !self.phases_loaded {
            self.phases_load_scheduled = true;
            let backend_clone = backend.clone();
            let tx = self.stream_state.pending_tx.clone();
            let ctx_clone = ctx.clone();

            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                if let Ok(Ok(baseline)) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.config_baseline(),
                )
                .await
                {
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
                } else {
                    #[cfg(debug_assertions)]
                    eprintln!("Warning: Failed to load phases from backend (timeout or error)");
                }
            });
        }

        if !self.model_state.models_loaded
            && self.model_state.last_models_fetch.elapsed() > std::time::Duration::from_secs(3)
        {
            self.model_state.last_models_fetch = std::time::Instant::now();
            let backend_clone = backend.clone();
            let tx = self.stream_state.pending_tx.clone();
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

                        let mut models_list = self.model_state.available_models.clone();
                        if models_list.is_empty() {
                            models_list.push("auto".to_string());
                        }
                        if self
                            .model_state
                            .available_agent_models
                            .contains_key("copilot")
                        {
                            models_list.push(ChatView::COPILOT_AUTO_MODEL.to_string());
                        }

                        egui::ComboBox::from_label("Model")
                            .selected_text(
                                if self.model_state.selected_model == ChatView::COPILOT_AUTO_MODEL {
                                    "copilot/auto".to_string()
                                } else {
                                    self.model_state.selected_model.clone()
                                },
                            )
                            .show_ui(ui, |ui| {
                                for model in &models_list {
                                    let label = if model == ChatView::COPILOT_AUTO_MODEL {
                                        "copilot/auto"
                                    } else {
                                        model.as_str()
                                    };
                                    ui.selectable_value(
                                        &mut self.model_state.selected_model,
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
        if self.show_extra_buttons && self.show_prompts {
            let has_collection = !self.template_state.prompt_collection.is_empty();
            let has_custom = !self.template_state.prompt_templates.is_empty();
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
                                    egui::TextEdit::singleline(&mut self.template_state.template_search_query)
                                        .hint_text(i18n.t("chat.searchTemplates"))
                                        .desired_width(260.0),
                                );
                                if has_custom
                                    && ui
                                        .button("✚")
                                        .on_hover_text(i18n.t("chat.templateNew"))
                                        .clicked()
                                {
                                    self.template_state.selected_template_idx = None;
                                    self.template_state.template_name_buf.clear();
                                    self.template_state.template_command_buf.clear();
                                    self.template_state.template_content_buf.clear();
                                }
                            });
                            ui.separator();

                            let query = self.template_state.template_search_query.to_ascii_lowercase();

                        if has_collection {
                            // ── Two-column: categories | templates ──
                            ui.columns(2, |cols| {
                                // Left: category list
                                egui::ScrollArea::vertical()
                                    .id_salt("prompt_cat_list")
                                    .max_height(320.0)
                                    .show(&mut cols[0], |ui| {
                                        for cat in &self.template_state.prompt_collection {
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
                                            let is_sel = self.template_state.prompt_selected_category
                                                .as_deref() == Some(cat_id_str);
                                            let label = format!("{}  {} ({})", cat.icon, cat.name, visible_count);
                                            if ui.selectable_label(is_sel, &label).clicked() {
                                                self.template_state.prompt_selected_category = Some(cat_id_str.clone());
                                            }
                                        }
                                    });

                                // Right: template list for selected category
                                egui::ScrollArea::vertical()
                                    .id_salt("prompt_tpl_list")
                                    .max_height(320.0)
                                    .show(&mut cols[1], |ui| {
                                        let mut inserted = false;
                                        if let Some(ref sel_id) = self.template_state.prompt_selected_category {
                                            if let Some(cat) = self.template_state.prompt_collection.iter().find(|c| &c.id == sel_id) {
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
                                        if !inserted && query.is_empty() && self.template_state.prompt_selected_category.is_none() {
                                            // Auto-select first non-empty category
                                            if let Some(first) = self.template_state.prompt_collection.first() {
                                                self.template_state.prompt_selected_category = Some(first.id.clone());
                                            }
                                        }
                                    });
                            });
                        } else if has_custom {
                            // Only custom templates — show simple list
                            let mut pick_idx = None;
                            for (idx, tpl) in self.template_state.prompt_templates.iter().enumerate() {
                                if ui.selectable_label(
                                    self.template_state.selected_template_idx == Some(idx),
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
                            ui.text_edit_singleline(&mut self.template_state.template_name_buf);
                            ui.label(i18n.t("chat.templateCommand"));
                            ui.text_edit_singleline(&mut self.template_state.template_command_buf);
                            ui.label(i18n.t("chat.templateBody"));
                            ui.add(
                                egui::TextEdit::multiline(&mut self.template_state.template_content_buf)
                                    .desired_rows(8)
                                    .desired_width(ui.available_width()),
                            );
                            ui.horizontal(|ui| {
                                if ui.button(i18n.t("chat.templateInsert")).clicked() {
                                    self.input = self.template_state.template_content_buf.clone();
                                    self.show_prompts = false;
                                }
                                if ui.button(i18n.t("chat.templateSave")).clicked() {
                                    let name = self.template_state.template_name_buf.trim().to_string();
                                    let cmd = Self::normalize_command(&self.template_state.template_command_buf);
                                    let content = self.template_state.template_content_buf.trim().to_string();
                                    if !name.is_empty() && !cmd.is_empty() && !content.is_empty() {
                                        self.template_state.prompt_templates.push(PromptTemplate {
                                            id: format!("tpl_{}", self.template_state.next_template_id),
                                            name, command: cmd, content,
                                        });
                                        self.template_state.next_template_id += 1;
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

        // Wire: only show risk decision window when mode policy permits risk display
        if self.show_extra_buttons && self.mode_policy.show_risk_display && self.show_risk_decision
        {
            egui::Window::new(i18n.t("chat.riskDecisionTitle"))
                .id(egui::Id::new("chat_risk_decision_window"))
                .resizable(true)
                .default_width(380.0)
                .show(ctx, |ui| {
                    ui.label(i18n.t("chat.riskDecisionLabel"));
                    ui.horizontal(|ui| {
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
                    ui.label(i18n.t("chat.riskDecisionReview"));
                    ui.horizontal(|ui| {
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
        egui::Panel::left("chat_sidebar_panel")
            .default_size(220.0)
            .resizable(true)
            .min_size(140.0)
            .max_size(400.0)
            .show(ui, |ui| {
                sidebar::show_sidebar(self, ui, i18n, backend, ctx);
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
            if self.show_mode_row {
                input::render_mode_row(self, ui, i18n);
            }

            // ── Messages area ─
            egui::ScrollArea::vertical()
                .id_salt("chat_messages_scroll")
                .auto_shrink([false; 2])
                .max_height(msg_h)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    messages::show_messages(self, ui, i18n);
                });

            ui.separator();

            // ── Input area (bottom, fixed) ────────────
            if !self.error.is_empty() {
                ui.colored_label(egui::Color32::RED, &self.error);
            }

            // ── Tool approval buttons (shown when sandbox denies or mode requires approval) ─
            // Gate: only show approval UI when mode policy enables tool approval
            if self.mode_policy.show_tool_approval {
                if let Some((ref tool_name, ref risk_score, ref last_msg_idx)) = self.pending_tool_approval.clone() {
                ui.horizontal(|ui| {
                    // Show mode context
                    let mode_hint = match self.mode_policy.mode.as_str() {
                        "safeguard" => "🛡️ SafeGuard",
                        "edit" => "✏️ Edit",
                        _ => "🧰",
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{} Tool '{}' requires approval:",
                            mode_hint, tool_name
                        ))
                        .size(12.0)
                        .strong(),
                    );

                    // Wire: show risk score from backend when mode policy permits risk display
                    if self.mode_policy.show_risk_display {
                        let risk_label = if *risk_score > 0.7 {
                            format!(" ⚠️ High risk ({:.0}%)", risk_score * 100.0)
                        } else if *risk_score > 0.4 {
                            format!(" ⚡ Medium risk ({:.0}%)", risk_score * 100.0)
                        } else {
                            format!(" ✅ Low risk ({:.0}%)", risk_score * 100.0)
                        };
                        ui.colored_label(
                            if *risk_score > 0.7 {
                                egui::Color32::from_rgb(220, 100, 60)
                            } else {
                                egui::Color32::from_rgb(180, 180, 100)
                            },
                            risk_label,
                        );
                    }

                    let approve_btn =
                        egui::Button::new(egui::RichText::new("✅ Approve").size(13.0))
                            .fill(egui::Color32::from_rgb(40, 160, 40));

                    let deny_btn = egui::Button::new(egui::RichText::new("❌ Deny").size(13.0))
                        .fill(egui::Color32::from_rgb(180, 40, 40));

                    if ui.add(approve_btn).clicked() {
                        let tool = tool_name.clone();
                        let b = backend.clone();
                        let msg_idx = *last_msg_idx;
                        let input_text = self
                            .session_state
                            .sessions
                            .get(self.session_state.active_session)
                            .and_then(|s| s.messages.get(msg_idx))
                            .map(|m| m.content.clone())
                            .unwrap_or_default();

                        self.pending_tool_approval = None;
                        self.error.clear();

                        // Spawn async task to approve tool on the backend
                        tokio::spawn(async move {
                            let _ = b.approve_tool(&tool).await;
                        });

                        // Re-send the user's last message with the approved tool
                        if !input_text.is_empty() {
                            self.input = input_text;
                            self.send_message(backend, ctx, autotune_chain_enabled);
                        }
                    }

                    if ui.add(deny_btn).clicked() {
                        self.pending_tool_approval = None;
                        self.error = format!("Tool '{}' was denied by the user.", tool_name);
                    }
                }); // ui.horizontal
                } // if let Some(tool_approval)
            } else if self.pending_tool_approval.is_some() {
                // Tool requires approval but mode policy disables approval UI —
                // show a static message instead of interactive buttons.
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "⚠️ Tool requires approval, but this mode does not support interactive approval.",
                );
            }
            if let Some(msg) = self.success_message.take() {
                let success_color = if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(100, 220, 100)
                } else {
                    egui::Color32::from_rgb(0, 140, 0)
                };
                ui.colored_label(success_color, &msg);
            }

            messages::render_token_stats(self, ui, i18n);

            attachments::render_attachments(self, ui);

            // ── Paste event handling ────────────
            let pasted_atts = self.handle_paste_events(ui);
            if !pasted_atts.is_empty() {
                self.attachments.extend(pasted_atts);
                ctx.request_repaint_after(self.stream_state.stream_repaint_interval);
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
                        && !ui.ctx().egui_is_using_pointer()
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
                if self.show_extra_buttons {
                    if ui
                        .button("📎")
                        .on_hover_text(i18n.t("chat.attachFile"))
                        .clicked()
                    {
                        attachments::handle_attach_button(self, ui);
                    }
                    if ui
                        .button("✏️")
                        .on_hover_text(i18n.t("chat.externalEditor"))
                        .clicked()
                    {
                        attachments::handle_external_editor(self, ui);
                    }
                    if ui
                        .button("💡")
                        .on_hover_text(i18n.t("chat.promptTemplates"))
                        .clicked()
                    {
                        self.show_prompts = !self.show_prompts;
                    }
                    // Wire: only show risk decision button when mode policy permits risk display
                    if self.mode_policy.show_risk_display
                        && ui
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
                input::render_send_button(self, ui, i18n, backend, ctx, autotune_chain_enabled);
            });

            input::handle_input_shortcuts(
                self,
                ui,
                input_focus,
                i18n,
                backend,
                ctx,
                autotune_chain_enabled,
            );
        });
    }
}
