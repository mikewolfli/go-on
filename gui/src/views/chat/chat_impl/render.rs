use super::*;

impl ChatView {
    /// Draw a small colored avatar circle with initials, returning the interaction response
    #[allow(dead_code)]
    fn avatar_circle_with_actions(
        ui: &mut egui::Ui,
        size: f32,
        color: egui::Color32,
        label: &str,
        _msg_idx: &usize,
    ) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
        let painter = ui.painter();
        painter.circle_filled(rect.center(), size / 2.0, color);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        resp
    }

    /// Draw a small colored avatar circle with initials
    #[allow(dead_code)]
    fn avatar_circle(ui: &mut egui::Ui, size: f32, color: egui::Color32, label: &str) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let painter = ui.painter();
        painter.circle_filled(rect.center(), size / 2.0, color);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
    }

    pub(super) fn render_markdown(
        ui: &mut egui::Ui,
        text: &str,
        copy_code_hint: &str,
        text_color: egui::Color32,
    ) {
        if CHAT_DISABLE_MARKDOWN_RENDER {
            ui.label(egui::RichText::new(Self::markdown_to_plain_text(text)).color(text_color));
            return;
        }

        const MAX_MARKDOWN_CHARS: usize = 5_000;
        if text.len() > MAX_MARKDOWN_CHARS {
            let preview: String = text.chars().take(MAX_MARKDOWN_CHARS).collect();
            ui.colored_label(
                egui::Color32::from_rgb(220, 170, 80),
                format!(
                    "⚠️ Large message ({} chars) truncated for UI safety",
                    text.len()
                ),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(preview).color(text_color));
            return;
        }

        // Simple markdown renderer: handles code blocks (```...```) and plain text.
        // Avoids comrak which can cause UI hangs with certain inputs.
        let mut remaining = text;
        loop {
            if let Some(start) = remaining.find("```") {
                let before = &remaining[..start];
                if !before.trim().is_empty() {
                    for para in before.trim().split("\n\n") {
                        let p = para.trim();
                        if !p.is_empty() {
                            ui.label(egui::RichText::new(p).color(text_color));
                        }
                    }
                }
                remaining = &remaining[start + 3..];
                let endline = remaining.find('\n').unwrap_or(remaining.len());
                let _lang = remaining[..endline].trim().to_string();
                remaining = &remaining[endline.min(remaining.len())..];
                if remaining.starts_with('\n') {
                    remaining = &remaining[1..];
                }
                if let Some(end) = remaining.find("```") {
                    let code = &remaining[..end].trim_end();
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(40, 44, 52))
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.colored_label(egui::Color32::from_rgb(150, 152, 160), "code");
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        if ui
                                            .button("\u{1f4cb}")
                                            .on_hover_text(copy_code_hint)
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(code.to_string());
                                        }
                                    },
                                );
                            });
                            ui.add(egui::Label::new(
                                egui::RichText::new(code.to_string())
                                    .font(egui::FontId::monospace(13.0))
                                    .color(egui::Color32::from_rgb(200, 204, 212)),
                            ));
                        });
                    remaining = &remaining[end + 3..];
                } else {
                    // No closing ``` found — treat the rest as plain text
                    let rest = remaining.trim();
                    if !rest.is_empty() {
                        ui.label(egui::RichText::new(rest).color(text_color));
                    }
                    break;
                }
            } else {
                if !remaining.trim().is_empty() {
                    for para in remaining.trim().split("\n\n") {
                        let p = para.trim();
                        if !p.is_empty() {
                            // Handle inline code with backticks
                            let parts: Vec<&str> = p.split('`').collect();
                            ui.horizontal_wrapped(|ui| {
                                for (i, part) in parts.iter().enumerate() {
                                    if i % 2 == 0 && !part.trim().is_empty() {
                                        ui.label(
                                            egui::RichText::new(part.trim()).color(text_color),
                                        );
                                    } else if !part.trim().is_empty() {
                                        ui.label(
                                            egui::RichText::new(part.trim())
                                                .color(egui::Color32::from_rgb(220, 80, 80))
                                                .family(egui::FontFamily::Monospace),
                                        );
                                    }
                                }
                            });
                            ui.add_space(2.0);
                        }
                    }
                }
                break;
            }
        }
    }

    /// Draw a small colored avatar circle with initials
    #[cfg(test)]
    #[allow(dead_code)]
    /// Render the content inside a message bubble
    fn message_bubble_content(
        &mut self,
        ui: &mut egui::Ui,
        msg: &Message,
        text_color: egui::Color32,
        i18n: &I18n,
    ) {
        // Timestamp + copy row
        ui.horizontal(|ui| {
            let ts_color = egui::Color32::from_rgb(160, 162, 170);
            let time_str = format_absolute_time(msg.timestamp);
            ui.colored_label(ts_color, time_str);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let copy_btn = egui::Button::new("\u{1f4cb}")
                    .min_size(egui::vec2(18.0, 14.0))
                    .fill(egui::Color32::from_rgba_premultiplied(0, 0, 0, 0));
                if ui
                    .add(copy_btn)
                    .on_hover_text(i18n.t("chat.copy"))
                    .clicked()
                {
                    ui.ctx().copy_text(msg.content.clone());
                }
            });
        });

        // Attachments
        for att in &msg.attachments {
            let icon = if att.mime.starts_with("image/") {
                "\u{1f5bc}"
            } else {
                "\u{1f4ce}"
            };
            ui.label(egui::RichText::new(format!("{} {}", icon, att.name)).color(text_color));
        }

        // Think toggle
        if msg.role == "assistant" && !msg.thinking.is_empty() {
            let toggle = if msg.show_thinking_msg {
                ui.button(format!("\u{25b2} {}", i18n.t("chat.thinkingLabel")))
            } else {
                ui.button(format!("\u{25bc} {}", i18n.t("chat.thinkingLabel")))
            };
            if toggle.clicked() {
                let session_msgs = &mut self.session().messages;
                if let Some(m) = session_msgs
                    .iter_mut()
                    .find(|m| m.timestamp == msg.timestamp)
                {
                    m.show_thinking_msg = !m.show_thinking_msg;
                }
            }
        }

        // Thinking text
        if msg.show_thinking_msg && !msg.thinking.is_empty() {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(250, 242, 220))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                .show(ui, |ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 130, 30),
                        i18n.t("chat.thinkingLabel"),
                    );
                    let think_resp = ui.label(
                        egui::RichText::new(&msg.thinking)
                            .color(egui::Color32::from_rgb(80, 60, 20)),
                    );
                    think_resp.context_menu(|ui| {
                        if ui
                            .button(format!("\u{1f4cb} {}", i18n.t("chat.copy")))
                            .clicked()
                        {
                            ui.ctx().copy_text(msg.thinking.clone());
                            ui.close_menu();
                        }
                    });
                });
        }

        // Main content - now handled inline in show_messages via render_markdown
        // Keep this method for backward compatibility / other callers
        let content_resp = ui.label(egui::RichText::new(&msg.content).color(text_color));
        content_resp.context_menu(|ui| {
            if ui
                .button(format!("\u{1f4cb} {}", i18n.t("chat.copy")))
                .clicked()
            {
                ui.ctx().copy_text(msg.content.clone());
                ui.close_menu();
            }
        });
    }
}
