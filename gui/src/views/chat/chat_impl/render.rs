use super::*;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

const MARKDOWN_CACHE_MAX_ENTRIES: usize = 256;

#[derive(Clone)]
enum CachedNode {
    Document(Vec<CachedNode>),
    Paragraph(Vec<CachedNode>),
    Heading {
        level: u8,
        text: String,
    },
    List {
        ordered: bool,
        items: Vec<Vec<CachedNode>>,
    },
    CodeBlock {
        lang: String,
        code: String,
    },
    Code(String),
    Strong(String),
    Emph(String),
    Text(String),
    ThematicBreak,
    BlockQuote(Vec<CachedNode>),
    Link {
        url: String,
        label: String,
    },
    Other(Vec<CachedNode>),
}

#[derive(Clone)]
struct CachedMarkdownDoc {
    source: String,
    root: CachedNode,
}

struct MarkdownRenderCache {
    docs: HashMap<u64, CachedMarkdownDoc>,
    order: VecDeque<u64>,
}

impl MarkdownRenderCache {
    fn new() -> Self {
        Self {
            docs: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get_or_parse(&mut self, text: &str) -> CachedNode {
        let key = markdown_cache_key(text);
        if let Some(doc) = self.docs.get(&key) {
            if doc.source == text {
                let cached = doc.root.clone();
                self.touch(key);
                return cached;
            }
        }

        let parsed = parse_markdown_cached_node(text);
        self.docs.insert(
            key,
            CachedMarkdownDoc {
                source: text.to_string(),
                root: parsed.clone(),
            },
        );
        self.touch(key);
        while self.order.len() > MARKDOWN_CACHE_MAX_ENTRIES {
            if let Some(evicted) = self.order.pop_front() {
                self.docs.remove(&evicted);
            }
        }
        parsed
    }

    fn touch(&mut self, key: u64) {
        if let Some(pos) = self.order.iter().position(|k| *k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key);
    }
}

fn markdown_cache() -> &'static Mutex<MarkdownRenderCache> {
    static CACHE: OnceLock<Mutex<MarkdownRenderCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(MarkdownRenderCache::new()))
}

fn markdown_cache_key(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn parse_markdown_cached_node(text: &str) -> CachedNode {
    let mut options = comrak::Options::default();
    options.extension.strikethrough = true;
    options.extension.tagfilter = false;
    options.render.hardbreaks = true;
    options.render.github_pre_lang = true;

    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, text, &options);
    to_cached_node(root)
}

fn to_cached_node<'a>(node: &'a comrak::nodes::AstNode<'a>) -> CachedNode {
    let ast = node.data.borrow();
    match &ast.value {
        comrak::nodes::NodeValue::Document => {
            CachedNode::Document(node.children().map(to_cached_node).collect())
        }
        comrak::nodes::NodeValue::Paragraph => {
            CachedNode::Paragraph(node.children().map(to_cached_node).collect())
        }
        comrak::nodes::NodeValue::Heading(heading) => CachedNode::Heading {
            level: heading.level,
            text: collect_text(node),
        },
        comrak::nodes::NodeValue::List(list) => {
            let ordered = list.list_type == comrak::nodes::ListType::Ordered;
            let mut items = Vec::new();
            for child in node.children() {
                items.push(child.children().map(to_cached_node).collect());
            }
            CachedNode::List { ordered, items }
        }
        comrak::nodes::NodeValue::CodeBlock(info) => CachedNode::CodeBlock {
            lang: info.info.trim().to_string(),
            code: collect_text(node),
        },
        comrak::nodes::NodeValue::Code(..) => CachedNode::Code(collect_text(node)),
        comrak::nodes::NodeValue::Strong => CachedNode::Strong(collect_text(node)),
        comrak::nodes::NodeValue::Emph => CachedNode::Emph(collect_text(node)),
        comrak::nodes::NodeValue::Text(ref literal) => CachedNode::Text(literal.to_string()),
        comrak::nodes::NodeValue::ThematicBreak => CachedNode::ThematicBreak,
        comrak::nodes::NodeValue::BlockQuote => {
            CachedNode::BlockQuote(node.children().map(to_cached_node).collect())
        }
        comrak::nodes::NodeValue::Link(link) => CachedNode::Link {
            url: link.url.to_string(),
            label: collect_text(node),
        },
        _ => CachedNode::Other(node.children().map(to_cached_node).collect()),
    }
}

fn render_cached_node(
    ui: &mut egui::Ui,
    node: &CachedNode,
    text_color: egui::Color32,
    copy_code_hint: &str,
) {
    match node {
        CachedNode::Document(children) => {
            for child in children {
                render_cached_node(ui, child, text_color, copy_code_hint);
            }
        }
        CachedNode::Paragraph(children) => {
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.set_max_width(ui.available_width());
                    for child in children {
                        render_cached_node(ui, child, text_color, copy_code_hint);
                    }
                });
            });
            ui.add_space(4.0);
        }
        CachedNode::Heading { level, text } => {
            let size = match *level {
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
        CachedNode::List { ordered, items } => {
            for (i, item_nodes) in items.iter().enumerate() {
                let prefix = if *ordered {
                    format!("{}. ", i + 1)
                } else {
                    "• ".to_string()
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(prefix).color(text_color));
                    for child in item_nodes {
                        render_cached_node(ui, child, text_color, copy_code_hint);
                    }
                });
            }
            ui.add_space(4.0);
        }
        CachedNode::CodeBlock { lang, code } => {
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
        CachedNode::Code(code) => {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(code)
                        .color(egui::Color32::from_rgb(220, 80, 80))
                        .family(egui::FontFamily::Monospace),
                )
                .wrap(),
            );
        }
        CachedNode::Strong(text) => {
            ui.add(egui::Label::new(egui::RichText::new(text).strong().color(text_color)).wrap());
        }
        CachedNode::Emph(text) => {
            ui.add(egui::Label::new(egui::RichText::new(text).italics().color(text_color)).wrap());
        }
        CachedNode::Text(text) => {
            if !text.trim().is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(text).color(text_color)).wrap());
            }
        }
        CachedNode::ThematicBreak => {
            ui.separator();
            ui.add_space(4.0);
        }
        CachedNode::BlockQuote(children) => {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(128, 128, 128, 20))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(10i8, 4i8))
                .show(ui, |ui| {
                    for child in children {
                        render_cached_node(ui, child, text_color, copy_code_hint);
                    }
                });
            ui.add_space(4.0);
        }
        CachedNode::Link { url, label } => {
            let display = if label.is_empty() {
                url.clone()
            } else {
                label.clone()
            };
            let _ = ui.link(display).clicked();
        }
        CachedNode::Other(children) => {
            for child in children {
                render_cached_node(ui, child, text_color, copy_code_hint);
            }
        }
    }
}

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

        let parsed = if let Ok(mut cache) = markdown_cache().lock() {
            cache.get_or_parse(text)
        } else {
            parse_markdown_cached_node(text)
        };
        render_cached_node(ui, &parsed, text_color, copy_code_hint);
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
