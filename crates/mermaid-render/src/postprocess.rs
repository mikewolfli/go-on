//! Post-process merman-generated SVG to apply theme colors and fix
//! compatibility issues for rasterization.
//!
//! Uses streaming XML parsing via quick-xml to avoid multiple tree builds.

use anyhow::{Context, Result};
use quick_xml::events::{BytesDecl, BytesText, Event};
use quick_xml::Reader;
use quick_xml::Writer;

use crate::{MermaidTheme, RgbaColor};

/// Run the full post-processing pipeline on a merman-generated SVG.
pub(super) fn postprocess(svg: &str, theme: &MermaidTheme) -> Result<String> {
    let mut reader = Reader::from_str(svg);
    reader.config_mut().check_end_names = false;

    let mut writer = Writer::new(Vec::with_capacity(svg.len()));

    let mut in_style = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = e.name().as_ref().to_vec();
                let tag = std::str::from_utf8(&name).unwrap_or("");
                if tag == "style" {
                    in_style = true;
                }
                if tag == "svg" {
                    let mut elem = e.to_owned();
                    let bg = css_hex(theme.background);
                    elem.push_attribute(("style", format!("background-color:{}", bg).as_str()));
                    writer.write_event(Event::Start(elem)).ok();
                } else {
                    writer.write_event(Event::Start(e.to_owned())).ok();
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.name().as_ref().to_vec();
                let tag = std::str::from_utf8(&name).unwrap_or("");
                if tag == "style" {
                    in_style = false;
                }
                writer.write_event(Event::End(e.to_owned())).ok();
            }
            Ok(Event::Text(ref e)) => {
                if in_style {
                    let css = std::str::from_utf8(e.as_ref()).unwrap_or("");
                    let mut patched = String::with_capacity(css.len() + 512);
                    patched.push_str(css);
                    patched.push_str(&theme_css_overrides(theme));
                    writer
                        .write_event(Event::Text(BytesText::new(&patched)))
                        .ok();
                } else {
                    writer.write_event(Event::Text(e.to_owned())).ok();
                }
            }
            Ok(Event::Comment(_)) => {} // strip comments
            Ok(Event::Decl(_)) => {
                writer
                    .write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))
                    .ok();
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("XML parse error: {}", e),
            _ => {}
        }
    }

    String::from_utf8(writer.into_inner()).context("SVG output is not valid UTF-8")
}

fn theme_css_overrides(theme: &MermaidTheme) -> String {
    let mut css = String::new();
    css.push_str(&format!(
        ".label, .nodeLabel, .edgeLabel {{ fill: {} !important; }}\n",
        css_hex(theme.text_color)
    ));
    css.push_str(&format!(
        ".node rect, .node circle, .node polygon, .node path {{ stroke: {} !important; }}\n",
        css_hex(theme.primary_border_color)
    ));
    css.push_str(&format!(
        ".edgePath .path {{ stroke: {} !important; }}\n",
        css_hex(theme.line_color)
    ));
    css.push_str(&format!(
        ".marker {{ fill: {} !important; stroke: {} !important; }}\n",
        css_hex(theme.line_color),
        css_hex(theme.line_color)
    ));
    // Inject accent node background colors
    for (i, accent) in theme.node_backgrounds.iter().enumerate() {
        css.push_str(&format!(
            ".zed-accent-{} {{ fill: {} !important; }}\n",
            i,
            css_hex(*accent)
        ));
        css.push_str(&format!(
            ".zed-accent-{} text {{ fill: {} !important; }}\n",
            i,
            text_color_for_bg(*accent)
        ));
    }
    css
}

fn css_hex(c: RgbaColor) -> String {
    crate::css_hex(c)
}

/// Compute a legible text color (black or white) for a given background.
fn text_color_for_bg(bg: RgbaColor) -> String {
    let lum = 0.2126 * (bg.r as f32 / 255.0)
        + 0.7152 * (bg.g as f32 / 255.0)
        + 0.0722 * (bg.b as f32 / 255.0);
    if lum > 0.5 {
        "#000000".to_string()
    } else {
        "#ffffff".to_string()
    }
}
