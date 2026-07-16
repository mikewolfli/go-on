//! SVG renderer for LaTeX math AST nodes.
//!
//! Converts a `MathNode` tree into an SVG string.
//! Handles: text, superscript, subscript, fractions, sqrt, sum, Greek letters, operators.

use crate::MathNode;

/// Base font size for rendering (in pixels).
const BASE_FONT_SIZE: f32 = 18.0;

/// Font size for superscript/subscript.
const SCRIPT_FONT_SIZE: f32 = 13.0;

/// Font size for fractions.
const FRACTION_FONT_SIZE: f32 = 16.0;

/// Layout information for a rendered math node.
#[derive(Debug, Clone)]
struct LayoutBox {
    /// SVG elements (text, lines, rects) to render
    elements: Vec<SvgElement>,
    /// Width of the bounding box
    width: f32,
    /// Height of the bounding box
    height: f32,
    /// Y-offset from top to the baseline
    baseline: f32,
}

/// An SVG element with positioning.
#[derive(Debug, Clone)]
enum SvgElement {
    Text {
        x: f32,
        y: f32,
        content: String,
        font_size: f32,
        font_style: FontStyle,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        stroke_width: f32,
    },
    #[allow(dead_code)]
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        rx: f32,
    },
    Group {
        elements: Vec<SvgElement>,
        transform: Option<(f32, f32)>, // translate(x, y)
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FontStyle {
    Normal,
    Italic,
    #[allow(dead_code)]
    Bold,
}

/// Estimate the width of a text string in pixels for the given font size.
/// Uses a rough monospace/math approximation. For a production renderer,
/// this would use font metrics; for our purpose, a character-width heuristic suffices.
fn estimate_text_width(text: &str, font_size: f32) -> f32 {
    let char_width = font_size * 0.55; // average character width
    let wide_chars: &[char] = &['W', 'M', 'm', '\u{03A3}', '\u{222B}', '\u{221E}'];
    let narrow_chars: &[char] = &['i', 'l', 'I', '1', 't', ',', '.', ';', ':', '\''];
    let mut width = 0.0;
    for c in text.chars() {
        if c.is_ascii_digit() {
            width += char_width * 0.85;
        } else if c == ' ' {
            width += char_width * 0.6;
        } else if wide_chars.contains(&c) {
            width += char_width * 1.3;
        } else if narrow_chars.contains(&c) {
            width += char_width * 0.6;
        } else {
            width += char_width;
        }
    }
    width.max(font_size * 0.3)
}

/// Map a Greek letter command name to its Unicode character.
fn greek_char(name: &str) -> &str {
    match name {
        "alpha" => "\u{03B1}",
        "beta" => "\u{03B2}",
        "gamma" => "\u{03B3}",
        "delta" => "\u{03B4}",
        "epsilon" => "\u{03B5}",
        "zeta" => "\u{03B6}",
        "eta" => "\u{03B7}",
        "theta" => "\u{03B8}",
        "iota" => "\u{03B9}",
        "kappa" => "\u{03BA}",
        "lambda" => "\u{03BB}",
        "mu" => "\u{03BC}",
        "nu" => "\u{03BD}",
        "xi" => "\u{03BE}",
        "omicron" => "\u{03BF}",
        "pi" => "\u{03C0}",
        "rho" => "\u{03C1}",
        "sigma" => "\u{03C3}",
        "tau" => "\u{03C4}",
        "upsilon" => "\u{03C5}",
        "phi" => "\u{03C6}",
        "chi" => "\u{03C7}",
        "psi" => "\u{03C8}",
        "omega" => "\u{03C9}",
        "Alpha" => "\u{0391}",
        "Beta" => "\u{0392}",
        "Gamma" => "\u{0393}",
        "Delta" => "\u{0394}",
        "Epsilon" => "\u{0395}",
        "Zeta" => "\u{0396}",
        "Eta" => "\u{0397}",
        "Theta" => "\u{0398}",
        "Iota" => "\u{0399}",
        "Kappa" => "\u{039A}",
        "Lambda" => "\u{039B}",
        "Mu" => "\u{039C}",
        "Nu" => "\u{039D}",
        "Xi" => "\u{039E}",
        "Omicron" => "\u{039F}",
        "Pi" => "\u{03A0}",
        "Rho" => "\u{03A1}",
        "Sigma" => "\u{03A3}",
        "Tau" => "\u{03A4}",
        "Upsilon" => "\u{03A5}",
        "Phi" => "\u{03A6}",
        "Chi" => "\u{03A7}",
        "Psi" => "\u{03A8}",
        "Omega" => "\u{03A9}",
        _ => name,
    }
}

/// Layout a single math node and return its bounding box + SVG elements.
fn layout_node(node: &MathNode, font_size: f32) -> LayoutBox {
    match node {
        MathNode::Text(text) => {
            let w = estimate_text_width(text, font_size);
            LayoutBox {
                elements: vec![SvgElement::Text {
                    x: 0.0,
                    y: 0.0,
                    content: text.clone(),
                    font_size,
                    font_style: FontStyle::Normal,
                }],
                width: w,
                height: font_size * 1.3,
                baseline: font_size * 0.85,
            }
        }
        MathNode::GreekChar(name) => {
            let ch = greek_char(name);
            let w = estimate_text_width(ch, font_size);
            LayoutBox {
                elements: vec![SvgElement::Text {
                    x: 0.0,
                    y: 0.0,
                    content: ch.to_string(),
                    font_size,
                    font_style: FontStyle::Italic,
                }],
                width: w,
                height: font_size * 1.3,
                baseline: font_size * 0.85,
            }
        }
        MathNode::Operator(op) => {
            let w = estimate_text_width(op, font_size);
            LayoutBox {
                elements: vec![SvgElement::Text {
                    x: 0.0,
                    y: 0.0,
                    content: op.clone(),
                    font_size,
                    font_style: FontStyle::Normal,
                }],
                width: w,
                height: font_size * 1.3,
                baseline: font_size * 0.85,
            }
        }
        MathNode::Group(nodes) => layout_horizontal(nodes, font_size),
        MathNode::SubSup { base, sup, sub } => {
            let base_box = layout_node(base, font_size);
            let mut elements = Vec::new();
            let mut total_w = base_box.width;
            let mut total_h = base_box.height;
            let baseline = base_box.baseline;

            // Position base
            elements.push(SvgElement::Group {
                elements: base_box.elements,
                transform: Some((0.0, 0.0)),
            });

            // Superscript (positioned above-right of base)
            if let Some(sup_node) = sup {
                let sup_box = layout_node(sup_node, SCRIPT_FONT_SIZE);
                let sup_x = base_box.width + 1.0;
                let sup_y = -(base_box.baseline * 0.6);
                elements.push(SvgElement::Group {
                    elements: sup_box.elements,
                    transform: Some((sup_x, sup_y)),
                });
                total_w = total_w.max(base_box.width + sup_box.width + 1.0);
                total_h = total_h.max(base_box.baseline - sup_y + 2.0);
            }

            // Subscript (positioned below-right of base)
            if let Some(sub_node) = sub {
                let sub_box = layout_node(sub_node, SCRIPT_FONT_SIZE);
                let sub_x = if sup.is_some() {
                    // If both sup and sub exist, align both to the right of the base
                    base_box.width + 1.0
                } else {
                    base_box.width + 1.0
                };
                let sub_y = baseline * 0.5;
                elements.push(SvgElement::Group {
                    elements: sub_box.elements,
                    transform: Some((sub_x, sub_y)),
                });
                total_w = total_w.max(base_box.width + sub_box.width + 1.0);
                let sub_bottom = sub_y + sub_box.height;
                if sub_bottom > total_h {
                    total_h = sub_bottom;
                }
            }

            LayoutBox {
                elements,
                width: total_w,
                height: total_h,
                baseline,
            }
        }
        MathNode::Fraction(num, den) => {
            let num_box = layout_node(num, FRACTION_FONT_SIZE);
            let den_box = layout_node(den, FRACTION_FONT_SIZE);
            let gap = 4.0;
            let line_thickness = 1.5;
            let max_w = num_box.width.max(den_box.width) + 6.0;
            let num_y = 0.0;
            let line_y = num_box.height + gap;
            let den_y = line_y + line_thickness + gap;
            let total_w = max_w;
            let total_h = den_y + den_box.height;
            let center_x = max_w / 2.0;

            let mut elements = Vec::new();

            // Numerator (centered above line)
            let num_offset_x = center_x - num_box.width / 2.0;
            elements.push(SvgElement::Group {
                elements: num_box.elements,
                transform: Some((num_offset_x, num_y)),
            });

            // Fraction line
            elements.push(SvgElement::Line {
                x1: center_x - max_w / 2.0 + 3.0,
                y1: line_y,
                x2: center_x + max_w / 2.0 - 3.0,
                y2: line_y,
                stroke_width: line_thickness,
            });

            // Denominator (centered below line)
            let den_offset_x = center_x - den_box.width / 2.0;
            elements.push(SvgElement::Group {
                elements: den_box.elements,
                transform: Some((den_offset_x, den_y)),
            });

            LayoutBox {
                elements,
                width: total_w,
                height: total_h,
                baseline: num_box.height + gap + line_thickness,
            }
        }
        MathNode::Sqrt(inner) => {
            let inner_box = layout_node(inner, font_size);
            let sqrt_gap = 3.0;
            let tick_height = font_size * 0.8;
            let tick_width = font_size * 0.4;
            let total_w = tick_width + sqrt_gap + inner_box.width;
            let total_h = (inner_box.height + 4.0).max(tick_height + 2.0);
            let bar_y = total_h - 1.5; // horizontal bar position
            let bar_thickness = 1.5;

            let mut elements = Vec::new();

            // Radical sign: a simple tick + horizontal line representation
            // We draw a simplified sqrt sign using lines
            // Top-left corner tick:
            //  (0, bar_y) -> (3, bar_y - tick_height) -> (tick_width, bar_y)
            elements.push(SvgElement::Line {
                x1: 0.0,
                y1: bar_y,
                x2: 3.0,
                y2: bar_y - tick_height,
                stroke_width: bar_thickness,
            });
            elements.push(SvgElement::Line {
                x1: 3.0,
                y1: bar_y - tick_height,
                x2: tick_width,
                y2: bar_y,
                stroke_width: bar_thickness,
            });

            // Horizontal bar over the radicand
            let bar_x = tick_width + sqrt_gap;
            elements.push(SvgElement::Line {
                x1: bar_x - 2.0,
                y1: bar_y,
                x2: bar_x + inner_box.width + 2.0,
                y2: bar_y,
                stroke_width: bar_thickness,
            });

            // Radicand content
            elements.push(SvgElement::Group {
                elements: inner_box.elements,
                transform: Some((bar_x, 2.0)),
            });

            LayoutBox {
                elements,
                width: total_w,
                height: total_h,
                baseline: total_h - 2.0,
            }
        }
        MathNode::Sum(inner, sup, sub) => {
            let sum_char = "\u{2211}"; // ∑
            let sum_w = estimate_text_width(sum_char, font_size * 1.3);
            let sum_h = font_size * 1.5;
            let mut elements = Vec::new();
            let mut total_w = sum_w;
            let mut total_h = sum_h;

            // The sum symbol
            elements.push(SvgElement::Text {
                x: 0.0,
                y: sum_h * 0.7,
                content: sum_char.to_string(),
                font_size: font_size * 1.3,
                font_style: FontStyle::Normal,
            });

            // Inner expression (to the right of the sum)
            let inner_box = layout_node(inner, font_size);
            let inner_x = sum_w + 3.0;
            elements.push(SvgElement::Group {
                elements: inner_box.elements,
                transform: Some((inner_x, sum_h * 0.25)),
            });

            if inner_box.width > 0.0 {
                total_w = inner_x + inner_box.width;
                total_h = total_h.max(inner_box.height + sum_h * 0.25);
            }

            // Subscript (below sum symbol)
            if let Some(sub_node) = sub {
                let sub_box = layout_node(sub_node, SCRIPT_FONT_SIZE);
                let sub_x = -2.0;
                let sub_y = sum_h * 0.85;
                elements.push(SvgElement::Group {
                    elements: sub_box.elements,
                    transform: Some((sub_x, sub_y)),
                });
                let sub_right = sub_x + sub_box.width;
                if sub_right > total_w {
                    total_w = sub_right;
                }
                let sub_bottom = sub_y + sub_box.height;
                if sub_bottom > total_h {
                    total_h = sub_bottom;
                }
            }

            // Superscript (above sum symbol)
            if let Some(sup_node) = sup {
                let sup_box = layout_node(sup_node, SCRIPT_FONT_SIZE);
                let sup_x = -2.0;
                let sup_y = -sup_box.height * 0.3;
                elements.push(SvgElement::Group {
                    elements: sup_box.elements,
                    transform: Some((sup_x, sup_y)),
                });
                let sup_right = sup_x + sup_box.width;
                if sup_right > total_w {
                    total_w = sup_right;
                }
            }

            LayoutBox {
                elements,
                width: total_w,
                height: total_h,
                baseline: sum_h * 0.7,
            }
        }
    }
}

/// Layout a sequence of nodes horizontally.
fn layout_horizontal(nodes: &[MathNode], font_size: f32) -> LayoutBox {
    if nodes.is_empty() {
        return LayoutBox {
            elements: vec![],
            width: 0.0,
            height: font_size * 1.3,
            baseline: font_size * 0.85,
        };
    }

    let mut elements = Vec::new();
    let mut x_offset = 0.0;
    let mut max_h = 0.0;
    let mut baseline = font_size * 0.85;

    for node in nodes {
        let box_ = layout_node(node, font_size);
        // Add spacing between elements
        if x_offset > 0.0 {
            x_offset += 2.0; // inter-element spacing
        }
        // Check if this is a binary operator (+, -, =, etc.)
        let is_operator = matches!(node, MathNode::Operator(_))
            || matches!(node, MathNode::Text(s) if s.len() == 1 && matches!(s.chars().next(), Some('+' | '-' | '=' | '<' | '>' | '/' | '(' | ')' | '[' | ']')));

        // Add extra spacing around operators
        if is_operator && x_offset > 0.0 {
            x_offset += 2.0;
        }

        elements.push(SvgElement::Group {
            elements: box_.elements,
            transform: Some((x_offset, 0.0)),
        });

        x_offset += box_.width;
        if box_.height > max_h {
            max_h = box_.height;
        }
        if box_.baseline > baseline {
            baseline = box_.baseline;
        }

        if is_operator {
            x_offset += 2.0;
        }
    }

    // Adjust all elements to shift baseline to y=font_size*0.85
    let shift_y = font_size * 0.85;
    let wrapped_elements = vec![SvgElement::Group {
        elements,
        transform: Some((0.0, shift_y)),
    }];

    LayoutBox {
        elements: wrapped_elements,
        width: x_offset,
        height: max_h.max(baseline + 2.0),
        baseline,
    }
}

/// Convert a LayoutBox into an SVG string.
fn layout_to_svg(layout: &LayoutBox, display_mode: bool) -> String {
    let pad = 4.0;
    let svg_w = layout.width + pad * 2.0;
    let svg_h = layout.height + pad * 2.0;

    let mut svg = String::new();
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {:.1} {:.1}" width="{:.1}" height="{:.1}" {}>"#,
        svg_w, svg_h, svg_w, svg_h,
        if display_mode {
            "display=\"block\" style=\"margin: 0.5em auto;\""
        } else {
            "display=\"inline-block\" style=\"vertical-align: middle;\""
        }
    ));
    svg.push_str(r#"<rect width="100%" height="100%" fill="transparent"/>"#);

    // Add a style block for math fonts
    svg.push_str(
        r#"<style>
.math-text { font-family: 'Latin Modern Math', 'STIX Two Math', 'Cambria Math', serif; }
.math-italic { font-family: 'Latin Modern Math', 'STIX Two Math', 'Cambria Math', serif; font-style: italic; }
.math-op { font-family: 'Latin Modern Math', 'STIX Two Math', 'Cambria Math', serif; }
</style>"#,
    );

    // Render elements
    svg.push_str(&render_elements(&layout.elements, pad, pad));

    svg.push_str("</svg>");
    svg
}

/// Recursively render SVG elements into a string.
fn render_elements(elements: &[SvgElement], offset_x: f32, offset_y: f32) -> String {
    let mut out = String::new();
    for el in elements {
        match el {
            SvgElement::Text {
                x,
                y,
                content,
                font_size,
                font_style,
            } => {
                let css_class = match font_style {
                    FontStyle::Italic => "math-italic",
                    _ => "math-text",
                };
                let escaped = content
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;");
                out.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" font-size="{:.1}" class="{}">{}</text>"#,
                    offset_x + x,
                    offset_y + y,
                    font_size,
                    css_class,
                    escaped,
                ));
            }
            SvgElement::Line {
                x1,
                y1,
                x2,
                y2,
                stroke_width,
            } => {
                out.push_str(&format!(
                    r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="currentColor" stroke-width="{:.1}" stroke-linecap="round"/>"#,
                    offset_x + x1, offset_y + y1, offset_x + x2, offset_y + y2, stroke_width,
                ));
            }
            SvgElement::Rect {
                x,
                y,
                width,
                height,
                rx,
            } => {
                out.push_str(&format!(
                    r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}" fill="currentColor" opacity="0.15"/>"#,
                    offset_x + x, offset_y + y, width, height, rx,
                ));
            }
            SvgElement::Group {
                elements,
                transform,
            } => {
                let (tx, ty) = transform.unwrap_or((0.0, 0.0));
                out.push_str(&render_elements(elements, offset_x + tx, offset_y + ty));
            }
        }
    }
    out
}

/// Render a parsed LaTeX math expression to an SVG string.
///
/// `nodes` is the parsed AST (from `parser::parse()`).
/// `display_mode` controls whether the SVG is block-level or inline.
pub fn render_to_svg(nodes: &[MathNode], display_mode: bool) -> String {
    if nodes.is_empty() {
        // Return an invisible zero-width SVG
        return r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 0 0" width="0" height="0"></svg>"#.to_string();
    }

    let layout = if nodes.len() == 1 {
        layout_node(&nodes[0], BASE_FONT_SIZE)
    } else {
        let group = MathNode::Group(nodes.to_vec());
        layout_node(&group, BASE_FONT_SIZE)
    };

    layout_to_svg(&layout, display_mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    #[test]
    fn test_render_simple_text() {
        let nodes = parse("x + y = z");
        let svg = render_to_svg(&nodes, false);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("x"));
        assert!(svg.contains("+"));
    }

    #[test]
    fn test_render_superscript() {
        let nodes = parse("x^2");
        let svg = render_to_svg(&nodes, false);
        assert!(svg.contains("x"));
        assert!(svg.contains("2"));
    }

    #[test]
    fn test_render_fraction() {
        let nodes = parse("\\frac{1}{2}");
        let svg = render_to_svg(&nodes, false);
        assert!(svg.contains("1"));
        assert!(svg.contains("2"));
        assert!(svg.contains("<line"));
    }

    #[test]
    fn test_render_sqrt() {
        let nodes = parse("\\sqrt{x}");
        let svg = render_to_svg(&nodes, false);
        assert!(svg.contains("x"));
        assert!(svg.contains("<line"));
    }

    #[test]
    fn test_render_display_mode() {
        let nodes = parse("\\sum_{i=0}^{n} x_i");
        let svg = render_to_svg(&nodes, true);
        assert!(svg.contains("display=\"block\""));
    }

    #[test]
    fn test_render_greek() {
        let nodes = parse("\\alpha + \\beta = \\gamma");
        let svg = render_to_svg(&nodes, false);
        assert!(svg.contains("\u{03B1}"));
        assert!(svg.contains("\u{03B2}"));
        assert!(svg.contains("\u{03B3}"));
    }
}
