use super::ChatView;
use super::CHAT_DISABLE_MARKDOWN_RENDER;

use crate::views::chat::types::{CachedMarkdownRender, MarkdownSegment, MarkdownStyle};
use go_on_mermaid_render::{render_to_raster, MermaidTheme, RgbaColor};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{OnceLock, RwLock};

/// Global cache for parsed markdown content, keyed by text hash.
/// Allows large documents to be parsed once and reused across messages and frames
/// without blocking the UI thread with comrak parsing on subsequent renders.
static MARKDOWN_CACHE: OnceLock<RwLock<HashMap<u64, CachedMarkdownRender>>> = OnceLock::new();

fn markdown_cache() -> &'static RwLock<HashMap<u64, CachedMarkdownRender>> {
    MARKDOWN_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Cached mermaid-rendered texture for reuse across frames.
struct CachedMermaid {
    color_image: egui::ColorImage,
    texture: Option<egui::TextureHandle>,
}

static MERMAID_CACHE: OnceLock<RwLock<HashMap<u64, CachedMermaid>>> = OnceLock::new();

fn mermaid_cache() -> &'static RwLock<HashMap<u64, CachedMermaid>> {
    MERMAID_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Cached math-rendered texture for reuse across frames.
struct CachedMath {
    color_image: egui::ColorImage,
    texture: Option<egui::TextureHandle>,
}

static MATH_CACHE: OnceLock<RwLock<HashMap<u64, CachedMath>>> = OnceLock::new();

fn math_cache() -> &'static RwLock<HashMap<u64, CachedMath>> {
    MATH_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
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
        if let Ok(cache) = markdown_cache().read() {
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
            if let Ok(mut cache) = markdown_cache().write() {
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
    pub(super) fn render_plain_text_fallback(
        ui: &mut egui::Ui,
        text: &str,
        text_color: egui::Color32,
    ) {
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
                // ── Mermaid diagram ────────────────────────────────
                if lang.eq_ignore_ascii_case("mermaid") && !code.trim().is_empty() {
                    render_mermaid_diagram(ui, code);
                    ui.add_space(6.0);
                    return;
                }

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
            MarkdownSegment::MathInline(svg) => {
                render_latex_diagram(ui, svg, false);
            }
            MarkdownSegment::MathDisplay(svg) => {
                render_latex_diagram(ui, svg, true);
            }
            MarkdownSegment::Raw(text) => {
                ui.add(
                    egui::Label::new(egui::RichText::new(text.as_str()).color(text_color)).wrap(),
                );
            }
        }
    }
}

/// Render a mermaid diagram code block as an egui image.
fn render_mermaid_diagram(ui: &mut egui::Ui, code: &str) {
    let hash = hash_text(code.trim());

    // Check cache for pre-rendered texture
    if let Ok(cache) = mermaid_cache().read() {
        if let Some(entry) = cache.get(&hash) {
            if let Some(ref tex) = entry.texture {
                let size = egui::vec2(
                    entry.color_image.width() as f32,
                    entry.color_image.height() as f32,
                );
                ui.add(egui::Image::from_texture((tex.id(), size)));
                return;
            }
        }
    }

    // Build theme from egui visuals
    let dark = ui.visuals().dark_mode;
    let bg = ui.visuals().panel_fill;
    let fg = ui.visuals().widgets.noninteractive.fg_stroke.color;

    let theme = if dark {
        MermaidTheme::dark(
            RgbaColor::rgba(bg.r(), bg.g(), bg.b(), bg.a()),
            RgbaColor::rgba(fg.r(), fg.g(), fg.b(), fg.a()),
            vec![
                RgbaColor::rgb(70, 130, 220),
                RgbaColor::rgb(60, 180, 120),
                RgbaColor::rgb(220, 160, 60),
                RgbaColor::rgb(200, 100, 120),
            ],
        )
    } else {
        MermaidTheme::light(
            RgbaColor::rgba(bg.r(), bg.g(), bg.b(), bg.a()),
            RgbaColor::rgba(fg.r(), fg.g(), fg.b(), fg.a()),
            vec![
                RgbaColor::rgb(50, 110, 200),
                RgbaColor::rgb(40, 160, 100),
                RgbaColor::rgb(200, 140, 40),
                RgbaColor::rgb(180, 80, 100),
            ],
        )
    };

    // Render mermaid to rasterized RGBA pixels
    let result = render_to_raster(code.trim(), &theme, 2.0);
    match result {
        Ok((w, h, rgba)) => {
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            let texture =
                ui.ctx()
                    .load_texture("mermaid", color_image.clone(), egui::TextureOptions::LINEAR);
            let size = egui::vec2(w as f32, h as f32);
            let tex_id = texture.id();

            // Store in cache (texture moved into cache, use tex_id for display)
            if let Ok(mut cache) = mermaid_cache().write() {
                cache.insert(
                    hash,
                    CachedMermaid {
                        color_image,
                        texture: Some(texture),
                    },
                );
                // Bound cache to 20 entries
                if cache.len() > 20 {
                    if let Some(key) = cache.keys().next().copied() {
                        cache.remove(&key);
                    }
                }
            }

            ui.add(egui::Image::from_texture((tex_id, size)));
        }
        Err(e) => {
            ui.colored_label(
                egui::Color32::from_rgb(200, 80, 80),
                format!("Mermaid render error: {}", e),
            );
        }
    }
}

/// Render a LaTeX math SVG as an egui image (rasterized via resvg).
fn render_latex_diagram(ui: &mut egui::Ui, svg_code: &str, is_display: bool) {
    let hash = hash_text(svg_code);

    // Check cache
    if let Ok(cache) = math_cache().read() {
        if let Some(entry) = cache.get(&hash) {
            if let Some(ref tex) = entry.texture {
                let size = egui::vec2(
                    entry.color_image.width() as f32,
                    entry.color_image.height() as f32,
                );
                if is_display {
                    ui.add_space(4.0);
                }
                ui.add(egui::Image::from_texture((tex.id(), size)));
                if is_display {
                    ui.add_space(4.0);
                } else {
                    // Inline: no extra space (inline with text)
                }
                return;
            }
        }
    }

    // Rasterize the SVG
    let result = render_svg_to_raster(svg_code, 2.0);
    match result {
        Ok((w, h, rgba)) => {
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            let texture =
                ui.ctx()
                    .load_texture("latex", color_image.clone(), egui::TextureOptions::LINEAR);
            let size = egui::vec2(w as f32, h as f32);
            let tex_id = texture.id();

            // Store in cache
            if let Ok(mut cache) = math_cache().write() {
                cache.insert(
                    hash,
                    CachedMath {
                        color_image,
                        texture: Some(texture),
                    },
                );
                if cache.len() > 50 {
                    if let Some(key) = cache.keys().next().copied() {
                        cache.remove(&key);
                    }
                }
            }

            if is_display {
                ui.add_space(4.0);
            }
            ui.add(egui::Image::from_texture((tex_id, size)));
            if is_display {
                ui.add_space(4.0);
            }
        }
        Err(e) => {
            if is_display {
                ui.add_space(4.0);
            }
            ui.colored_label(
                egui::Color32::from_rgb(200, 80, 80),
                format!("Math render error: {}", e),
            );
            if is_display {
                ui.add_space(4.0);
            }
        }
    }
}

/// Render an SVG string to a rasterized RGBA pixmap using resvg.
fn render_svg_to_raster(svg: &str, scale: f32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg.as_bytes(), &opt)?;

    let pixmap_size = tree.size();
    let w = (pixmap_size.width() * scale).ceil() as u32;
    let h = (pixmap_size.height() * scale).ceil() as u32;

    let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1))
        .ok_or_else(|| anyhow::anyhow!("failed to create pixmap {}x{}", w, h))?;

    resvg::render(
        &tree,
        usvg::Transform::from_scale(scale, scale),
        &mut resvg::tiny_skia::PixmapMut::from_bytes(pixmap.data_mut(), w.max(1), h.max(1))
            .ok_or_else(|| anyhow::anyhow!("failed to create PixmapMut"))?,
    );

    Ok((w, h, pixmap.data().to_vec()))
}

/// Parse markdown text to a Vec<MarkdownSegment> on the calling thread.
/// This is the CPU-bound work that should not run on the UI thread.
/// Math expressions (`$...$` and `$$...$$`) are preprocessed into Math segments.
fn parse_markdown_to_segments(text: &str) -> CachedMarkdownRender {
    // ── Step 1: Extract and replace math expressions with placeholders ──
    let expressions = go_on_latex::extract_math_expressions(text);

    if expressions.is_empty() {
        // Fast path: no math — parse normally
        return CachedMarkdownRender {
            segments: parse_markdown_to_segments_inner(text),
        };
    }

    // Build placeholder-replaced text and collect rendered SVGs
    let mut processed = String::with_capacity(text.len());
    let mut last_end = 0;

    let mut math_entries: Vec<MathSvgEntry> = Vec::new();

    for (idx, expr) in expressions.iter().enumerate() {
        // Append text before this expression
        processed.push_str(&text[last_end..expr.start]);

        // Render the LaTeX to SVG
        let svg = match go_on_latex::render_to_svg(&expr.content, expr.display_mode) {
            Ok(s) => s,
            Err(_) => {
                // Fallback: keep the original LaTeX
                processed.push_str(&text[expr.start..expr.end]);
                last_end = expr.end;
                continue;
            }
        };

        math_entries.push(MathSvgEntry {
            svg,
            display_mode: expr.display_mode,
        });

        // Replace with placeholder
        processed.push_str(&format!("\x00MATH_{idx}_\x00"));
        last_end = expr.end;
    }

    // ── Step 2: Parse the placeholder text with comrak ──
    let segments = parse_markdown_to_segments_inner(&processed);

    // ── Step 3: Post-process segments — replace placeholders with Math segments ──
    let segments = postprocess_math_segments(segments, &math_entries);

    CachedMarkdownRender { segments }
}

/// An entry for a pre-rendered math expression.
struct MathSvgEntry {
    svg: String,
    display_mode: bool,
}

/// Inner parsing function — does the actual comrak parsing without math preprocessing.
fn parse_markdown_to_segments_inner(text: &str) -> Vec<MarkdownSegment> {
    let mut options = comrak::Options::default();
    options.extension.strikethrough = true;
    options.extension.tagfilter = true;
    options.render.hardbreaks = true;
    options.render.github_pre_lang = true;

    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, text, &options);

    let mut segments = Vec::new();
    collect_segments(&mut segments, root);
    segments
}

/// Post-process segments to replace math placeholders with `MathInline`/`MathDisplay` segments.
/// Returns a new Vec with placeholders replaced.
fn postprocess_math_segments(
    segments: Vec<MarkdownSegment>,
    math_entries: &[MathSvgEntry],
) -> Vec<MarkdownSegment> {
    if math_entries.is_empty() {
        return segments;
    }

    let mut result = Vec::with_capacity(segments.len());

    for segment in segments {
        match segment {
            MarkdownSegment::Raw(ref text) | MarkdownSegment::Text(ref text, _) => {
                if text.contains("\x00MATH_") {
                    let (new_segs, contains_math) = split_math_placeholders(text, math_entries);
                    if contains_math {
                        result.extend(new_segs);
                    } else {
                        result.push(segment);
                    }
                } else {
                    result.push(segment);
                }
            }
            MarkdownSegment::Heading(level, text) => {
                if text.contains("\x00MATH_") {
                    let (new_segs, _) = split_math_placeholders(&text, math_entries);
                    // For headings with math, just emit the segments in order
                    result.extend(new_segs);
                } else {
                    result.push(MarkdownSegment::Heading(level, text));
                }
            }
            MarkdownSegment::ListItem(prefix, children) => {
                let new_prefix = if prefix.contains("\x00MATH_") {
                    let (new_prefix_segs, _) = split_math_placeholders(&prefix, math_entries);
                    new_prefix_segs
                        .iter()
                        .map(|s| match s {
                            MarkdownSegment::Raw(t) => t.clone(),
                            MarkdownSegment::MathInline(_) | MarkdownSegment::MathDisplay(_) => {
                                "[math]".to_string()
                            }
                            _ => prefix.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join("")
                } else {
                    prefix
                };
                let children = postprocess_math_segments(children, math_entries);
                result.push(MarkdownSegment::ListItem(new_prefix, children));
            }
            MarkdownSegment::BlockQuote(children) => {
                let children = postprocess_math_segments(children, math_entries);
                result.push(MarkdownSegment::BlockQuote(children));
            }
            other => {
                result.push(other);
            }
        }
    }

    result
}

/// Split a text containing math placeholders into a sequence of segments.
/// Returns `(Vec<MarkdownSegment>, bool)` where the bool indicates whether
/// any placeholders were found.
fn split_math_placeholders(
    text: &str,
    math_entries: &[MathSvgEntry],
) -> (Vec<MarkdownSegment>, bool) {
    let mut result = Vec::new();
    let mut last_end = 0;
    let mut found = false;

    let placeholder_pattern = "\x00MATH_";
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(placeholder_pattern) {
        let abs_pos = search_start + pos;

        // Find the closing marker _\x00
        if let Some(close_pos) = text[abs_pos..].find("_\x00") {
            let abs_end = abs_pos + close_pos + 3;

            // Text before the placeholder
            if abs_pos > last_end {
                result.push(MarkdownSegment::Raw(text[last_end..abs_pos].to_string()));
            }

            // Extract the index
            let num_start = abs_pos + placeholder_pattern.len();
            let num_str = &text[num_start..abs_pos + close_pos];
            if let Ok(idx) = num_str.parse::<usize>() {
                if idx < math_entries.len() {
                    let entry = &math_entries[idx];
                    let seg = if entry.display_mode {
                        MarkdownSegment::MathDisplay(entry.svg.clone())
                    } else {
                        MarkdownSegment::MathInline(entry.svg.clone())
                    };
                    result.push(seg);
                    found = true;
                }
            }

            last_end = abs_end;
            search_start = abs_end;
        } else {
            break;
        }
    }

    // Remaining text after last placeholder
    if last_end < text.len() {
        result.push(MarkdownSegment::Raw(text[last_end..].to_string()));
    }

    if !found {
        result.push(MarkdownSegment::Raw(text.to_string()));
    }

    (result, found)
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
