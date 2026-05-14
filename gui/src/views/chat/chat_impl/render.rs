use super::*;

impl ChatView {
    /// Render markdown text using comrak for full markdown support.
    /// Code blocks are rendered with language label, styled Frame, scroll,
    /// and a copy button — always visible (no collapsing).
    pub(super) fn render_markdown(
        ui: &mut egui::Ui,
        text: &str,
        copy_code_hint: &str,
        enable_markdown: bool,
        text_color: egui::Color32,
        truncation_hint: &str,
    ) {
        if CHAT_DISABLE_MARKDOWN_RENDER || !enable_markdown {
            ui.label(egui::RichText::new(Self::markdown_to_plain_text(text)).color(text_color));
            return;
        }

        const MAX_MARKDOWN_CHARS: usize = 10_000;
        if text.len() > MAX_MARKDOWN_CHARS {
            let preview: String = text.chars().take(MAX_MARKDOWN_CHARS).collect();
            let trunc_color = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(220, 170, 80)
            } else {
                egui::Color32::from_rgb(180, 130, 40)
            };
            ui.colored_label(
                trunc_color,
                truncation_hint.replace("{chars}", &text.len().to_string()),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(preview).color(text_color));
            return;
        }

        // Parse with comrak and render node tree
        let mut options = comrak::Options::default();
        options.extension.strikethrough = true;
        options.extension.tagfilter = false;
        options.render.hardbreaks = true;
        options.render.github_pre_lang = true;

        let arena = comrak::Arena::new();
        let root = comrak::parse_document(&arena, text, &options);
        render_node(ui, root, text_color, copy_code_hint);
    }
}

// ── Tree traversal ───────────────────────────────────────────────────────

fn render_children<'a>(
    ui: &mut egui::Ui,
    node: &'a comrak::nodes::AstNode<'a>,
    text_color: egui::Color32,
    copy_code_hint: &str,
) {
    for child in node.children() {
        render_node(ui, child, text_color, copy_code_hint);
    }
}

fn render_node<'a>(
    ui: &mut egui::Ui,
    node: &'a comrak::nodes::AstNode<'a>,
    text_color: egui::Color32,
    copy_code_hint: &str,
) {
    let ast = node.data.borrow();
    match &ast.value {
        comrak::nodes::NodeValue::Document
        | comrak::nodes::NodeValue::Paragraph
        | comrak::nodes::NodeValue::Item(_) => {
            render_children(ui, node, text_color, copy_code_hint);
        }

        comrak::nodes::NodeValue::Heading(h) => {
            let text = collect_text(node);
            let size = match h.level {
                1 => 20.0,
                2 => 17.0,
                3 => 15.0,
                _ => 14.0,
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(size)
                        .strong()
                        .color(text_color),
                )
                .wrap(),
            );
            ui.add_space(4.0);
        }

        comrak::nodes::NodeValue::List(list) => {
            let ordered = list.list_type == comrak::nodes::ListType::Ordered;
            for (i, child) in node.children().enumerate() {
                let prefix = if ordered {
                    format!("{}. ", i + 1)
                } else {
                    "• ".to_string()
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(prefix).color(text_color));
                    render_children(ui, child, text_color, copy_code_hint);
                });
            }
            ui.add_space(4.0);
        }

        // ── Code block: always visible with language label, scroll, copy ──
        comrak::nodes::NodeValue::CodeBlock(info) => {
            let lang = info.info.trim();
            let code = info.literal.clone();

            // Language badge
            ui.horizontal(|ui| {
                if !lang.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("⬡ {}", lang))
                            .color(egui::Color32::from_rgb(100, 200, 255))
                            .size(11.0),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(egui::RichText::new("📋").size(11.0))
                        .on_hover_text(copy_code_hint)
                        .clicked()
                    {
                        ui.ctx().copy_text(code.clone());
                    }
                });
            });

            // Code display: theme-aware frame + monospace + scroll
            let code_bg = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(30, 30, 35)
            } else {
                egui::Color32::from_rgb(245, 245, 250)
            };
            let code_border = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(60, 60, 70)
            } else {
                egui::Color32::from_rgb(200, 200, 210)
            };
            let code_fg = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(200, 204, 212)
            } else {
                egui::Color32::from_rgb(40, 44, 50)
            };
            egui::Frame::new()
                .fill(code_bg)
                .stroke(egui::Stroke::new(1.0, code_border))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    let line_count = code.lines().count().max(1) as f32;
                    let max_h = (line_count * 18.0).clamp(60.0, 400.0);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .max_height(max_h)
                        .show(ui, |ui| {
                            egui::ScrollArea::horizontal()
                                .auto_shrink([false; 2])
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(&code)
                                            .font(egui::FontId::monospace(13.0))
                                            .color(code_fg),
                                    );
                                });
                        });
                });
            ui.add_space(6.0);
        }

        // ── Inline code ──
        comrak::nodes::NodeValue::Code(..) => {
            let text = collect_text(node);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .color(if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(255, 120, 120)
                        } else {
                            egui::Color32::from_rgb(180, 40, 40)
                        })
                        .family(egui::FontFamily::Monospace),
                )
                .wrap(),
            );
        }

        comrak::nodes::NodeValue::Strong => {
            let text = collect_text(node);
            ui.add(egui::Label::new(egui::RichText::new(text).strong().color(text_color)).wrap());
        }

        comrak::nodes::NodeValue::Emph => {
            let text = collect_text(node);
            ui.add(egui::Label::new(egui::RichText::new(text).italics().color(text_color)).wrap());
        }

        comrak::nodes::NodeValue::Text(ref literal) => {
            if !literal.trim().is_empty() {
                ui.add(
                    egui::Label::new(egui::RichText::new(literal.as_str()).color(text_color))
                        .wrap(),
                );
            }
        }

        comrak::nodes::NodeValue::LineBreak | comrak::nodes::NodeValue::SoftBreak => {
            // newlines handled by hardbreaks option
        }

        comrak::nodes::NodeValue::ThematicBreak => {
            ui.separator();
            ui.add_space(4.0);
        }

        comrak::nodes::NodeValue::BlockQuote => {
            let quote_bg = if ui.visuals().dark_mode {
                egui::Color32::from_rgba_premultiplied(128, 128, 128, 25)
            } else {
                egui::Color32::from_rgba_premultiplied(128, 128, 128, 15)
            };
            egui::Frame::new()
                .fill(quote_bg)
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10, 4))
                .show(ui, |ui| {
                    render_children(ui, node, text_color, copy_code_hint);
                });
            ui.add_space(4.0);
        }

        comrak::nodes::NodeValue::Link(link) => {
            let label = collect_text(node);
            let display = if label.is_empty() {
                link.url.to_string()
            } else {
                label
            };
            let _ = ui.link(display).clicked();
        }

        _ => {
            render_children(ui, node, text_color, copy_code_hint);
        }
    }
}

/// Collect text from a comrak node tree
fn collect_text<'a>(node: &'a comrak::nodes::AstNode<'a>) -> String {
    let ast = node.data.borrow();
    match &ast.value {
        comrak::nodes::NodeValue::Text(ref literal) => literal.to_string(),
        comrak::nodes::NodeValue::Code(..) => format!("`{}`", collect_text_children(node)),
        _ => collect_text_children(node),
    }
}

fn collect_text_children<'a>(node: &'a comrak::nodes::AstNode<'a>) -> String {
    let mut result = String::new();
    for child in node.children() {
        result.push_str(&collect_text(child));
    }
    result
}
