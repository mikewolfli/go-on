//! Message rendering sub-module for the Chat UI.
//!
//! Handles rendering of message bubbles, thinking sections, token stats,
//! role avatars, and the collapsed bubble cache.

use super::super::*;
use crate::views::chat::types::MessageSegment;
use std::hash::{Hash, Hasher};

const MAX_RENDERED_MESSAGES: usize = 250;

/// Draw a colored circle avatar with the role initial letter.
/// User gets a blue circle with "U", AI gets a green circle with "A".
pub fn draw_role_avatar(ui: &mut egui::Ui, is_user: bool) {
    let size = 28.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter();
    let dark = ui.visuals().dark_mode;
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

/// Render token statistics line for the current model.
pub fn render_token_stats(chat: &mut ChatView, ui: &mut egui::Ui, i18n: &I18n) {
    if !chat.show_token_details || chat.model_state.model_stats.is_empty() {
        return;
    }

    let Some(stats) = chat
        .model_state
        .model_stats
        .get(&chat.model_state.selected_model)
    else {
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

/// Render a thin collapsed bubble for unchanged messages (avoids expensive markdown re-render).
/// Shows a one-line preview of the actual content instead of a generic placeholder.
#[allow(clippy::too_many_arguments)]
pub fn render_collapsed_bubble(
    ui: &mut egui::Ui,
    i18n: &I18n,
    is_user: bool,
    content: &str,
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
        draw_role_avatar(ui, is_user);
        ui.add_space(6.0);

        // Constrain the bubble to remaining horizontal width so text wraps properly
        let bubble_max_w = ui.available_width().max(100.0);
        egui::Frame::new()
            .fill(bubble_color)
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(14i8, 6i8))
            .show(ui, |ui| {
                ui.set_max_width(bubble_max_w - 28.0);
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
                let display_text: std::borrow::Cow<'_, str> = {
                    let trimmed = content.trim();
                    if trimmed.is_empty() {
                        // Fall back to placeholder when there's no content to show
                        if is_user {
                            i18n.t("chat.userMessagePlaceholder")
                        } else {
                            i18n.t("chat.assistantMessagePlaceholder")
                        }
                    } else {
                        trimmed.into()
                    }
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(display_text)
                            .color(text_color)
                            .size(11.0),
                    )
                    .wrap(),
                );
            });
        ui.add_space(if is_user { 8.0 } else { 60.0 });
    });
    ui.add_space(4.0);
}

/// Render all messages from the current session, with full bubble rendering,
/// streaming cursor, edit mode, collapsed/cached bubbles, thinking/sub-agent/command panels,
/// and progress indicators.
pub fn show_messages(chat: &mut ChatView, ui: &mut egui::Ui, i18n: &I18n) {
    let dark = ui.visuals().dark_mode;
    let total_msgs = chat.messages().len();
    let start_idx = total_msgs.saturating_sub(MAX_RENDERED_MESSAGES);

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
        let msgs_ref = chat.messages();
        let has_thinking = msgs_ref.iter().any(|m| !m.thinking.is_empty());
        if has_thinking {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        chat.show_all_thinking,
                        format!("💭 {}", i18n.t("chat.showAllThinking")),
                    )
                    .clicked()
                {
                    chat.show_all_thinking = !chat.show_all_thinking;
                    if !chat.show_all_thinking {
                        chat.show_thinking_idx = None;
                    }
                }
            });
            ui.add_space(4.0);
        }
    }

    // ── Global sub-agent toggle ──
    {
        let msgs_ref = chat.messages();
        let has_sub = msgs_ref.iter().any(|m| !m.sub_agent_records.is_empty());
        if has_sub {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        chat.show_all_sub_agents,
                        format!("🤖 {}", "Show/Hide all sub-agents"),
                    )
                    .clicked()
                {
                    chat.show_all_sub_agents = !chat.show_all_sub_agents;
                    if !chat.show_all_sub_agents {
                        chat.show_sub_agent_idx = None;
                    }
                }
            });
            ui.add_space(2.0);
        }
    }

    // ── Global command toggle ──
    {
        let msgs_ref = chat.messages();
        let has_cmd = msgs_ref.iter().any(|m| !m.command_records.is_empty());
        if has_cmd {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        chat.show_all_commands,
                        format!("⌨️ {}", "Show/Hide all commands"),
                    )
                    .clicked()
                {
                    chat.show_all_commands = !chat.show_all_commands;
                    if !chat.show_all_commands {
                        chat.show_command_idx = None;
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
    let msg_count = chat.messages().len();
    chat.rendered_content_hashes.resize(msg_count, 0);

    // The last assistant message (if sending or the most recent one) is
    // always re-rendered because it may be receiving streaming updates.
    // All earlier messages use hash comparison.
    let last_assistant_idx: Option<usize> = if chat.sending {
        // During streaming, the last assistant message is actively updating
        Some(msg_count.saturating_sub(1))
    } else if msg_count > 0 && chat.messages()[msg_count - 1].role != "user" {
        Some(msg_count - 1)
    } else {
        None
    };

    for msg_idx in start_idx..msg_count {
        let is_last = last_assistant_idx == Some(msg_idx);

        // For the last (streaming) message, skip hash computation
        // and always re-render — it changes continuously.
        if !is_last {
            // Compute content hash for dirty check
            {
                let msgs = chat.messages();
                if msg_idx < msgs.len() {
                    let m = &msgs[msg_idx];
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    m.content.hash(&mut hasher);
                    m.thinking.hash(&mut hasher);
                    m.sub_agent_records.len().hash(&mut hasher);
                    m.command_records.len().hash(&mut hasher);
                    let current_hash = hasher.finish();
                    let prev_hash = chat.rendered_content_hashes[msg_idx];
                    if current_hash == prev_hash {
                        // Content unchanged, render a thin placeholder instead
                        // of re-running the full markdown pipeline.
                        let (is_user, content, timestamp, model_name) = {
                            (
                                m.role == "user",
                                m.content.clone(),
                                m.timestamp,
                                m.model.clone(),
                            )
                        };
                        render_collapsed_bubble(
                            ui,
                            i18n,
                            is_user,
                            &content,
                            timestamp,
                            &model_name,
                            muted_text,
                            weak_text,
                            dark_mode,
                        );
                        // ── Hover action bar for collapsed bubbles ──
                        // Provide copy/edit/delete so cached messages
                        // aren't stuck without interaction affordances.
                        let bubble_rect = ui.min_rect();
                        let hovered = ui.rect_contains_pointer(bubble_rect);
                        if hovered {
                            let action_color = if dark_mode {
                                egui::Color32::from_rgb(130, 135, 145)
                            } else {
                                egui::Color32::from_rgb(110, 115, 125)
                            };
                            ui.horizontal(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(format!(
                                                        "📋 {}",
                                                        i18n.t("chat.copy")
                                                    ))
                                                    .size(11.0)
                                                    .color(action_color),
                                                )
                                                .frame(false)
                                                .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .on_hover_text(i18n.t("chat.copyMessage"))
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(content.clone());
                                        }
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(format!(
                                                        "✏️ {}",
                                                        i18n.t("chat.edit")
                                                    ))
                                                    .size(11.0)
                                                    .color(action_color),
                                                )
                                                .frame(false)
                                                .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .on_hover_text(i18n.t("chat.edit"))
                                            .clicked()
                                        {
                                            chat.edit_msg_idx = Some(msg_idx);
                                            chat.edit_msg_buf = content.clone();
                                        }
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(format!(
                                                        "🗑 {}",
                                                        i18n.t("chat.delete")
                                                    ))
                                                    .size(11.0)
                                                    .color(action_color),
                                                )
                                                .frame(false)
                                                .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .on_hover_text(i18n.t("chat.delete"))
                                            .clicked()
                                        {
                                            chat.remove_message_at(msg_idx);
                                            chat.save_sessions_to_disk();
                                        }
                                    },
                                );
                            });
                        }
                        continue;
                    }
                    chat.rendered_content_hashes[msg_idx] = current_hash;
                }
            }
        }

        // Full render for changed or last message
        #[allow(clippy::type_complexity)]
        let (
            is_user,
            timestamp,
            model_name,
            content_text,
            _has_thinking,
            _thinking_text,
            sub_agent_records,
            command_records,
            segments,
        ) = {
            let msgs = chat.messages();
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
                m.sub_agent_records.clone(),
                m.command_records.clone(),
                m.segments.clone(),
            )
        };
        // ── Edit mode: show TextEdit instead of bubble ────────
        if chat.edit_msg_idx == Some(msg_idx) {
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
                        egui::TextEdit::multiline(&mut chat.edit_msg_buf)
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
                            let new_content = chat.edit_msg_buf.trim().to_string();
                            if !new_content.is_empty() {
                                if let Some(session) = chat
                                    .session_state
                                    .sessions
                                    .get_mut(chat.session_state.active_session)
                                {
                                    if msg_idx < session.messages.len() {
                                        session.messages[msg_idx].content = new_content;
                                        chat.save_sessions_to_disk();
                                    }
                                }
                            }
                            chat.edit_msg_idx = None;
                            chat.edit_msg_buf.clear();
                        }
                        if ui
                            .button(format!("✕ {}", i18n.t("chat.cancelEdit")))
                            .clicked()
                        {
                            chat.edit_msg_idx = None;
                            chat.edit_msg_buf.clear();
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
            && chat
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
                draw_role_avatar(ui, is_user);
                ui.add_space(8.0);
                ui.label(egui::RichText::new(&name_label).strong().size(13.0));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(time_str).color(weak_text).size(10.0));
            });
            ui.add_space(2.0);
        }

        // Bubble content width — no upper cap so text fills available space
        let max_bubble_width = (ui.available_width() - 40.0).max(200.0);

        let enable_markdown_val = chat.enable_markdown;
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

                        // ── Content-hash cached markdown rendering ──
                        // Reuse the hash computed in the dirty-check above
                        // (already stored in rendered_content_hashes[msg_idx])
                        let _content_changed = msg_idx < chat.rendered_content_hashes.len()
                            && chat.rendered_content_hashes[msg_idx] != 0;

                        // ── Streaming cursor: append ▊ to last AI message during streaming ──
                        let is_streaming = chat.sending
                            && !is_user
                            && msg_idx == chat.messages().len().saturating_sub(1);
                        // ── Zed-style interleaved segment rendering ──
                        // Render segments in chronological order: thinking (colored) and content (normal)
                        // appear interleaved as they were produced during streaming.
                        if !segments.is_empty() {
                            let trunc_hint = i18n.t("chat.largeMessageTruncated").to_string();
                            let (think_bg, think_border, think_text) = if dark {
                                (
                                    egui::Color32::from_rgba_premultiplied(50, 60, 80, 60),
                                    egui::Color32::from_rgba_premultiplied(80, 120, 180, 50),
                                    egui::Color32::from_rgb(160, 180, 210),
                                )
                            } else {
                                (
                                    egui::Color32::from_rgba_premultiplied(235, 242, 255, 180),
                                    egui::Color32::from_rgba_premultiplied(160, 190, 230, 100),
                                    egui::Color32::from_rgb(70, 100, 140),
                                )
                            };

                            // Zed-style interleaved rendering: render segments in chronological
                            // order as they were produced during streaming. Thinking segments
                            // get a colored background box, content segments render as regular markdown.
                            let total_segs = segments.len();
                            for (seg_idx, seg) in segments.iter().enumerate() {
                                let is_last_seg = seg_idx == total_segs - 1;
                                ui.add_space(4.0);
                                match seg {
                                    MessageSegment::Thinking(text) => {
                                        let thinking_visible = chat.show_all_thinking
                                            || chat.show_thinking_idx == Some(msg_idx);
                                        if !thinking_visible {
                                            // Zed-style collapsed thinking indicator
                                            ui.horizontal(|ui| {
                                                let label = ui.selectable_label(
                                                    false,
                                                    egui::RichText::new("💭 See thinking")
                                                        .size(10.0)
                                                        .color(weak_text),
                                                );
                                                if label.clicked() {
                                                    chat.show_thinking_idx = Some(msg_idx);
                                                }
                                            });
                                            continue;
                                        }
                                        egui::Frame::new()
                                            .fill(think_bg)
                                            .stroke(egui::Stroke::new(1.0, think_border))
                                            .corner_radius(6.0)
                                            .inner_margin(egui::Margin::symmetric(12i8, 10i8))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label(egui::RichText::new("💭 ").size(12.0));
                                                    ui.label(
                                                        egui::RichText::new(
                                                            i18n.t("chat.thinkingLabel"),
                                                        )
                                                        .size(11.0)
                                                        .color(think_text)
                                                        .strong(),
                                                    );
                                                });
                                                ui.add_space(4.0);
                                                ChatView::render_markdown(
                                                    ui,
                                                    text,
                                                    &i18n.t("chat.copyCode"),
                                                    enable_markdown_val,
                                                    think_text,
                                                    &trunc_hint,
                                                );
                                            });
                                    }
                                    MessageSegment::Content(text) => {
                                        let display_text = if is_streaming && is_last_seg {
                                            format!("{}▊", text)
                                        } else {
                                            text.clone()
                                        };
                                        ChatView::render_markdown(
                                            ui,
                                            &display_text,
                                            &i18n.t("chat.copyCode"),
                                            enable_markdown_val,
                                            text_color,
                                            &trunc_hint,
                                        );
                                    }
                                }
                            }
                        } else {
                            // Legacy fallback: render content only (no segments)
                            let display_content = if is_streaming && !content_text.is_empty() {
                                format!("{}▊", content_text)
                            } else {
                                content_text.clone()
                            };
                            ChatView::render_markdown(
                                ui,
                                &display_content,
                                &i18n.t("chat.copyCode"),
                                enable_markdown_val,
                                text_color,
                                &i18n.t("chat.largeMessageTruncated"),
                            );
                        }

                        // ── Action bar (Zed/Copilot-style, hover-only) ──
                        let bubble_rect = ui.min_rect();
                        let hovered = ui.rect_contains_pointer(bubble_rect);
                        if hovered {
                            ui.add_space(4.0);
                            let action_color = if dark_mode {
                                egui::Color32::from_rgb(130, 135, 145)
                            } else {
                                egui::Color32::from_rgb(110, 115, 125)
                            };
                            ui.horizontal(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        let copy_label = format!("📋 {}", i18n.t("chat.copy"));
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(&copy_label)
                                                        .size(11.0)
                                                        .color(action_color),
                                                )
                                                .frame(false)
                                                .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .on_hover_text(i18n.t("chat.copyMessage"))
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(content_text.clone());
                                        }
                                        let edit_label = format!("✏️ {}", i18n.t("chat.edit"));
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(&edit_label)
                                                        .size(11.0)
                                                        .color(action_color),
                                                )
                                                .frame(false)
                                                .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .on_hover_text(i18n.t("chat.edit"))
                                            .clicked()
                                        {
                                            chat.edit_msg_idx = Some(msg_idx);
                                            chat.edit_msg_buf = content_text.clone();
                                        }
                                        let del_label = format!("🗑 {}", i18n.t("chat.delete"));
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(&del_label)
                                                        .size(11.0)
                                                        .color(action_color),
                                                )
                                                .frame(false)
                                                .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .on_hover_text(i18n.t("chat.delete"))
                                            .clicked()
                                        {
                                            chat.remove_message_at(msg_idx);
                                            chat.save_sessions_to_disk();
                                        }
                                    },
                                );
                            });
                        }

                        // ── Sub-agent records panel ──
                        let has_sub_agents = !sub_agent_records.is_empty();
                        if has_sub_agents {
                            ui.add_space(6.0);

                            // Wire: always expand sub-agent panels when mode policy says so
                            let is_expanded = chat.mode_policy.expand_sub_agents
                                || chat.show_all_sub_agents
                                || chat.show_sub_agent_idx == Some(msg_idx);
                            let toggle_icon = if is_expanded { "▼" } else { "▶" };
                            let sub_count = sub_agent_records.len();

                            let (bar_bg, bar_border, bar_text, accent) = if dark {
                                (
                                    egui::Color32::from_rgba_premultiplied(30, 70, 80, 50),
                                    egui::Color32::from_rgba_premultiplied(60, 140, 160, 80),
                                    egui::Color32::from_rgb(80, 200, 210),
                                    egui::Color32::from_rgb(60, 190, 200),
                                )
                            } else {
                                (
                                    egui::Color32::from_rgba_premultiplied(220, 245, 255, 120),
                                    egui::Color32::from_rgba_premultiplied(100, 180, 200, 100),
                                    egui::Color32::from_rgb(0, 120, 150),
                                    egui::Color32::from_rgb(0, 140, 170),
                                )
                            };

                            // Header bar
                            let header_id = ui.next_auto_id();
                            let header_rect = {
                                let mut header_frame = egui::Frame::new()
                                    .fill(bar_bg)
                                    .stroke(egui::Stroke::new(1.0, bar_border))
                                    .corner_radius(6.0);
                                if is_expanded {
                                    header_frame = header_frame.corner_radius(egui::CornerRadius {
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
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} 🤖  Sub-agents ({})",
                                                    toggle_icon, sub_count
                                                ))
                                                .size(12.0)
                                                .color(bar_text)
                                                .strong(),
                                            );
                                        });
                                    })
                                    .response
                                    .rect
                            };

                            let header_clicked = ui
                                .interact(
                                    header_rect,
                                    header_id.with("sub_toggle"),
                                    egui::Sense::click(),
                                )
                                .clicked();

                            if header_clicked {
                                if chat.show_all_sub_agents {
                                    chat.show_all_sub_agents = false;
                                    chat.show_sub_agent_idx = Some(msg_idx);
                                } else if chat.show_sub_agent_idx == Some(msg_idx) {
                                    chat.show_sub_agent_idx = None;
                                } else {
                                    chat.show_sub_agent_idx = Some(msg_idx);
                                }
                            }

                            // Expanded content
                            if is_expanded {
                                let content_bg = if dark {
                                    egui::Color32::from_rgba_premultiplied(20, 60, 70, 40)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(230, 248, 255, 180)
                                };
                                let content_border = if dark {
                                    egui::Color32::from_rgba_premultiplied(60, 140, 160, 40)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(100, 180, 200, 60)
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
                                        for (rec_idx, rec) in sub_agent_records.iter().enumerate() {
                                            // Agent header with status badge
                                            let status_color = match rec.status.as_str() {
                                                "completed" => egui::Color32::from_rgb(80, 200, 80),
                                                "failed" => egui::Color32::from_rgb(220, 60, 60),
                                                _ => egui::Color32::from_rgb(220, 200, 60),
                                            };
                                            let status_icon = match rec.status.as_str() {
                                                "completed" => "🟢",
                                                "failed" => "🔴",
                                                _ => "🟡",
                                            };
                                            let duration_str = if rec.duration_ms > 0 {
                                                format!(" · {}ms", rec.duration_ms)
                                            } else {
                                                String::new()
                                            };

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{}. {} {}",
                                                        rec_idx + 1,
                                                        rec.agent_name,
                                                        rec.action
                                                    ))
                                                    .size(11.0)
                                                    .color(bar_text)
                                                    .strong(),
                                                );
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "{} {}{}",
                                                                status_icon,
                                                                rec.status,
                                                                duration_str
                                                            ))
                                                            .size(10.0)
                                                            .color(status_color),
                                                        );
                                                    },
                                                );
                                            });

                                            // Input text
                                            if !rec.input.is_empty() {
                                                ui.add_space(4.0);
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Input: {}",
                                                        rec.input
                                                    ))
                                                    .size(10.0)
                                                    .color(weak_text),
                                                );
                                            }

                                            // Tool calls
                                            if !rec.tool_calls.is_empty() {
                                                ui.add_space(4.0);
                                                ui.label(
                                                    egui::RichText::new("Tool calls:")
                                                        .size(10.0)
                                                        .strong(),
                                                );
                                                for (tc_idx, tc) in
                                                    rec.tool_calls.iter().enumerate()
                                                {
                                                    ui.add_space(2.0);
                                                    egui::Frame::new()
                                                        .fill(if dark {
                                                            egui::Color32::from_rgba_premultiplied(
                                                                40, 40, 50, 60,
                                                            )
                                                        } else {
                                                            egui::Color32::from_rgba_premultiplied(
                                                                240, 240, 245, 180,
                                                            )
                                                        })
                                                        .corner_radius(4.0)
                                                        .inner_margin(egui::Margin::symmetric(
                                                            8i8, 6i8,
                                                        ))
                                                        .show(ui, |ui| {
                                                            ui.label(
                                                                egui::RichText::new(format!(
                                                                    "{}· {} ({}ms)",
                                                                    tc_idx + 1,
                                                                    tc.tool_name,
                                                                    tc.duration_ms
                                                                ))
                                                                .size(10.0)
                                                                .color(accent)
                                                                .strong(),
                                                            );
                                                            if !tc.arguments.is_empty()
                                                                && tc.arguments != "{}"
                                                            {
                                                                ui.label(
                                                                    egui::RichText::new(format!(
                                                                        "  args: {}",
                                                                        tc.arguments
                                                                    ))
                                                                    .size(9.0)
                                                                    .color(weak_text),
                                                                );
                                                            }
                                                            if !tc.result.is_empty() {
                                                                let max_preview: String = tc
                                                                    .result
                                                                    .chars()
                                                                    .take(200)
                                                                    .collect();
                                                                ui.label(
                                                                    egui::RichText::new(format!(
                                                                        "  → {}",
                                                                        max_preview
                                                                    ))
                                                                    .size(9.0)
                                                                    .color(weak_text),
                                                                );
                                                            }
                                                        });
                                                }
                                            }

                                            // Output text
                                            if !rec.output.is_empty() {
                                                ui.add_space(4.0);
                                                let max_out: String =
                                                    rec.output.chars().take(300).collect();
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Output: {}",
                                                        max_out
                                                    ))
                                                    .size(10.0)
                                                    .color(weak_text),
                                                );
                                            }

                                            if rec_idx + 1 < sub_agent_records.len() {
                                                ui.separator();
                                            }
                                        }
                                    });
                            }
                        }

                        // ── Command records panel ──
                        let has_commands = !command_records.is_empty();
                        if has_commands {
                            ui.add_space(6.0);

                            let is_expanded =
                                chat.show_all_commands || chat.show_command_idx == Some(msg_idx);
                            let toggle_icon = if is_expanded { "▼" } else { "▶" };
                            let cmd_count = command_records.len();

                            let (bar_bg, bar_border, bar_text, _accent) = if dark {
                                (
                                    egui::Color32::from_rgba_premultiplied(50, 40, 80, 50),
                                    egui::Color32::from_rgba_premultiplied(120, 100, 180, 80),
                                    egui::Color32::from_rgb(160, 140, 220),
                                    egui::Color32::from_rgb(140, 120, 210),
                                )
                            } else {
                                (
                                    egui::Color32::from_rgba_premultiplied(240, 235, 255, 120),
                                    egui::Color32::from_rgba_premultiplied(160, 140, 200, 100),
                                    egui::Color32::from_rgb(80, 60, 150),
                                    egui::Color32::from_rgb(90, 70, 160),
                                )
                            };

                            // Header bar
                            let header_id = ui.next_auto_id();
                            let header_rect = {
                                let mut header_frame = egui::Frame::new()
                                    .fill(bar_bg)
                                    .stroke(egui::Stroke::new(1.0, bar_border))
                                    .corner_radius(6.0);
                                if is_expanded {
                                    header_frame = header_frame.corner_radius(egui::CornerRadius {
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
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} ⌨️  Commands ({})",
                                                    toggle_icon, cmd_count
                                                ))
                                                .size(12.0)
                                                .color(bar_text)
                                                .strong(),
                                            );
                                        });
                                    })
                                    .response
                                    .rect
                            };

                            let header_clicked = ui
                                .interact(
                                    header_rect,
                                    header_id.with("cmd_toggle"),
                                    egui::Sense::click(),
                                )
                                .clicked();

                            if header_clicked {
                                if chat.show_all_commands {
                                    chat.show_all_commands = false;
                                    chat.show_command_idx = Some(msg_idx);
                                } else if chat.show_command_idx == Some(msg_idx) {
                                    chat.show_command_idx = None;
                                } else {
                                    chat.show_command_idx = Some(msg_idx);
                                }
                            }

                            // Expanded content
                            if is_expanded {
                                let content_bg = if dark {
                                    egui::Color32::from_rgba_premultiplied(30, 25, 55, 40)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(245, 240, 255, 180)
                                };
                                let content_border = if dark {
                                    egui::Color32::from_rgba_premultiplied(120, 100, 180, 40)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(160, 140, 200, 60)
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
                                        for (cmd_idx, cmd) in command_records.iter().enumerate() {
                                            // Command line header
                                            let exit_icon =
                                                if cmd.exit_code == 0 { "✅" } else { "❌" };
                                            let exit_color = if cmd.exit_code == 0 {
                                                egui::Color32::from_rgb(80, 200, 80)
                                            } else {
                                                egui::Color32::from_rgb(220, 60, 60)
                                            };

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{}  {}",
                                                        exit_icon, cmd.command
                                                    ))
                                                    .size(11.0)
                                                    .color(bar_text)
                                                    .strong(),
                                                );
                                            });

                                            // Working directory
                                            if !cmd.working_dir.is_empty() {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "  dir: {}",
                                                        cmd.working_dir
                                                    ))
                                                    .size(9.0)
                                                    .color(weak_text),
                                                );
                                            }

                                            // Exit code + duration
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Exit: {}  ·  {}ms",
                                                        cmd.exit_code, cmd.duration_ms
                                                    ))
                                                    .size(10.0)
                                                    .color(exit_color),
                                                );
                                            });

                                            // stdout (monospace)
                                            if !cmd.stdout.is_empty() {
                                                ui.add_space(2.0);
                                                egui::Frame::new()
                                                    .fill(if dark {
                                                        egui::Color32::from_rgba_premultiplied(
                                                            20, 20, 30, 80,
                                                        )
                                                    } else {
                                                        egui::Color32::from_rgba_premultiplied(
                                                            230, 230, 240, 180,
                                                        )
                                                    })
                                                    .corner_radius(4.0)
                                                    .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                                                    .show(ui, |ui| {
                                                        let max_stdout: String =
                                                            cmd.stdout.chars().take(500).collect();
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "[stdout]\n{}",
                                                                max_stdout
                                                            ))
                                                            .size(9.0)
                                                            .family(egui::FontFamily::Monospace)
                                                            .color(weak_text),
                                                        );
                                                    });
                                            }

                                            // stderr (monospace)
                                            if !cmd.stderr.is_empty() {
                                                ui.add_space(2.0);
                                                egui::Frame::new()
                                                    .fill(if dark {
                                                        egui::Color32::from_rgba_premultiplied(
                                                            50, 20, 20, 80,
                                                        )
                                                    } else {
                                                        egui::Color32::from_rgba_premultiplied(
                                                            255, 235, 235, 180,
                                                        )
                                                    })
                                                    .corner_radius(4.0)
                                                    .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                                                    .show(ui, |ui| {
                                                        let max_stderr: String =
                                                            cmd.stderr.chars().take(500).collect();
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "[stderr]\n{}",
                                                                max_stderr
                                                            ))
                                                            .size(9.0)
                                                            .family(egui::FontFamily::Monospace)
                                                            .color(if dark {
                                                                egui::Color32::from_rgb(
                                                                    220, 120, 120,
                                                                )
                                                            } else {
                                                                egui::Color32::from_rgb(180, 60, 60)
                                                            }),
                                                        );
                                                    });
                                            }

                                            if cmd_idx + 1 < command_records.len() {
                                                ui.separator();
                                            }
                                        }
                                    });
                            }
                        }
                    });
            });
        });
        ui.add_space(6.0);
    }

    // Feature 5a: show AI thinking indicator while streaming
    if chat.ai_status == AiStatus::Thinking && !chat.session_state.sessions.is_empty() {
        if let Some(session) = chat
            .session_state
            .sessions
            .get(chat.session_state.active_session)
        {
            if let Some(last) = session.messages.last() {
                if last.role == "assistant" && last.content.is_empty() && last.thinking.is_empty() {
                    // Show thinking indicator after the last placeholder message
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        draw_role_avatar(ui, false);
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
    if chat.last_token_estimate > 0 {
        let msgs = chat.messages();
        if let Some(last) = msgs.last() {
            if last.role == "assistant" {
                ui.horizontal(|ui| {
                    ui.add_space(36.0);
                    ui.colored_label(
                        muted_text,
                        format!(
                            "⚡ {}",
                            i18n.t("chat.tokenSummary")
                                .replace("{input}", &chat.input_token_estimate.to_string())
                                .replace("{output}", &chat.output_token_estimate.to_string())
                                .replace("{total}", &chat.last_token_estimate.to_string())
                        ),
                    );
                });
            }
        }
    }

    // GAP-B50-01: Token-level streaming progress indicator
    // Wire: only show when mode policy permits progress display
    if chat.mode_policy.show_progress_steps
        && chat.sending
        && chat.stream_state.stream_progress.tokens_received > 0
    {
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            let prog = &chat.stream_state.stream_progress;
            let detail = format!(
                "📨 {} tokens · {} KB received{}",
                prog.tokens_received,
                prog.bytes_processed / 1024,
                if prog.total_tokens > 0 {
                    format!(
                        " · {}/{} output tokens",
                        prog.output_tokens, prog.total_tokens
                    )
                } else {
                    String::new()
                },
            );
            ui.colored_label(
                if ui.visuals().dark_mode {
                    egui::Color32::from_rgb(100, 180, 255)
                } else {
                    egui::Color32::from_rgb(0, 80, 180)
                },
                detail,
            );
        });
    }
}
