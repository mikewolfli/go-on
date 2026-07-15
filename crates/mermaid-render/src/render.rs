//! Mermaid rendering via the `merman` crate.
//!
//! Converts mermaid source text to SVG using merman's headless renderer,
//! using the resvg-safe pipeline for rasterizer compatibility.

use anyhow::{Context, Result};
use merman::render::HeadlessRenderer;
use serde_json::Value;

use crate::MermaidTheme;

/// Render mermaid source to SVG string using merman.
pub(super) fn render_mermaid(source: &str, theme: &MermaidTheme) -> Result<String> {
    let config = to_merman_config(theme);

    let renderer = HeadlessRenderer::new()
        .with_diagram_id("merman")
        .with_site_config(config);

    let svg = renderer
        .render_svg_resvg_safe_sync(source)
        .context("merman render failed")?
        .ok_or_else(|| anyhow::anyhow!("merman returned no SVG"))?;

    Ok(svg)
}

fn to_merman_config(theme: &MermaidTheme) -> merman::MermaidConfig {
    let mut vars = serde_json::Map::new();
    let mut set = |k: &str, v: &str| {
        vars.insert(k.to_string(), Value::String(v.to_string()));
    };
    set("primaryColor", &css_hex(theme.primary_color));
    set("primaryTextColor", &css_hex(theme.primary_text_color));
    set("primaryBorderColor", &css_hex(theme.primary_border_color));
    set("secondaryColor", &css_hex(theme.secondary_color));
    set("tertiaryColor", &css_hex(theme.tertiary_color));
    set("lineColor", &css_hex(theme.line_color));
    set("textColor", &css_hex(theme.text_color));
    set("mainBkg", &css_hex(theme.primary_color));
    set("nodeBorder", &css_hex(theme.primary_border_color));
    set("clusterBkg", &css_hex(theme.cluster_background));
    set("clusterBorder", &css_hex(theme.cluster_border));
    set("edgeLabelBackground", &css_hex(theme.edge_label_background));
    set("noteBkgColor", &css_hex(theme.note_background));
    set("noteBorderColor", &css_hex(theme.note_border));
    set("actorBkg", &css_hex(theme.actor_background));
    set("actorBorder", &css_hex(theme.actor_border));
    set("actorTextColor", &css_hex(theme.primary_text_color));
    set("signalColor", &css_hex(theme.line_color));
    set("signalTextColor", &css_hex(theme.text_color));
    set("labelBoxBkgColor", &css_hex(theme.secondary_color));
    set("labelBoxBorderColor", &css_hex(theme.primary_border_color));
    set("labelTextColor", &css_hex(theme.text_color));
    set("loopTextColor", &css_hex(theme.text_color));
    set(
        "activationBorderColor",
        &css_hex(theme.primary_border_color),
    );
    set("activationBkgColor", &css_hex(theme.secondary_color));
    set("sequenceNumberColor", &css_hex(theme.text_color));
    set("sectionBkgColor", &css_hex(theme.secondary_color));
    set("altSectionBkgColor", &css_hex(theme.tertiary_color));
    set("taskBorderColor", &css_hex(theme.primary_border_color));
    set("taskBkgColor", &css_hex(theme.primary_color));
    set("taskTextColor", &css_hex(theme.primary_text_color));
    set("taskTextOutsideColor", &css_hex(theme.text_color));
    set("todayLineColor", &css_hex(theme.line_color));
    set("critBorderColor", &css_hex(theme.primary_border_color));
    set("critBkgColor", &css_hex(theme.secondary_color));
    set("background", &css_hex(theme.background));

    let mut root = serde_json::Map::new();
    root.insert("theme".to_string(), Value::String("base".to_string()));
    root.insert("darkMode".to_string(), Value::Bool(theme.dark_mode));
    root.insert(
        "fontFamily".to_string(),
        Value::String("sans-serif".to_string()),
    );
    root.insert("htmlLabels".to_string(), Value::Bool(false));
    root.insert(
        "flowchart".to_string(),
        serde_json::json!({"htmlLabels": false, "padding": 16}),
    );
    root.insert("themeVariables".to_string(), Value::Object(vars));

    merman::MermaidConfig::from_value(Value::Object(root))
}

fn css_hex(c: crate::RgbaColor) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}
