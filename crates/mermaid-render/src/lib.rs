#![recursion_limit = "256"]

//! Framework-agnostic Mermaid diagram renderer.
//!
//! Takes mermaid source text and a color theme, outputs SVG or rasterized RGBA pixels.
//! No GUI framework dependency — can be used with egui, gpui, or any other renderer.
//!
//! # Usage
//!
//! ```ignore
//! use go_on_mermaid_render::{MermaidTheme, render_to_svg, render_to_raster};
//!
//! let theme = MermaidTheme::dark(...);
//! let svg = render_to_svg("graph TD; A-->B;", &theme)?;
//! let (w, h, rgba_pixels) = render_to_raster("graph TD; A-->B;", &theme, 2.0)?;
//! ```

mod postprocess;
mod render;

use anyhow::Result;

/// RGBA color (framework-agnostic).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

/// Accent color pair for diagram nodes.
#[derive(Debug, Clone)]
pub struct AccentColor {
    pub foreground: RgbaColor,
    pub background: RgbaColor,
}

/// Theme colors for mermaid diagram rendering.
/// Maps cleanly from any GUI framework's color system.
#[derive(Debug, Clone)]
pub struct MermaidTheme {
    pub dark_mode: bool,
    pub background: RgbaColor,
    pub primary_color: RgbaColor,
    pub primary_text_color: RgbaColor,
    pub primary_border_color: RgbaColor,
    pub secondary_color: RgbaColor,
    pub tertiary_color: RgbaColor,
    pub line_color: RgbaColor,
    pub text_color: RgbaColor,
    pub edge_label_background: RgbaColor,
    pub cluster_background: RgbaColor,
    pub cluster_border: RgbaColor,
    pub note_background: RgbaColor,
    pub note_border: RgbaColor,
    pub actor_background: RgbaColor,
    pub actor_border: RgbaColor,
    pub node_backgrounds: Vec<RgbaColor>,
}

impl MermaidTheme {
    /// Build a dark theme with the given accent node colors.
    pub fn dark(
        background: RgbaColor,
        text_color: RgbaColor,
        node_backgrounds: Vec<RgbaColor>,
    ) -> Self {
        Self {
            dark_mode: true,
            background,
            primary_color: rgba_lerp(background, text_color, 0.15),
            primary_text_color: text_color,
            primary_border_color: rgba_lerp(background, text_color, 0.3),
            secondary_color: rgba_lerp(background, text_color, 0.1),
            tertiary_color: rgba_lerp(background, text_color, 0.07),
            line_color: rgba_lerp(background, text_color, 0.25),
            text_color,
            edge_label_background: rgba_lerp(background, text_color, 0.12),
            cluster_background: rgba_lerp(background, text_color, 0.05),
            cluster_border: rgba_lerp(background, text_color, 0.2),
            note_background: rgba_alpha(text_color, 30),
            note_border: rgba_lerp(background, text_color, 0.3),
            actor_background: rgba_lerp(background, text_color, 0.1),
            actor_border: rgba_lerp(background, text_color, 0.3),
            node_backgrounds,
        }
    }

    /// Build a light theme with the given accent node colors.
    pub fn light(
        background: RgbaColor,
        text_color: RgbaColor,
        node_backgrounds: Vec<RgbaColor>,
    ) -> Self {
        Self {
            dark_mode: false,
            background,
            primary_color: rgba_lerp(background, text_color, 0.1),
            primary_text_color: text_color,
            primary_border_color: rgba_lerp(background, text_color, 0.25),
            secondary_color: rgba_lerp(background, text_color, 0.05),
            tertiary_color: rgba_lerp(background, text_color, 0.03),
            line_color: rgba_lerp(background, text_color, 0.2),
            text_color,
            edge_label_background: rgba_lerp(background, text_color, 0.08),
            cluster_background: rgba_lerp(background, text_color, 0.02),
            cluster_border: rgba_lerp(background, text_color, 0.15),
            note_background: rgba_alpha(text_color, 20),
            note_border: rgba_lerp(background, text_color, 0.25),
            actor_background: rgba_lerp(background, text_color, 0.05),
            actor_border: rgba_lerp(background, text_color, 0.25),
            node_backgrounds,
        }
    }
}

/// Render a mermaid diagram to an SVG string.
///
/// The output SVG is post-processed with theme colors for a polished look.
pub fn render_to_svg(source: &str, theme: &MermaidTheme) -> Result<String> {
    let svg = render::render_mermaid(source, theme)?;
    let svg = postprocess::postprocess(&svg, theme)?;
    Ok(svg)
}

/// Render a mermaid diagram to rasterized RGBA pixels.
///
/// `scale` controls the output resolution (1.0 = 1x, 2.0 = 2x for retina).
/// Returns `(width, height, RGBA pixel data)`.
pub fn render_to_raster(
    source: &str,
    theme: &MermaidTheme,
    scale: f32,
) -> Result<(u32, u32, Vec<u8>)> {
    let svg = render_to_svg(source, theme)?;

    // Parse SVG with usvg
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg.as_bytes(), &opt)?;

    // Rasterize with resvg
    let pixmap_size = tree.size();
    let w = (pixmap_size.width() * scale).ceil() as u32;
    let h = (pixmap_size.height() * scale).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| anyhow::anyhow!("failed to create pixmap {}x{}", w, h))?;

    resvg::render(
        &tree,
        usvg::Transform::from_scale(scale, scale),
        &mut resvg::tiny_skia::PixmapMut::from_bytes(pixmap.data_mut(), w, h)
            .ok_or_else(|| anyhow::anyhow!("failed to create PixmapMut"))?,
    );

    Ok((w, h, pixmap.data().to_vec()))
}

// ── Internal helpers ──────────────────────────────────────────────

fn rgba_lerp(a: RgbaColor, b: RgbaColor, t: f32) -> RgbaColor {
    RgbaColor {
        r: lerp_u8(a.r, b.r, t),
        g: lerp_u8(a.g, b.g, t),
        b: lerp_u8(a.b, b.b, t),
        a: lerp_u8(a.a, b.a, t),
    }
}

fn rgba_alpha(c: RgbaColor, a: u8) -> RgbaColor {
    RgbaColor { a, ..c }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Format an RGBA color as a CSS hex string (`#rrggbb` when opaque,
/// `#rrggbbaa` when translucent). Shared by the SVG theme renderer and the
/// post-processor (previously duplicated in both modules).
pub(crate) fn css_hex(c: RgbaColor) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}
