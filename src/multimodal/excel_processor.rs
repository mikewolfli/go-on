//! Excel (.xlsx / .xls) document parser using the `calamine` crate.
//!
//! This module extracts text content and metadata from Excel workbooks.
//! It is feature-gated behind `document-excel`.
//!
//! # Feature gate
//!
//! ```toml
//! document-excel = ["dep:calamine"]
//! ```
//!
//! # Public types
//!
//! - [`ParsedExcel`] — rich representation of a parsed workbook with
//!   per-sheet metadata, formula cell locations, merged cell ranges,
//!   and column/row dimensions.
//! - [`ExcelSheet`] — metadata for a single worksheet.
//! - [`CellCoordinate`] — zero-based row/col location.
//! - [`CellRange`] — a rectangular block of cells.

use serde::{Deserialize, Serialize};

use crate::multimodal::document_parser::{DocumentParserError, ParsedContent};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Rich representation of a parsed Excel workbook.
///
/// This struct exposes per-sheet metadata (dimensions, formula cells, merged
/// cell ranges) in addition to the extracted text content that is returned
/// via [`ParsedContent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedExcel {
    /// Per-sheet metadata in workbook order.
    pub sheets: Vec<ExcelSheet>,
    /// Total number of worksheets.
    pub sheet_count: usize,
}

/// Metadata extracted from a single worksheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExcelSheet {
    /// Sheet name (tab label).
    pub name: String,
    /// Number of non-empty data rows found.
    pub row_count: usize,
    /// Number of non-empty data columns found.
    pub column_count: usize,
    /// Zero-based coordinates of the first used cell (top-left corner of the
    /// used range), if any.
    pub first_cell: Option<CellCoordinate>,
    /// Zero-based coordinates of the last used cell (bottom-right corner of
    /// the used range), if any.
    pub last_cell: Option<CellCoordinate>,
    /// Cell references (`"A1"`, `"C3"`, etc.) that contain formulas.
    #[serde(default)]
    pub formula_cells: Vec<String>,
    /// Human-readable merged-cell range descriptions (`"A1:C3"`).
    #[serde(default)]
    pub merged_cell_ranges: Vec<String>,
    /// Number of cells that are the top-left corner of a merged block.
    pub merged_cell_count: usize,
    /// Zero-based coordinate ranges for merged cell blocks.
    #[serde(default)]
    pub merged_cell_coords: Vec<CellRange>,
    /// Tab-separated cell text per non-empty data row (the actual sheet
    /// content). Previously this was computed and then discarded — the parsed
    /// output carried only metadata.
    #[serde(default)]
    pub text_parts: Vec<String>,
}

/// A zero-based cell coordinate.
///
/// `row` is the zero-based row index, `col` is the zero-based column index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellCoordinate {
    pub row: u32,
    pub col: u32,
}

/// A rectangular cell range in zero-based coordinates.
///
/// The range is inclusive on all four boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRange {
    pub start_row: u32,
    pub start_col: u32,
    pub end_row: u32,
    pub end_col: u32,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse Excel bytes (`.xlsx` or `.xls`) and return extracted content.
///
/// Supports `.xlsx` (Office Open XML) and `.xls` (legacy OLE2) formats.
/// Both are opened through the same calamine `Xlsx` reader, which handles
/// the container format; the format is not branched on here.
#[cfg(feature = "document-excel")]
pub fn parse_excel_bytes(bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
    let parsed = open_workbook(bytes)?;

    // Convert the rich ParsedExcel into a flat ParsedContent.
    let parsed_content = parsed_excel_to_content(&parsed);
    Ok(parsed_content)
}

/// Placeholder for when the feature is disabled.
#[cfg(not(feature = "document-excel"))]
pub fn parse_excel_bytes(_bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
    Err(DocumentParserError::feature_disabled("Excel"))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Open a workbook from a byte slice and build a [`ParsedExcel`].
#[cfg(feature = "document-excel")]
fn open_workbook(bytes: &[u8]) -> Result<ParsedExcel, DocumentParserError> {
    use calamine::{Reader, Xlsx};

    let mut workbook: Xlsx<std::io::Cursor<&[u8]>> = Xlsx::new(std::io::Cursor::new(bytes))
        .map_err(|e| DocumentParserError::Other(format!("Excel open error: {e}")))?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut sheets: Vec<ExcelSheet> = Vec::with_capacity(sheet_names.len());

    for sheet_name in &sheet_names {
        let range = workbook.worksheet_range(sheet_name).map_err(|e| {
            DocumentParserError::Other(format!("Excel sheet '{sheet_name}' error: {e}"))
        })?;

        // ── Dimensions ────────────────────────────────────────────────
        let start = range.start();
        let end = range.end();

        let (first_cell, last_cell) = match (start, end) {
            (Some((r, c)), Some((re, ce))) => (
                Some(CellCoordinate { row: r, col: c }),
                Some(CellCoordinate { row: re, col: ce }),
            ),
            _ => (None, None),
        };

        // ── Column / row counts ───────────────────────────────────────
        let column_count = match (first_cell, last_cell) {
            (Some(f), Some(l)) => (l.col - f.col + 1) as usize,
            _ => 0,
        };

        // ── Merged cells ──────────────────────────────────────────────
        let mut merged_cell_ranges: Vec<String> = Vec::new();
        let mut merged_cell_count: usize = 0;
        let mut merged_cell_coords: Vec<CellRange> = Vec::new();
        // `worksheet_merge_cells` is a method on `Xlsx` (calamine 0.26.1).
        if let Some(Ok(merge_cells)) = workbook.worksheet_merge_cells(sheet_name) {
            for dim in &merge_cells {
                merged_cell_ranges.push(format!(
                    "{}:{}",
                    col_row_to_a1(dim.start.0, dim.start.1),
                    col_row_to_a1(dim.end.0, dim.end.1)
                ));
                merged_cell_coords.push(CellRange {
                    start_row: dim.start.0,
                    start_col: dim.start.1,
                    end_row: dim.end.0,
                    end_col: dim.end.1,
                });
                merged_cell_count += 1;
            }
        }

        // ── Formula cells ─────────────────────────────────────────────
        let mut formula_cells: Vec<String> = Vec::new();
        if let Ok(formula_range) = workbook.worksheet_formula(sheet_name) {
            for (row, col, formula_str) in formula_range.cells() {
                let trimmed = formula_str.trim();
                if !trimmed.is_empty() {
                    formula_cells.push(format!(
                        "{} = {trimmed}",
                        col_row_to_a1(row as u32, col as u32)
                    ));
                }
            }
        }

        // ── Text extraction with data-type awareness ──────────────
        let mut row_count: usize = 0;
        let mut sheet_text_parts: Vec<String> = Vec::new();

        for row_data in range.rows() {
            let row_cells: Vec<String> = row_data
                .iter()
                .filter_map(|cell| {
                    let (text, _type_name) = data_cell_to_string(cell);
                    if text.is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                })
                .collect();

            if !row_cells.is_empty() {
                sheet_text_parts.push(row_cells.join("\t"));
                row_count += 1;
            }
        }

        sheets.push(ExcelSheet {
            name: sheet_name.clone(),
            row_count,
            column_count,
            first_cell,
            last_cell,
            formula_cells,
            merged_cell_ranges,
            merged_cell_count,
            merged_cell_coords,
            // Preserve the extracted cell text so downstream consumers
            // receive the actual sheet content, not just metadata.
            text_parts: sheet_text_parts,
        });
    }

    Ok(ParsedExcel {
        sheet_count: sheets.len(),
        sheets,
    })
}

/// Convert a [`Data`] cell into a display string and a type label.
#[cfg(feature = "document-excel")]
fn data_cell_to_string(cell: &calamine::Data) -> (String, &'static str) {
    use calamine::Data;
    match cell {
        Data::String(s) => (s.clone(), "text"),
        Data::Float(f) => (format!("{f}"), "number"),
        Data::Int(i) => (format!("{i}"), "integer"),
        Data::Bool(b) => (format!("{b}"), "boolean"),
        Data::DateTime(_) => (format!("{cell}"), "date"),
        Data::DateTimeIso(_) => (format!("{cell}"), "date"),
        Data::DurationIso(_) => (format!("{cell}"), "duration"),
        Data::Error(e) => (format!("[ERROR: {e}]"), "error"),
        Data::Empty => (String::new(), "empty"),
    }
}

/// Convert zero-based (row, col) to an A1-style cell reference (e.g. `0,0` → `A1`).
#[cfg(feature = "document-excel")]
fn col_row_to_a1(row: u32, col: u32) -> String {
    // Convert column index to letters (0 = A, 1 = B, …, 25 = Z, 26 = AA, …).
    let mut col_letters = String::new();
    let mut c = col;
    loop {
        let rem = c % 26;
        col_letters.insert(0, char::from(b'A' + rem as u8));
        c /= 26;
        if c == 0 {
            break;
        }
        c -= 1; // Adjust for 1-based column lettering.
    }
    format!("{}{}", col_letters, row + 1)
}

/// Convert a rich [`ParsedExcel`] into a flat [`ParsedContent`] suitable for
/// the caller.
#[cfg(feature = "document-excel")]
fn parsed_excel_to_content(parsed: &ParsedExcel) -> ParsedContent {
    let mut content = ParsedContent::default();
    let mut all_text_parts: Vec<String> = Vec::new();
    let mut total_rows: usize = 0;
    let mut total_formulas: usize = 0;
    let mut total_merged: usize = 0;

    for sheet in &parsed.sheets {
        // Build a text representation similar to the original approach but
        // enriched with formula and merge annotations.
        let mut parts = Vec::new();

        // Actual cell content: tab-separated rows, one line per row.
        if !sheet.text_parts.is_empty() {
            parts.push(sheet.text_parts.join("\n"));
        }

        if !sheet.formula_cells.is_empty() {
            parts.push(format!(
                "  Formulas ({}): {}",
                sheet.formula_cells.len(),
                sheet.formula_cells.join(", ")
            ));
        }
        if !sheet.merged_cell_ranges.is_empty() {
            parts.push(format!(
                "  Merged cells ({}): {}",
                sheet.merged_cell_count,
                sheet.merged_cell_ranges.join(", ")
            ));
        }
        if let (Some(f), Some(l)) = (sheet.first_cell, sheet.last_cell) {
            parts.push(format!(
                "  Used range: {} → {}  ({} rows × {} cols)",
                col_row_to_a1(f.row, f.col),
                col_row_to_a1(l.row, l.col),
                l.row - f.row + 1,
                l.col - f.col + 1,
            ));
        }

        let meta_lines = parts.join("\n");

        let text = if meta_lines.is_empty() {
            String::new()
        } else {
            format!("[Sheet metadata]\n{meta_lines}")
        };

        all_text_parts.push(format!("=== Sheet: {name} ===\n{text}", name = sheet.name));

        total_rows += sheet.row_count;
        total_formulas += sheet.formula_cells.len();
        total_merged += sheet.merged_cell_count;
    }

    content.text_content = all_text_parts.join("\n\n");
    content.metadata.insert("sheets".to_string(), {
        let names: Vec<&str> = parsed.sheets.iter().map(|s| s.name.as_str()).collect();
        names.join(", ")
    });
    content
        .metadata
        .insert("sheet_count".to_string(), parsed.sheet_count.to_string());
    content
        .metadata
        .insert("row_count".to_string(), total_rows.to_string());
    content
        .metadata
        .insert("formula_count".to_string(), total_formulas.to_string());
    content
        .metadata
        .insert("merged_cell_count".to_string(), total_merged.to_string());
    content
        .metadata
        .insert("parser".to_string(), "calamine".to_string());

    content
}
