use super::ChatView;
use super::CHAT_DISABLE_MARKDOWN_RENDER;

use crate::views::chat::types::{CachedMarkdownRender, MarkdownSegment, MarkdownStyle};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

/// Global cache for parsed markdown content, keyed by text hash.
/// Allows large documents to be parsed once and reused across messages and frames
/// without blocking the UI thread with comrak parsing on subsequent renders.
static MARKDOWN_CACHE: OnceLock<Mutex<HashMap<u64, CachedMarkdownRender>>> = OnceLock::new();

fn markdown_cache() -> &'static Mutex<HashMap<u64, CachedMarkdownRender>> {
    MARKDOWN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Compute a hash of the markdown text for cache lookup.
fn hash_text(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl ChatView {
    /// Render markdown text using comrak for full markdown support.
    /// Code blocks are rendered with language label, styled Frame, scroll,
    /// and a copy button — always visible (no collapsing).
    ///
    /// Optimization: Uses a global `MARKDOWN_CACHE` keyed by text hash.
    /// All comrak parsing happens on a background thread; the UI thread
    /// never blocks on markdown parsing.
    ///
    /// Cache hit: renders pre-parsed segments directly (fast path).
    /// Cache miss: renders plain text immediately (using markdown_to_plain_text)
    /// for instant visual feedback, then spawns a background thread to parse
    /// with comrak. When parsing completes, the result is cached and a repaint
    /// is requested so the rich rendering appears next frame.
    pub(super) fn render_markdown(
        ui: &mut egui::Ui,
        text: &str,
        copy_code_hint: &str,
        enable_markdown: bool,
        text_color: egui::Color32,
        _truncation_hint: &str,
    ) {
        if CHAT_DISABLE_MARKDOWN_RENDER || !enable_markdown {
            ui.label(egui::RichText::new(Self::markdown_to_plain_text(text)).color(text_color));
            return;
        }

        // Check global cache first (for all document sizes)
        let hash = hash_text(text);
        if let Ok(cache) = markdown_cache().lock() {
            if let Some(cached) = cache.get(&hash) {
                return Self::render_markdown_from_cache(ui, cached, copy_code_hint, text_color);
            }
        }

        // Cache miss: render plain text immediately instead of "Rendering…"
        // This provides instant visual feedback while comrak parses in background.
        Self::render_plain_text_fallback(ui, text, text_color);

        let text = text.to_string();
        let ctx = ui.ctx().clone();
        let _ = std::thread::spawn(move || {
            let cached = parse_markdown_to_segments(&text);
            if let Ok(mut cache) = markdown_cache().lock() {
                // Bounded cache: evict oldest entry if at capacity
                const MAX_CACHE_ENTRIES: usize = 50;
                if cache.len() >= MAX_CACHE_ENTRIES {
                    if let Some(key) = cache.keys().next().copied() {
                        cache.remove(&key);
                    }
                }
                cache.insert(hash, cached);
            }
            ctx.request_repaint();
        });
    }

    /// Render markdown from pre-parsed segments (cache hit path).
    /// No comrak parsing needed — segments are rendered directly with egui.
    pub(super) fn render_markdown_from_cache(
        ui: &mut egui::Ui,
        cache: &CachedMarkdownRender,
        copy_code_hint: &str,
        text_color: egui::Color32,
    ) {
        for segment in &cache.segments {
            Self::render_segment(ui, segment, copy_code_hint, text_color);
        }
    }

    /// Render plain text immediately as a visual fallback while comrak parses
    /// in the background. Strips basic markdown syntax for readable instant display
    /// — no comrak overhead, no UI thread blocking.
    fn render_plain_text_fallback(ui: &mut egui::Ui, text: &str, text_color: egui::Color32) {
        let plain = Self::markdown_to_plain_text(text);
        ui.add(egui::Label::new(egui::RichText::new(plain).color(text_color)).wrap());
    }

    /// Render a single pre-parsed markdown segment.
    fn render_segment(
        ui: &mut egui::Ui,
        segment: &MarkdownSegment,
        copy_code_hint: &str,
        text_color: egui::Color32,
    ) {
        match segment {
            MarkdownSegment::Text(text, style) => {
                let mut rich = egui::RichText::new(text.as_str()).color(text_color);
                if style.bold {
                    rich = rich.strong();
                }
                if style.italic {
                    rich = rich.italics();
                }
                if style.font_size > 0.0 {
                    rich = rich.size(style.font_size);
                }
                ui.add(egui::Label::new(rich).wrap());
            }
            MarkdownSegment::CodeBlock(lang, code) => {
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
                        let code_ref = code.clone();
                        if ui
                            .button(egui::RichText::new("📋").size(11.0))
                            .on_hover_text(copy_code_hint)
                            .clicked()
                        {
                            ui.ctx().copy_text(code_ref);
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
                                egui::ScrollArea::horizontal().auto_shrink([false; 2]).show(
                                    ui,
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(code.as_str())
                                                .font(egui::FontId::monospace(13.0))
                                                .color(code_fg),
                                        );
                                    },
                                );
                            });
                    });
                ui.add_space(6.0);
            }
            MarkdownSegment::InlineCode(code) => {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(code.as_str())
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
            MarkdownSegment::ThematicBreak => {
                ui.separator();
                ui.add_space(4.0);
            }
            MarkdownSegment::Heading(level, text) => {
                let size = match level {
                    1 => 20.0,
                    2 => 17.0,
                    3 => 15.0,
                    _ => 14.0,
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(text.as_str())
                            .size(size)
                            .strong()
                            .color(text_color),
                    )
                    .wrap(),
                );
                ui.add_space(4.0);
            }
            MarkdownSegment::ListItem(prefix, children) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(prefix.as_str()).color(text_color));
                    for child in children {
                        Self::render_segment(ui, child, copy_code_hint, text_color);
                    }
                });
            }
            MarkdownSegment::BlockQuote(children) => {
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
                        for child in children {
                            Self::render_segment(ui, child, copy_code_hint, text_color);
                        }
                    });
                ui.add_space(4.0);
            }
            MarkdownSegment::Link(url, label) => {
                let display = if label.is_empty() { url } else { label };
                if ui.link(display).clicked() {
                    let url = url.clone();
                    let _ = webbrowser::open(&url);
                }
            }
            MarkdownSegment::Image(url, alt) => {
                if !url.is_empty() {
                    ui.add(
                        egui::Image::new(url.as_str())
                            .max_width(400.0)
                            .max_height(400.0)
                            .alt_text(if alt.is_empty() {
                                url.clone()
                            } else {
                                alt.clone()
                            }),
                    );
                } else if !alt.is_empty() {
                    ui.label(egui::RichText::new(alt.as_str()).color(text_color));
                }
                ui.add_space(4.0);
            }
            MarkdownSegment::Raw(text) => {
                ui.add(
                    egui::Label::new(egui::RichText::new(text.as_str()).color(text_color)).wrap(),
                );
            }
        }
    }
}

/// Parse markdown text to a Vec<MarkdownSegment> on the calling thread.
/// This is the CPU-bound work that should not run on the UI thread.
fn parse_markdown_to_segments(text: &str) -> CachedMarkdownRender {
    let mut options = comrak::Options::default();
    options.extension.strikethrough = true;
    options.extension.tagfilter = true;
    options.render.hardbreaks = true;
    options.render.github_pre_lang = true;

    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, text, &options);

    let mut segments = Vec::new();
    collect_segments(&mut segments, root);
    CachedMarkdownRender { segments }
}

/// Collect markdown segments from a comrak AST node tree.
fn collect_segments<'a>(segments: &mut Vec<MarkdownSegment>, node: &'a comrak::nodes::AstNode<'a>) {
    let ast = node.data.borrow();
    match &ast.value {
        comrak::nodes::NodeValue::Document
        | comrak::nodes::NodeValue::Paragraph
        | comrak::nodes::NodeValue::Item(_) => {
            for child in node.children() {
                collect_segments(segments, child);
            }
        }
        comrak::nodes::NodeValue::Heading(h) => {
            let text = collect_text(node);
            segments.push(MarkdownSegment::Heading(h.level, text));
        }
        comrak::nodes::NodeValue::List(list) => {
            let ordered = list.list_type == comrak::nodes::ListType::Ordered;
            for (i, child) in node.children().enumerate() {
                let prefix = if ordered {
                    format!("{}. ", i + 1)
                } else {
                    "• ".to_string()
                };
                let mut children_segs = Vec::new();
                collect_segments(&mut children_segs, child);
                segments.push(MarkdownSegment::ListItem(prefix, children_segs));
            }
        }
        comrak::nodes::NodeValue::CodeBlock(info) => {
            let lang = info.info.trim().to_string();
            let code = info.literal.clone();
            segments.push(MarkdownSegment::CodeBlock(lang, code));
        }
        comrak::nodes::NodeValue::Code(..) => {
            let text = collect_text(node);
            segments.push(MarkdownSegment::InlineCode(text));
        }
        comrak::nodes::NodeValue::Strong => {
            let text = collect_text(node);
            segments.push(MarkdownSegment::Text(
                text,
                MarkdownStyle {
                    bold: true,
                    ..Default::default()
                },
            ));
        }
        comrak::nodes::NodeValue::Emph => {
            let text = collect_text(node);
            segments.push(MarkdownSegment::Text(
                text,
                MarkdownStyle {
                    italic: true,
                    ..Default::default()
                },
            ));
        }
        comrak::nodes::NodeValue::Text(ref literal) => {
            let t = literal.trim().to_string();
            if !t.is_empty() {
                segments.push(MarkdownSegment::Raw(t));
            }
        }
        comrak::nodes::NodeValue::LineBreak | comrak::nodes::NodeValue::SoftBreak => {
            // newlines handled by wrapping
        }
        comrak::nodes::NodeValue::ThematicBreak => {
            segments.push(MarkdownSegment::ThematicBreak);
        }
        comrak::nodes::NodeValue::BlockQuote => {
            let mut children = Vec::new();
            for child in node.children() {
                collect_segments(&mut children, child);
            }
            segments.push(MarkdownSegment::BlockQuote(children));
        }
        comrak::nodes::NodeValue::Link(link) => {
            let label = collect_text(node);
            segments.push(MarkdownSegment::Link(link.url.to_string(), label));
        }
        comrak::nodes::NodeValue::Image(image) => {
            let alt = collect_text(node);
            segments.push(MarkdownSegment::Image(image.url.to_string(), alt));
        }
        _ => {
            for child in node.children() {
                collect_segments(segments, child);
            }
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
