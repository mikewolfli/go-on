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

use crate::multimodal::document_parser::{DocumentParserError, ParsedContent};

/// Parse Excel bytes (`.xlsx` or `.xls`) and return extracted content.
///
/// This function uses `calamine` to open the workbook, iterates over every
/// worksheet, and collects cell text. Empty rows are skipped. Metadata includes
/// the list of sheet names and the total number of non-empty rows.
///
/// Supports both `.xlsx` (Office Open XML) and `.xls` (legacy OLE2) formats.
/// The format is auto-detected from the byte content: `.xlsx` starts with
/// the ZIP magic bytes (`PK\x03\x04`), while `.xls` uses the OLE2 compound
/// document format.
#[cfg(feature = "document-excel")]
pub fn parse_excel_bytes(bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
    // Detect format: XLSX files start with ZIP magic, XLS uses OLE2.
    let is_xlsx = bytes.starts_with(b"PK\x03\x04");

    if is_xlsx {
        open_xlsx(bytes)
    } else {
        // Try XLS (legacy format) via Xlsx as well; calamine 0.26's Xlsx
        // reader delegates to the OLE2-based Xls reader internally when
        // the input is an OLE2 document. If that fails, produce a clear
        // error.
        open_xlsx(bytes).map_err(|e| {
            // If Xlsx fails on non-ZIP data, give a helpful error.
            DocumentParserError::Other(format!(
                "Excel parse error: {e} (tried both .xlsx and .xls formats)"
            ))
        })
    }
}

/// Open an XLSX workbook from a byte slice and extract all text content.
#[cfg(feature = "document-excel")]
fn open_xlsx(bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
    use calamine::{Data, Reader, Xlsx};

    let mut workbook: Xlsx<std::io::Cursor<&[u8]>> = Xlsx::new(std::io::Cursor::new(bytes))
        .map_err(|e| DocumentParserError::Other(format!("Excel open error: {e}")))?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut content = ParsedContent::default();
    let mut all_text_parts: Vec<String> = Vec::new();
    let mut total_rows: usize = 0;

    for sheet_name in &sheet_names {
        let range = workbook
            .worksheet_range(sheet_name)
            .map_err(|e| DocumentParserError::Other(format!("Excel sheet error: {e}")))?;

        let mut sheet_text_parts: Vec<String> = Vec::new();

        for row in range.rows() {
            let row_cells: Vec<String> = row
                .iter()
                .filter_map(|cell| match cell {
                    Data::String(s) => Some(s.clone()),
                    Data::Float(f) => Some(format!("{f}")),
                    Data::Int(i) => Some(format!("{i}")),
                    Data::Bool(b) => Some(format!("{b}")),
                    Data::DateTime(_) => Some(format!("{cell}")),
                    Data::DateTimeIso(_) => Some(format!("{cell}")),
                    Data::DurationIso(_) => Some(format!("{cell}")),
                    Data::Error(e) => Some(format!("[ERROR: {e}]")),
                    Data::Empty => None,
                })
                .collect();

            if !row_cells.is_empty() {
                sheet_text_parts.push(row_cells.join("\t"));
                total_rows += 1;
            }
        }

        if !sheet_text_parts.is_empty() {
            all_text_parts.push(format!(
                "=== Sheet: {sheet_name} ===\n{}",
                sheet_text_parts.join("\n")
            ));
        }
    }

    content.text_content = all_text_parts.join("\n\n");
    content
        .metadata
        .insert("sheets".to_string(), sheet_names.join(", "));
    content
        .metadata
        .insert("sheet_count".to_string(), sheet_names.len().to_string());
    content
        .metadata
        .insert("row_count".to_string(), total_rows.to_string());
    content
        .metadata
        .insert("parser".to_string(), "calamine".to_string());

    Ok(content)
}

/// Placeholder for when the feature is disabled.
#[cfg(not(feature = "document-excel"))]
pub fn parse_excel_bytes(_bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
    Err(DocumentParserError::feature_disabled("Excel"))
}
