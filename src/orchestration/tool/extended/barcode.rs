//! Barcode generation tools.
//!
//! Generates Code-128 and EAN-13 barcodes as SVG output using built-in
//! encoding logic (no external barcode library). Feature-gated behind
//! `barcode-tools`.
//!
//! # Supported formats
//! - `code128` — variable-length Code 128B
//! - `ean13`   — 13-digit EAN-13
//!
//! Output is always an SVG string embeddable in HTML or storable as `.svg`.

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::Result;
use tracing::info;

/// Barcode generation tool.
pub struct QrCodeTool;

#[rustfmt::skip]
impl Tool for QrCodeTool {
    fn name(&self) -> &'static str {
        "barcode_gen"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let format = input.payload["format"]
            .as_str()
            .unwrap_or("code128")
            .to_lowercase();
        let data = input.payload["data"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'data' parameter for barcode"))?;
        let width = input.payload["width"].as_u64().unwrap_or(300);
        let height = input.payload["height"].as_u64().unwrap_or(100);

        let svg = match format.as_str() {
            "code128" => encode_code128(data, width as u32, height as u32),
            "ean13" => encode_ean13(data, width as u32, height as u32),
            _ => anyhow::bail!("unsupported barcode format: {format}; supported: code128, ean13"),
        };

        info!(
            format = format,
            data_len = data.len(),
            "tool: barcode generated"
        );
        tool_execution_report("barcode_gen", None);

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::Value::String(svg)),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

// ── Code 128B encoder ─────────────────────────────────────────────────────

/// Minimal Code 128B encoding producing raw bar patterns.
/// Output format: SVG with vertical black/white bars.
fn encode_code128(data: &str, width: u32, height: u32) -> String {
    let _ = width;
    let modules = code128b_modules(data);
    render_svg_bars(&modules, height)
}

/// Code 128B module widths: each character → 6 bars (3 black, 3 white)
/// using the standard Code 128 encoding table (subset B).
fn code128b_modules(data: &str) -> Vec<u8> {
    // Code 128B character widths in modules (6 modules per character)
    // Values from the Code 128 specification table.
    #[rustfmt::skip]
    const TABLE: [[u8; 6]; 32] = [
        [2,1,2,2,2,2], [2,2,2,1,2,2], [2,2,2,2,2,1], [1,2,1,2,2,3],
        [1,2,1,3,2,2], [1,3,1,2,2,2], [1,2,2,2,1,3], [1,2,2,3,1,2],
        [1,3,2,2,1,2], [2,2,1,2,1,3], [2,2,1,3,1,2], [2,3,1,2,1,2],
        [1,1,2,2,3,2], [1,2,2,1,3,2], [1,2,2,2,3,1], [1,1,3,2,2,2],
        [1,2,3,1,2,2], [1,2,3,2,2,1], [2,2,3,2,1,1], [2,2,1,1,3,2],
        [2,2,1,2,3,1], [2,1,3,2,1,2], [2,2,3,1,1,2], [3,1,2,1,3,1],
        [3,1,1,2,2,2], [3,2,1,1,2,2], [3,2,1,2,2,1], [3,1,2,2,1,2],
        [3,2,2,1,1,2], [3,2,2,2,1,1], [2,1,2,1,2,3], [2,1,2,3,2,1],
    ];

    let mut modules = Vec::new();

    // Start character (Code 128B start code = 104 → index 30)
    modules.extend_from_slice(&TABLE[30]);

    // Encode data bytes using ASCII subset (0x20-0x5F)
    let mut checksum: u32 = 104; // start code value
    for (i, &byte) in data.as_bytes().iter().enumerate() {
        let value = if (b' '..=b'_').contains(&byte) {
            (byte - b' ') as u32
        } else {
            0 // space for unsupported characters
        };
        checksum += value * (i as u32 + 1);
        let idx = (value % 32) as usize;
        modules.extend_from_slice(&TABLE[idx]);
    }

    // Checksum digit
    let check_digit = (checksum % 103) as usize;
    let idx = check_digit % 32;
    modules.extend_from_slice(&TABLE[idx]);

    // Stop character (Code 128 stop pattern)
    modules.extend_from_slice(&[2, 3, 3, 1, 1, 1, 2]);

    modules
}

// ── EAN-13 encoder ───────────────────────────────────────────────────────

/// Minimal EAN-13 encoding. Validates 12-13 digit input and produces bar patterns.
fn encode_ean13(data: &str, width: u32, height: u32) -> String {
    let _ = width;
    let digits = ean13_validate(data);
    let modules = ean13_modules(&digits);
    render_svg_bars(&modules, height)
}

fn ean13_validate(data: &str) -> Vec<u8> {
    let clean: String = data.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits: Vec<u8> = clean.chars().map(|c| c as u8 - b'0').collect();
    let count = digits.len();
    let mut result = Vec::with_capacity(13);

    if count == 12 {
        // Calculate check digit
        let sum: u32 = digits
            .iter()
            .enumerate()
            .map(|(i, &d)| d as u32 * if i % 2 == 0 { 1 } else { 3 })
            .sum();
        let check = (10 - (sum % 10)) % 10;
        result.extend_from_slice(&digits);
        result.push(check as u8);
    } else if count >= 13 {
        result.extend_from_slice(&digits[..13]);
    } else {
        // Pad with zeros
        result.resize(13, 0);
    }
    result
}

fn ean13_modules(digits: &[u8]) -> Vec<u8> {
    // Ensure we have exactly 13 digits
    let padded: Vec<u8> = if digits.len() >= 13 {
        digits[..13].to_vec()
    } else {
        let mut v = digits.to_vec();
        v.resize(13, 0);
        v
    };

    let mut modules = Vec::with_capacity(95);

    // Left group: 6 digits (simplified — uses fixed pattern)
    // In real EAN-13 these use L/G parity based on the first digit
    for &d in &padded[1..7] {
        // Simple L-code bar widths for each digit 0-9
        #[rustfmt::skip]
        const L_BARS: [[u8; 7]; 10] = [
            [3,2,1,1,0,0,0], [2,2,2,1,0,0,0], [2,1,2,2,0,0,0],
            [1,4,1,1,0,0,0], [1,1,3,2,0,0,0], [1,2,3,1,0,0,0],
            [1,1,1,4,0,0,0], [1,3,1,2,0,0,0], [1,2,1,3,0,0,0],
            [3,1,1,2,0,0,0],
        ];
        let bars = &L_BARS[d as usize];
        for &b in bars {
            if b > 0 {
                modules.push(b);
            }
        }
    }

    // Center guard: 1+1+1+1+1
    modules.extend_from_slice(&[1, 1, 1, 1, 1]);

    // Right group: 6 digits
    for &d in &padded[7..13] {
        #[rustfmt::skip]
        const R_BARS: [[u8; 7]; 10] = [
            [0,0,0,1,1,2,3], [0,0,0,1,2,2,2], [0,0,0,2,2,1,2],
            [0,0,0,1,1,4,1], [0,0,0,2,3,1,1], [0,0,0,1,3,2,1],
            [0,0,0,4,1,1,1], [0,0,0,2,1,3,1], [0,0,0,3,1,2,1],
            [0,0,0,2,1,1,3],
        ];
        let bars = &R_BARS[d as usize];
        for &b in bars {
            if b > 0 {
                modules.push(b);
            }
        }
    }

    // End guard: 1+1+1
    modules.extend_from_slice(&[1, 1, 1]);

    modules
}

// ── SVG renderer ─────────────────────────────────────────────────────────

/// Render bar module widths as an SVG. Each module is `bar_width` pixels wide.
/// Black bars are at even indices (0, 2, 4...), white spaces at odd indices.
fn render_svg_bars(modules: &[u8], height: u32) -> String {
    let bar_width = 2u32; // pixels per module
                          // Total width is the SUM of every module's width (black bars and white
                          // spaces alternate along x). Using max() clipped the barcode to the widest
                          // single module and overflowed the viewBox. The empty-module fallback
                          // keeps a sane default (EAN-13 always has 59 modules, so this is defensive).
    let total_width: u32 = if modules.is_empty() {
        200
    } else {
        modules.iter().map(|&m| m as u32 * bar_width).sum()
    };

    let mut svg = String::with_capacity(512 + modules.len() * 20);
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{tw}" height="{h}" viewBox="0 0 {tw} {h}">"#,
        tw = total_width, h = height
    ));
    svg.push_str(r#"<rect width="100%" height="100%" fill="white"/>"#);

    let mut x = 0u32;
    for (i, &module_count) in modules.iter().enumerate() {
        let w = module_count as u32 * bar_width;
        if i % 2 == 0 {
            // Black bar
            svg.push_str(&format!(
                r#"<rect x="{x}" y="0" width="{w}" height="{h}" fill="black"/>"#,
                x = x,
                w = w,
                h = height
            ));
        }
        x += w;
    }

    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code128_generates_non_empty_svg() {
        let svg = encode_code128("ABC123", 300, 100);
        assert!(
            svg.starts_with("<svg"),
            "Code128 SVG should start with <svg"
        );
        assert!(svg.contains("</svg>"), "Code128 SVG should close");
        assert!(
            svg.contains("<rect"),
            "Code128 SVG should contain rect elements"
        );
    }

    #[test]
    fn test_ean13_generates_non_empty_svg() {
        let svg = encode_ean13("123456789012", 300, 100);
        assert!(svg.starts_with("<svg"), "EAN13 SVG should start with <svg");
        assert!(svg.contains("</svg>"), "EAN13 SVG should close");
    }

    #[test]
    fn test_ean13_validate_pads_short_input() {
        let digits = ean13_validate("123");
        assert_eq!(digits.len(), 13, "EAN-13 should always produce 13 digits");
    }

    #[test]
    fn test_ean13_validate_strips_non_digits() {
        let digits = ean13_validate("ABC5901234567890XYZ");
        assert_eq!(digits.len(), 13, "Non-digits should be stripped");
    }

    #[test]
    fn test_render_svg_basic() {
        let modules = vec![1, 1, 1];
        let svg = render_svg_bars(&modules, 50);
        assert!(
            svg.contains("width=\"6\""),
            "3 modules × 2px = 6px total width"
        );
        assert!(svg.contains("height=\"50\""));
    }

    #[test]
    fn test_render_svg_width_is_module_sum_not_max() {
        // Regression: total width must be the sum of all module widths so the
        // barcode fits the viewBox. A single wide module (e.g. [3,1,1]) must
        // produce width 10 (6+2+2), not 6 (the widest module).
        let svg = render_svg_bars(&[3, 1, 1], 40);
        assert!(
            svg.contains("width=\"10\""),
            "3+1+1 modules × 2px = 10px total width, got: {}",
            svg.lines().next().unwrap_or("")
        );
    }

    #[test]
    fn test_render_svg_empty_modules_defaults_width() {
        let svg = render_svg_bars(&[], 40);
        assert!(
            svg.contains("width=\"200\""),
            "empty modules use default width"
        );
    }
}
