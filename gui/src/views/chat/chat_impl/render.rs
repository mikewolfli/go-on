use super::*;

impl ChatView {
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
            ui.colored_label(
                egui::Color32::from_rgb(220, 170, 80),
                truncation_hint.replace("{chars}", &text.len().to_string()),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(preview).color(text_color));
            return;
        }

        render_with_thinking(ui, text, text_color, copy_code_hint);
    }
}

/// Split and render text with <thinking>...</thinking> blocks
fn render_with_thinking(
    ui: &mut egui::Ui,
    text: &str,
    text_color: egui::Color32,
    copy_code_hint: &str,
) {
    const OPEN: &str = "<thinking>";
    const CLOSE: &str = "</thinking>";

    let lower = text.to_ascii_lowercase();
    if let Some(start) = lower.find(OPEN) {
        let before = &text[..start];
        if !before.trim().is_empty() {
            render_markdown_content(ui, before, text_color, copy_code_hint);
        }

        let after_open = start + OPEN.len();
        let (thinking_content, rest) = match lower[after_open..].find(CLOSE) {
            Some(rel) => (
                &text[after_open..after_open + rel],
                &text[after_open + rel + CLOSE.len()..],
            ),
            None => (&text[after_open..], ""),
        };

        // Use a hash-based ID for this thinking block to ensure state persistence
        let thinking_id = egui::Id::new(format!("thinking_{}", start));
        egui::CollapsingHeader::new(
            egui::RichText::new("💭 Thinking...")
                .color(egui::Color32::from_rgb(140, 140, 170))
                .italics()
                .size(12.5),
        )
        .id_salt(thinking_id)
        .default_open(false)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                // Render thinking content with scroll area for long content
                let line_count = thinking_content.lines().count();
                let estimated_height = (line_count as f32 * 14.0).min(250.0).max(40.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .max_height(estimated_height)
                    .show(ui, |ui| {
                        let content_str = thinking_content.trim();
                        if !content_str.is_empty() {
                            ui.label(
                                egui::RichText::new(content_str)
                                    .color(egui::Color32::from_rgb(155, 155, 180))
                                    .size(11.0),
                            );
                        }
                    });
            });
        });
        ui.add_space(4.0);

        if !rest.trim().is_empty() {
            render_with_thinking(ui, rest, text_color, copy_code_hint);
        }
    } else {
        render_markdown_content(ui, text, text_color, copy_code_hint);
    }
}

fn render_markdown_content(
    ui: &mut egui::Ui,
    text: &str,
    text_color: egui::Color32,
    copy_code_hint: &str,
) {
    let mut options = comrak::Options::default();
    options.extension.strikethrough = true;
    options.extension.tagfilter = false;
    options.render.hardbreaks = true;
    options.render.github_pre_lang = true;

    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, text, &options);
    render_children(ui, root, text_color, copy_code_hint);
}

fn render_children<'a>(
    ui: &mut egui::Ui,
    node: &'a comrak::nodes::AstNode<'a>,
    text_color: egui::Color32,
    copy_code_hint: &str,
) {
    for child in node.children() {
        render_comrak_node(ui, child, text_color, copy_code_hint);
    }
}

fn render_comrak_node<'a>(
    ui: &mut egui::Ui,
    node: &'a comrak::nodes::AstNode<'a>,
    text_color: egui::Color32,
    copy_code_hint: &str,
) {
    let ast = node.data.borrow();
    match &ast.value {
        comrak::nodes::NodeValue::Document => {
            render_children(ui, node, text_color, copy_code_hint);
        }
        comrak::nodes::NodeValue::Paragraph => {
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(ui.available_width());
                    render_children(ui, node, text_color, copy_code_hint);
                });
            });
            ui.add_space(4.0);
        }
        comrak::nodes::NodeValue::Heading(heading) => {
            let text = collect_text(node);
            let size = match heading.level {
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
        comrak::nodes::NodeValue::CodeBlock(info) => {
            let lang = info.info.trim().to_string();
            let code = collect_text(node);

            // Use collapsible header for code blocks
            let code_id = egui::Id::new(format!("code_{:p}", node as *const _));
            egui::CollapsingHeader::new(
                egui::RichText::new(format!(
                    "📄 Code{}",
                    if !lang.is_empty() {
                        format!(" ({})", lang)
                    } else {
                        String::new()
                    }
                ))
                .color(egui::Color32::from_rgb(100, 200, 255))
                .size(12.0),
            )
            .id_salt(code_id)
            .default_open(true)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    // Copy button in header
                    ui.horizontal(|ui| {
                        if ui
                            .button("📋 Copy")
                            .on_hover_text("Copy to clipboard")
                            .clicked()
                        {
                            ui.ctx().copy_text(code.clone());
                        }
                    });
                    ui.separator();

                    // Code content with scroll area
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(30, 30, 35))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 90)))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            let line_count = code.lines().count();
                            let estimated_height = (line_count as f32 * 15.0).min(400.0).max(80.0);

                            egui::ScrollArea::vertical()
                                .auto_shrink([false; 2])
                                .max_height(estimated_height)
                                .show(ui, |ui| {
                                    egui::ScrollArea::horizontal().auto_shrink([false; 2]).show(
                                        ui,
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(&code)
                                                    .font(egui::FontId::monospace(12.0))
                                                    .color(egui::Color32::from_rgb(220, 220, 220)),
                                            );
                                        },
                                    );
                                });
                        });
                });
            });
            ui.add_space(4.0);
        }
        comrak::nodes::NodeValue::Code(..) => {
            let text = collect_text(node);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .color(egui::Color32::from_rgb(220, 80, 80))
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
        comrak::nodes::NodeValue::ThematicBreak => {
            ui.separator();
            ui.add_space(4.0);
        }
        comrak::nodes::NodeValue::BlockQuote => {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(128, 128, 128, 20))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10i8, 4i8))
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
        comrak::nodes::NodeValue::SoftBreak | comrak::nodes::NodeValue::LineBreak => {
            // handled by text rendering via collect_text, or ignored
        }
        _ => {
            render_children(ui, node, text_color, copy_code_hint);
        }
    }
}

/// Collect all text content from a comrak node tree
fn collect_text<'a>(node: &'a comrak::nodes::AstNode<'a>) -> String {
    let ast = node.data.borrow();
    match &ast.value {
        comrak::nodes::NodeValue::Text(ref literal) => literal.to_string(),
        comrak::nodes::NodeValue::Code(..) => format!("`{}`", collect_text_children(node)),
        comrak::nodes::NodeValue::LineBreak | comrak::nodes::NodeValue::SoftBreak => {
            "\n".to_string()
        }
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
