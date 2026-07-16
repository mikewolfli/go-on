//! # go-on-latex
//!
//! LaTeX math rendering for go-on — converts LaTeX math expressions to SVG.
//!
//! ## Usage
//!
//! ```ignore
//! use go_on_latex::{render_to_svg, extract_math_expressions, render_math_in_text};
//!
//! // Render a standalone LaTeX expression
//! let svg = render_to_svg("E = mc^2", false)?;
//!
//! // Extract math expressions from text
//! let exprs = extract_math_expressions("Inline $x^2$ and display $$\\frac{1}{2}$$");
//! assert_eq!(exprs.len(), 2);
//!
//! // Render markdown text with math expressions replaced by SVG
//! let result = render_math_in_text("The formula $E = mc^2$ is famous.")?;
//! ```
//!
//! ## Supported LaTeX constructs
//!
//! - Text: letters, digits, operators (`+`, `-`, `=`, etc.)
//! - `^` — superscript (e.g., `x^2`)
//! - `_` — subscript (e.g., `x_i`, `x_i^2`)
//! - `\frac{a}{b}` — fractions
//! - `\sqrt{x}` — square root
//! - `\sum_{i=0}^{n}` — summation
//! - Greek letters: `\alpha`, `\beta`, `\theta`, `\pi`, etc.
//! - Operators: `\times`, `\div`, `\pm`, `\cdot`, `\to`, `\infty`, etc.

pub mod parser;
pub mod renderer;

use anyhow::Result;

/// A detected LaTeX math expression within a larger text.
#[derive(Debug, Clone)]
pub struct MathExpression {
    /// The LaTeX content (without delimiters)
    pub content: String,
    /// Whether this is block math (`$$...$$`) vs inline (`$...$`)
    pub display_mode: bool,
    /// Start byte offset of the opening delimiter in the original text
    pub start: usize,
    /// End byte offset (exclusive) after the closing delimiter
    pub end: usize,
}

/// The AST node types for parsed LaTeX math expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum MathNode {
    /// Plain text
    Text(String),
    /// A group of nodes (from `{...}`)
    Group(Vec<MathNode>),
    /// An element with optional superscript and/or subscript
    SubSup {
        base: Box<MathNode>,
        sup: Option<Box<MathNode>>,
        sub: Option<Box<MathNode>>,
    },
    /// Fraction: numerator and denominator
    Fraction(Box<MathNode>, Box<MathNode>),
    /// Square root (with optional degree encoded inside)
    Sqrt(Box<MathNode>),
    /// Summation with optional superscript and subscript
    Sum(Box<MathNode>, Option<Box<MathNode>>, Option<Box<MathNode>>),
    /// A Greek letter (command name like "alpha", "beta", etc.)
    GreekChar(String),
    /// An operator symbol (like `\times`, `\to`, or rendered unicode)
    Operator(String),
}

/// Render a LaTeX math expression directly to an SVG string.
///
/// This function parses the LaTeX and renders it to SVG in one step.
///
/// # Arguments
///
/// * `latex` - A LaTeX math expression (without delimiters like `$...$`)
/// * `display_mode` - If true, produces a block-level SVG; if false, inline
///
/// # Returns
///
/// An SVG string that can be embedded in HTML or rendered by an SVG viewer.
pub fn render_to_svg(latex: &str, display_mode: bool) -> Result<String> {
    let nodes = parser::parse(latex);
    Ok(renderer::render_to_svg(&nodes, display_mode))
}

/// Extract all LaTeX math expressions from a text string.
///
/// Detects both inline (`$...$`) and display (`$$...$$`) math patterns.
/// Returns a list of `MathExpression` structs with content, mode, and positions.
///
/// # Arguments
///
/// * `text` - Text that may contain `$...$` and `$$...$$` math expressions
///
/// # Returns
///
/// A sorted vector of `MathExpression` values, ordered by their position in the text.
pub fn extract_math_expressions(text: &str) -> Vec<MathExpression> {
    let mut expressions = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Check for $$...$$ (display math)
        if i + 1 < len && chars[i] == '$' && chars[i + 1] == '$' {
            let start = i;
            let mut j = i + 2;
            while j + 1 < len {
                if chars[j] == '$' && chars[j + 1] == '$' {
                    let end = j + 2;
                    let content: String = chars[(i + 2)..j].iter().collect();
                    expressions.push(MathExpression {
                        content: content.trim().to_string(),
                        display_mode: true,
                        start,
                        end,
                    });
                    i = end;
                    break;
                }
                j += 1;
            }
            if i == start {
                // No closing $$ found, skip past the opening $$
                i += 2;
            }
            continue;
        }

        // Check for $...$ (inline math)
        if chars[i] == '$' {
            let start = i;
            let mut j = i + 1;
            let mut found = false;
            while j < len {
                if chars[j] == '$' {
                    // Make sure it's not part of a $$ sequence
                    if j + 1 < len && chars[j + 1] == '$' {
                        // This is actually a $$, skip
                        j += 2;
                        continue;
                    }
                    let end = j + 1;
                    let content: String = chars[(i + 1)..j].iter().collect();
                    expressions.push(MathExpression {
                        content: content.trim().to_string(),
                        display_mode: false,
                        start,
                        end,
                    });
                    i = end;
                    found = true;
                    break;
                }
                j += 1;
            }
            if !found {
                i += 1;
            }
            continue;
        }

        i += 1;
    }

    expressions
}

/// Render a text string containing `$...$` and `$$...$$` math expressions,
/// replacing them with inline SVG representations.
///
/// The returned string contains embedded SVG elements for the math expressions,
/// with the surrounding text preserved as-is.
///
/// # Arguments
///
/// * `text` - Text that may contain LaTeX math expressions
///
/// # Returns
///
/// A string with math expressions replaced by SVG elements.
pub fn render_math_in_text(text: &str) -> Result<String> {
    let expressions = extract_math_expressions(text);

    if expressions.is_empty() {
        return Ok(text.to_string());
    }

    // Build the result by replacing expressions with SVG from right to left
    // (to preserve byte offsets)
    let mut result = text.to_string();

    for expr in expressions.iter().rev() {
        let svg = render_to_svg(&expr.content, expr.display_mode)?;
        result.replace_range(expr.start..expr.end, &svg);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_inline() {
        let exprs = extract_math_expressions("The formula $E = mc^2$ is famous");
        assert_eq!(exprs.len(), 1);
        assert!(!exprs[0].display_mode);
        assert!(exprs[0].content.contains("E = mc^2"));
    }

    #[test]
    fn test_extract_display() {
        let exprs = extract_math_expressions("Display: $$\\frac{1}{2}$$ is half");
        assert_eq!(exprs.len(), 1);
        assert!(exprs[0].display_mode);
        assert!(exprs[0].content.contains("\\frac{1}{2}"));
    }

    #[test]
    fn test_extract_multiple() {
        let exprs = extract_math_expressions("$a$ and $b$ and $$c$$");
        assert_eq!(exprs.len(), 3);
    }

    #[test]
    fn test_extract_none() {
        let exprs = extract_math_expressions("No math here");
        assert!(exprs.is_empty());
    }

    #[test]
    fn test_render_inline_math_text() {
        let result = render_math_in_text("Test $x^2$ here").unwrap();
        assert!(result.contains("<svg"));
        assert!(result.contains("Test"));
        assert!(result.contains("here"));
        // The $ should be replaced
        assert!(!result.contains("$x^2$"));
    }

    #[test]
    fn test_render_display_math_text() {
        let result = render_math_in_text("Display: $$\\sum_{i=0}^{n} x_i$$ end").unwrap();
        assert!(result.contains("<svg"));
        assert!(result.contains("Display:"));
        assert!(result.contains("end"));
    }

    #[test]
    fn test_render_to_svg_direct() {
        let svg = render_to_svg("E = mc^2", false).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn test_empty_expression() {
        let svg = render_to_svg("", false).unwrap();
        assert!(svg.starts_with("<svg"));
    }
}
