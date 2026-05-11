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

        // comrak for full markdown rendering (bold, italic, headings, lists, code blocks, inline code).
        // Safe now because layout no longer uses horizontal_top/bottom_up which caused the freeze.
        let mut options = comrak::Options::default();
        options.extension.strikethrough = true;
        options.extension.tagfilter = false;
        options.render.hardbreaks = true;
        options.render.github_pre_lang = true;

        let arena = comrak::Arena::new();
        let root = comrak::parse_document(&arena, text, &options);
        render_comrak_node(ui, root, text_color, copy_code_hint);
    }
}

// ── Free functions for comrak tree traversal ─────────────────

/// Render children of a comrak node
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

/// Render a single comrak AST node into egui widgets
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
            // Use vertical layout with wrapping to ensure proper line breaks
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(ui.available_width());
                    render_children(ui, node, text_color, copy_code_hint);
                });
            });
            ui.add_space(4.0);
        }
        comrak::nodes::NodeValue::Heading(heading) => {
            let size = match heading.level {
                1 => 20.0,
                2 => 17.0,
                3 => 15.0,
                _ => 14.0,
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(collect_text(node))
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
            let code = collect_text(node);
            let lang = info.info.trim();
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(40, 44, 52))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10i8, 8i8))
                .show(ui, |ui| {
                    if !lang.is_empty() {
                        ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::from_rgb(150, 152, 160), lang);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                if ui.button("📋").on_hover_text(copy_code_hint).clicked() {
                                    ui.ctx().copy_text(code.clone());
                                }
                            });
                        });
                    }
                    // Use ScrollArea for long code blocks
                    egui::ScrollArea::horizontal()
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(
                                egui::RichText::new(code)
                                    .font(egui::FontId::monospace(13.0))
                                    .color(egui::Color32::from_rgb(200, 204, 212)),
                            ));
                        });
                });
            ui.add_space(4.0);
        }
        comrak::nodes::NodeValue::Code(..) => {
            let code = collect_text(node);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(code)
                        .color(egui::Color32::from_rgb(220, 80, 80))
                        .family(egui::FontFamily::Monospace),
                )
                .wrap(),
            );
        }
        comrak::nodes::NodeValue::Strong => {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(collect_text(node))
                        .strong()
                        .color(text_color),
                )
                .wrap(),
            );
        }
        comrak::nodes::NodeValue::Emph => {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(collect_text(node))
                        .italics()
                        .color(text_color),
                )
                .wrap(),
            );
        }
        comrak::nodes::NodeValue::Text(ref literal) => {
            let t = literal.to_string();
            if !t.trim().is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(t).color(text_color)).wrap());
            }
        }
        comrak::nodes::NodeValue::LineBreak | comrak::nodes::NodeValue::SoftBreak => {}
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
            let url = link.url.to_string();
            let label = collect_text(node);
            let display = if label.is_empty() { url.clone() } else { label };
            if ui.link(display).clicked() {
                // Link opened externally by egui; no custom handler needed.
            }
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
