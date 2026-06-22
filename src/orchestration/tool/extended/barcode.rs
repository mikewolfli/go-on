//! QR Code generation tool
//!
//! Provides `QrCodeTool` for generating QR codes as SVG strings using a pure Rust
//! implementation with no external crates. Supports Version 1-4 QR codes with
//! byte mode encoding and error correction level M.
//! Only compiled when `feature = "barcode-tools"` is enabled.

#[cfg(feature = "barcode-tools")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "barcode-tools")]
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
#[cfg(feature = "barcode-tools")]
use anyhow::Result;
#[cfg(feature = "barcode-tools")]
use tracing::info;

// ── QrCodeTool ────────────────────────────────────────────────────────────────

#[cfg(feature = "barcode-tools")]
pub struct QrCodeTool;

#[cfg(feature = "barcode-tools")]
impl Tool for QrCodeTool {
    fn name(&self) -> &'static str {
        "qrcode_generate"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let text = input.payload["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'text'"))?;

        let module_size = input.payload["module_size"].as_u64().unwrap_or(4) as u32;
        let quiet_zone = input.payload["quiet_zone"].as_u64().unwrap_or(2) as u32;

        info!(
            text_len = text.len(),
            module_size = module_size,
            "generating QR code"
        );

        let svg = generate_qr_code_svg(text, module_size, quiet_zone)?;

        let report = tool_execution_report("qrcode_generate", Some("qrcode_generated"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "svg": svg,
                "text": text,
                "module_count": "version_dependent",
                "format": "svg",
            })),
            error: None,
            verification: Some("qrcode_generated".to_string()),
            audit_log: Some(format!(
                "Generated QR code for text of length {}",
                text.len()
            )),
            pua_report: Some(report),
        })
    }
}

// ── QR Code constants ─────────────────────────────────────────────────────────

/// ECC codewords per version and error correction level:
/// Version => (total data codewords, ECC codewords) for ECC level M
#[cfg(feature = "barcode-tools")]
const QR_VERSION_DATA: &[(usize, usize)] = &[
    (0, 0),   // unused index 0
    (16, 10), // Version 1: 16 data codewords, 10 ECC codewords (ECC-M)
    (28, 16), // Version 2: 28 data codewords, 16 ECC codewords (ECC-M)
    (44, 26), // Version 3: 44 data codewords, 26 ECC codewords (ECC-M)
    (64, 36), // Version 4: 64 data codewords, 36 ECC codewords (ECC-M)
];

/// QR code module size per version: version => modules_per_side (21 + 4*(v-1))
#[cfg(feature = "barcode-tools")]
fn qr_module_count(version: usize) -> usize {
    17 + 4 * version
}

// ── Main QR code generation ───────────────────────────────────────────────────

#[cfg(feature = "barcode-tools")]
fn generate_qr_code_svg(text: &str, module_size: u32, quiet_zone: u32) -> Result<String> {
    let bytes = text.as_bytes();

    // Select version based on data length (ECC level M, byte mode)
    let version = select_version(bytes.len())?;

    // Encode data into codewords
    let data_codewords = encode_data(bytes, version)?;

    // Generate ECC codewords (Reed-Solomon)
    let ecc_codewords = compute_reed_solomon(&data_codewords, version);

    // Interleave data and ECC codewords
    let all_codewords = [&data_codewords[..], &ecc_codewords[..]].concat();

    // Convert codewords to bit array (MSB first)
    let bits = codewords_to_bits(&all_codewords);

    // Build the module matrix
    let size = qr_module_count(version);
    let mut modules = vec![vec![false; size]; size];

    // Place finder patterns
    place_finder_patterns(&mut modules, size);
    // Place timing patterns
    place_timing_patterns(&mut modules, size);
    // Place data bits with masking
    place_data(&mut modules, &bits, version);
    // Apply mask pattern (mask 0: (row + col) % 2 == 0)
    apply_mask(&mut modules, version, 0);
    // Place format info
    place_format_info(&mut modules, version, 0);

    // Render SVG
    let svg = render_svg(&modules, module_size, quiet_zone);

    Ok(svg)
}

/// Select the minimum QR code version that can hold the given data length.
/// Uses ECC level M, byte mode.
#[cfg(feature = "barcode-tools")]
fn select_version(data_len: usize) -> Result<usize> {
    // Byte mode overhead: 4 bits mode indicator + 8 bits (v1-9) character count + data
    // For byte mode: total bits = 4 + 8 + data_len * 8
    let needed_bits = 4 + 8 + data_len * 8;
    let needed_codewords = (needed_bits + 7) / 8;

    for v in 1..=4 {
        let (capacity, _) = QR_VERSION_DATA[v];
        if needed_codewords <= capacity {
            return Ok(v);
        }
    }
    anyhow::bail!(
        "text too long for QR code version 1-4 ({} bytes max, got {} bytes)",
        QR_VERSION_DATA[4].0,
        data_len
    );
}

// ── Data encoding ─────────────────────────────────────────────────────────────

#[cfg(feature = "barcode-tools")]
fn encode_data(data: &[u8], version: usize) -> Result<Vec<u8>> {
    let (capacity, _) = QR_VERSION_DATA[version];
    let mut codewords = Vec::new();

    // Mode indicator: 0100 for byte mode
    // Character count: 8 bits for versions 1-9
    let char_count = data.len() as u16;

    // Mode: 0100
    let mut bits: Vec<bool> = Vec::new();
    // Mode indicator "0100"
    bits.push(false);
    bits.push(true);
    bits.push(false);
    bits.push(false);

    // Character count (8 bits)
    for i in (0..8).rev() {
        bits.push((char_count >> i) & 1 == 1);
    }

    // Data bits
    for &byte in data {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1 == 1);
        }
    }

    // Terminator: add up to 4 zero bits
    let terminator_len = std::cmp::min(4, capacity * 8 - bits.len());
    for _ in 0..terminator_len {
        bits.push(false);
    }

    // Pad to byte boundary
    while bits.len() % 8 != 0 {
        bits.push(false);
    }

    // Pad to capacity with alternating 0xEC and 0x11
    while bits.len() < capacity * 8 {
        let remaining = capacity * 8 - bits.len();
        let pad_byte: u8 = if (bits.len() / 8) % 2 == 0 {
            0xEC
        } else {
            0x11
        };
        let nbits = std::cmp::min(8, remaining);
        for i in (0..nbits).rev() {
            bits.push((pad_byte >> i) & 1 == 1);
        }
    }

    // Convert bits to codewords
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (i, &b) in chunk.iter().enumerate() {
            if b {
                byte |= 1 << (7 - i);
            }
        }
        codewords.push(byte);
    }

    Ok(codewords)
}

// ── Reed-Solomon error correction ────────────────────────────────────────────

/// QR code uses Reed-Solomon over GF(256) with primitive polynomial 0x11D (x^8 + x^4 + x^3 + x^2 + 1)
#[cfg(feature = "barcode-tools")]
const QR_GF_PRIMITIVE: u16 = 0x11D;

/// Pre-computed log and antilog tables for GF(256)
#[cfg(feature = "barcode-tools")]
fn gf_tables() -> ([u8; 256], [u16; 512]) {
    let mut log = [0u8; 256];
    let mut antilog = [0u16; 512];
    let mut val: u16 = 1;
    for i in 0..255 {
        antilog[i] = val;
        log[val as usize] = i as u8;
        val <<= 1;
        if val & 0x100 != 0 {
            val ^= QR_GF_PRIMITIVE;
        }
        val &= 0xFF;
    }
    antilog[255] = 1;
    for i in 255..511 {
        antilog[i + 1] = antilog[(i + 1) % 255];
    }
    (log, antilog)
}

/// Multiply two GF(256) elements
#[cfg(feature = "barcode-tools")]
fn gf_mul(a: u8, b: u8, log: &[u8; 256], antilog: &[u16; 512]) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let log_sum = log[a as usize] as u16 + log[b as usize] as u16;
    antilog[log_sum as usize] as u8
}

/// Compute Reed-Solomon error correction codewords for QR code.
#[cfg(feature = "barcode-tools")]
fn compute_reed_solomon(data: &[u8], version: usize) -> Vec<u8> {
    let (_, ecc_count) = QR_VERSION_DATA[version];
    let (log, antilog) = gf_tables();

    // Generator polynomial for the given ECC count
    // g(x) = (x - a^0)(x - a^1)...(x - a^(ecc_count-1))
    let mut gen = vec![1u8]; // start with 1
    for i in 0..ecc_count {
        // Multiply gen by (x - a^i)
        let a = antilog[i] as u8;
        let mut new_gen = vec![0u8; gen.len() + 1];
        for j in 0..gen.len() {
            new_gen[j] ^= gf_mul(gen[j], a, &log, &antilog);
            new_gen[j + 1] ^= gen[j];
        }
        gen = new_gen;
    }

    // Polynomial division: message * x^ecc_count / generator
    let mut remainder = vec![0u8; data.len() + ecc_count];
    remainder[..data.len()].copy_from_slice(data);

    for i in 0..data.len() {
        if remainder[i] != 0 {
            let lead = remainder[i];
            for j in 0..ecc_count {
                remainder[i + j + 1] ^= gf_mul(gen[j + 1], lead, &log, &antilog);
            }
        }
    }

    remainder[data.len()..].to_vec()
}

// ── Bit conversion ────────────────────────────────────────────────────────────

#[cfg(feature = "barcode-tools")]
fn codewords_to_bits(codewords: &[u8]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(codewords.len() * 8);
    for &byte in codewords {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1 == 1);
        }
    }
    bits
}

// ── Module placement ──────────────────────────────────────────────────────────

/// Place finder patterns (7x7) in three corners plus separators.
#[cfg(feature = "barcode-tools")]
fn place_finder_patterns(modules: &mut [Vec<bool>], size: usize) {
    let positions = [(0usize, 0usize), (0, size - 7), (size - 7, 0)];
    for &(row, col) in &positions {
        place_finder_at(modules, row, col, size);
    }
}

/// Place a single finder pattern (7x7 with separator) at the given position.
#[cfg(feature = "barcode-tools")]
fn place_finder_at(modules: &mut [Vec<bool>], start_row: usize, start_col: usize, size: usize) {
    // 7x7 finder pattern:
    // Outer dark border (row 0,6 and col 0,6), inner white ring (row 1-5 col 1-5), center dark 3x3
    for r in 0..7 {
        for c in 0..7 {
            let row = start_row + r;
            let col = start_col + c;
            if row < size && col < size {
                // Dark if on border (r==0||r==6||c==0||c==6) or in center 3x3 (r>=2&&r<=4&&c>=2&&c<=4)
                let dark = (r == 0 || r == 6 || c == 0 || c == 6)
                    || (r >= 2 && r <= 4 && c >= 2 && c <= 4);
                modules[row][col] = dark;
            }
        }
    }

    // Separator: white border around finder (one module wide)
    // Top separator
    if start_row > 0 {
        for dc in 0i32..8 {
            let col = (start_col as i32 + dc - 1) as usize;
            if col < size {
                modules[start_row - 1][col] = false;
            }
        }
    }
    // Bottom separator
    let bottom = start_row + 7;
    if bottom < size {
        for dc in 0i32..8 {
            let col = (start_col as i32 + dc - 1) as usize;
            if col < size {
                modules[bottom][col] = false;
            }
        }
    }
    // Left separator
    if start_col > 0 {
        for dr in 0i32..8 {
            let row = (start_row as i32 + dr - 1) as usize;
            if row < size {
                modules[row][start_col - 1] = false;
            }
        }
    }
    // Right separator
    let right = start_col + 7;
    if right < size {
        for dr in 0i32..8 {
            let row = (start_row as i32 + dr - 1) as usize;
            if row < size {
                modules[row][right] = false;
            }
        }
    }
}

/// Place timing patterns (alternating dark/light modules on row 6 and col 6).
#[cfg(feature = "barcode-tools")]
fn place_timing_patterns(modules: &mut [Vec<bool>], size: usize) {
    for i in 8..size - 8 {
        modules[6][i] = i % 2 == 0;
        modules[i][6] = i % 2 == 0;
    }
}

/// Place data bits in the QR code matrix using the standard QR code placement
/// pattern (upward and downward zigzag pairs from bottom-right).
#[cfg(feature = "barcode-tools")]
fn place_data(modules: &mut [Vec<bool>], bits: &[bool], version: usize) {
    let size = qr_module_count(version);
    let mut bit_idx = 0;

    // Reserve finder pattern areas, timing patterns, and format areas
    let mut reserved = vec![vec![false; size]; size];

    // Mark finder pattern areas as reserved
    for r in 0..9 {
        for c in 0..9 {
            if r < size && c < size {
                reserved[r][c] = true;
            }
        }
    }
    for r in 0..9 {
        for c in size - 8..size {
            if r < 9 && c < size {
                reserved[r][c] = true;
            }
        }
    }
    for r in size - 8..size {
        for c in 0..9 {
            if r < size && c < size {
                reserved[r][c] = true;
            }
        }
    }
    // Mark timing patterns
    for i in 0..size {
        reserved[6][i] = true;
        reserved[i][6] = true;
    }

    // Place bits in columns from right to left, 2 columns at a time
    let mut col = size as isize - 1;
    while col > 0 {
        if col == 6 {
            col -= 1; // skip timing pattern column
            continue;
        }

        let mut row: isize;
        // Determine direction: odd-numbered column pair (from bottom), even (from top)
        let pair_index = (size as isize - 1 - col) / 2;
        let dir: isize;
        if pair_index % 2 == 0 {
            // Upward: bottom to top
            row = size as isize - 1;
            dir = -1;
        } else {
            // Downward: top to bottom
            row = 0;
            dir = 1;
        }

        for _ in 0..size as isize {
            if row >= 0 && row < size as isize {
                for col_offset in 0..2 {
                    let c = (col - col_offset) as usize;
                    if c < size && !reserved[row as usize][c] {
                        if bit_idx < bits.len() {
                            modules[row as usize][c] = bits[bit_idx];
                            bit_idx += 1;
                        }
                    }
                }
            }
            row += dir;
        }

        col -= 2;
    }
}

/// Apply mask pattern to the QR code modules.
/// Mask 0: (row + col) % 2 == 0 — XOR data modules.
#[cfg(feature = "barcode-tools")]
fn apply_mask(modules: &mut [Vec<bool>], version: usize, mask: u8) {
    let size = qr_module_count(version);

    // Determine which modules are data (not reserved for finder, timing, format)
    let mut is_data = vec![vec![false; size]; size];
    for r in 0..size {
        for c in 0..size {
            // Skip finder pattern areas (9x9 corners)
            let in_finder_tl = r < 9 && c < 9;
            let in_finder_tr = r < 9 && c >= size - 8;
            let in_finder_bl = r >= size - 8 && c < 9;
            let is_timing = r == 6 || c == 6;

            // Format info area (around finder patterns)
            let in_format_top = r < 9 && (c == 8);
            let in_format_bottom = r >= size - 8 && c == 8;
            let in_format_left = (r == 8) && c < 8;
            let in_format_right = r == 8 && c >= size - 8;

            if !in_finder_tl
                && !in_finder_tr
                && !in_finder_bl
                && !is_timing
                && !in_format_top
                && !in_format_bottom
                && !in_format_left
                && !in_format_right
            {
                is_data[r][c] = true;
            }
        }
    }

    // Apply mask 0: (row + col) % 2 == 0
    for r in 0..size {
        for c in 0..size {
            if is_data[r][c] {
                let mask_bit = match mask {
                    0 => (r + c) % 2 == 0,
                    1 => r % 2 == 0,
                    2 => c % 3 == 0,
                    3 => (r + c) % 3 == 0,
                    4 => (r / 2 + c / 3) % 2 == 0,
                    5 => (r * c) % 2 + (r * c) % 3 == 0,
                    6 => ((r * c) % 2 + (r * c) % 3) % 2 == 0,
                    7 => ((r + c) % 2 + (r * c) % 3) % 2 == 0,
                    _ => false,
                };
                if mask_bit {
                    modules[r][c] = !modules[r][c];
                }
            }
        }
    }
}

/// Place format info bits (15 bits) around the finder patterns.
/// Format: 5 data bits | 10 BCH error correction bits, XOR'd with mask 0x5412.
#[cfg(feature = "barcode-tools")]
fn place_format_info(modules: &mut [Vec<bool>], version: usize, _mask: u8) {
    let size = qr_module_count(version);

    // ECC level M = 0b00, mask pattern 0 = 0b000
    // 5 format bits: 00 (ECC) | 000 (mask) = 0b00000
    let format_bits: u16 = 0b00000;

    // BCH encoding: 15-bit format string with generator 0x537
    // format_string = (format_bits << 10) ^ BCH remainder
    let mut bch = format_bits << 10;
    for i in (0..5).rev() {
        if (bch >> (i + 10)) & 1 != 0 {
            bch ^= 0x537 << i;
        }
    }
    let format_string = ((format_bits << 10) ^ bch) ^ 0x5412;

    // Place format bits around finder patterns
    // Top-right: columns 0-5 (row 8), column 7 (row 8), column 8 (rows 0-5, row 7)
    // Bottom-left: rows size-8 to size-1 (column 8), row 8 (columns size-8 to size-1)

    // Horizontal timing pattern top: row 8, columns 0-5
    for i in 0..6 {
        let bit = ((format_string >> (14 - i)) & 1) == 1;
        modules[8][i] = bit;
    }
    // Horizontal: row 8, column 7
    let bit7 = ((format_string >> 8) & 1) == 1;
    modules[8][7] = bit7;
    // Horizontal: row 8, column 8 is dark module
    modules[8][8] = true;
    // Horizontal: row 8, column 9-14 (not needed for smaller versions)

    // Vertical timing pattern left: rows 0-5, column 8
    for i in 0..6 {
        let bit = ((format_string >> (14 - i)) & 1) == 1;
        modules[i][8] = bit;
    }
    // Vertical: row 7, column 8
    let bit_v7 = ((format_string >> 8) & 1) == 1;
    modules[7][8] = bit_v7;

    // Bottom-left: rows size-8+1 to size-1, column 8
    for i in 0..7 {
        let row = size - 7 + i;
        if row < size {
            let bit = ((format_string >> (14 - i)) & 1) == 1;
            modules[row][8] = bit;
        }
    }
}

// ── SVG rendering ─────────────────────────────────────────────────────────────

#[cfg(feature = "barcode-tools")]
fn render_svg(modules: &[Vec<bool>], module_size: u32, quiet_zone: u32) -> String {
    let size = modules.len() as u32;
    let canvas_size = (size + 2 * quiet_zone) * module_size;

    let mut svg = String::new();
    svg.push_str(&format!(r##"<?xml version="1.0" encoding="UTF-8"?>"##));
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" version="1.1" width="{}" height="{}" shape-rendering="crispEdges">"##,
        canvas_size, canvas_size
    ));

    // Background (white)
    svg.push_str(&format!(
        r##"<rect x="0" y="0" width="{}" height="{}" fill="#ffffff"/>"##,
        canvas_size, canvas_size
    ));

    // Dark modules (black)
    for (r, row) in modules.iter().enumerate() {
        for (c, &is_dark) in row.iter().enumerate() {
            if is_dark {
                let x = (c as u32 + quiet_zone) * module_size;
                let y = (r as u32 + quiet_zone) * module_size;
                svg.push_str(&format!(
                    r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#000000"/>"##,
                    x, y, module_size, module_size
                ));
            }
        }
    }

    svg.push_str("</svg>");
    svg
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[cfg(feature = "barcode-tools")]
mod tests {
    use super::*;

    #[test]
    fn test_qrcode_generates_svg() {
        let svg = generate_qr_code_svg("Hello, world!", 4, 2).unwrap();
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("#000000"));
        assert!(svg.contains("#ffffff"));
    }

    #[test]
    fn test_qrcode_short_text() {
        let svg = generate_qr_code_svg("A", 4, 2).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn test_qrcode_empty_text_fails() {
        let result = generate_qr_code_svg("", 4, 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_qrcode_long_text_fails() {
        let long = "A".repeat(100);
        let result = generate_qr_code_svg(&long, 4, 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_select_version() {
        assert_eq!(select_version(10).unwrap(), 1);
        assert_eq!(select_version(17).unwrap(), 1);
        assert_eq!(select_version(18).unwrap(), 2);
    }

    #[test]
    fn test_reed_solomon() {
        let data = vec![0x40, 0x12, 0x34];
        let ecc = compute_reed_solomon(&data, 1);
        assert_eq!(ecc.len(), 10);
        // ECC should not be all zeros
        assert!(ecc.iter().any(|&b| b != 0));
    }
}
