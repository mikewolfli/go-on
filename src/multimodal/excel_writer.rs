//! Excel (.xlsx) file writer using the `rust_xlsxwriter` crate.
//!
//! This module provides a simple API to create Excel workbooks with
//! multiple sheets and cell data.
//!
//! # Feature gate
//!
//! ```toml
//! document-excel-write = ["dep:rust_xlsxwriter"]
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Error type for Excel writing operations.
///
/// When the `document-excel-write` feature is enabled, the `XlsxWriter` variant
/// wraps a [`rust_xlsxwriter::XlsxError`]. When disabled, only the `FeatureDisabled`
/// variant is reachable.
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ExcelWriterError {
    /// Wrapper for rust_xlsxwriter errors (only when feature is enabled).
    #[cfg(feature = "document-excel-write")]
    #[error("Excel write error: {0}")]
    XlsxWriter(#[from] rust_xlsxwriter::XlsxError),
    /// No data provided to write.
    #[error("no data provided for Excel write")]
    NoData,
    /// Sheet name conflict or empty sheet name.
    #[error("invalid sheet name: {0}")]
    InvalidSheetName(String),
    /// Column/row index out of bounds.
    #[error("cell index out of bounds: {0}")]
    CellIndexOutOfBounds(String),
    /// Feature is not enabled.
    #[error("feature document-excel-write is not enabled")]
    FeatureDisabled,
}

/// Describes a single cell's value for Excel writing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellValue {
    /// A string value.
    String(String),
    /// A floating-point number.
    Number(f64),
    /// An integer.
    Integer(i64),
    /// A boolean value.
    Bool(bool),
    /// An empty cell.
    Empty,
}

/// A single row of cell data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    /// Cell values in column order.
    pub cells: Vec<CellValue>,
}

/// A worksheet to be written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetData {
    /// Sheet name (must be non-empty and <= 31 chars for Excel).
    pub name: String,
    /// Optional column headers (written as the first row).
    #[serde(default)]
    pub headers: Vec<String>,
    /// Data rows.
    #[serde(default)]
    pub rows: Vec<Row>,
}

/// Configuration for writing an Excel workbook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteExcelConfig {
    /// The sheets to include in the workbook.
    pub sheets: Vec<SheetData>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new Excel workbook from a configuration and return the bytes.
///
/// # Errors
///
/// Returns `ExcelWriterError` if the workbook cannot be created, if a sheet
/// name is invalid, or if cell data exceeds Excel's limits.
#[cfg(feature = "document-excel-write")]
pub fn write_excel_bytes(config: &WriteExcelConfig) -> Result<Vec<u8>, ExcelWriterError> {
    use rust_xlsxwriter::*;

    if config.sheets.is_empty() {
        return Err(ExcelWriterError::NoData);
    }

    let mut workbook = Workbook::new();

    for sheet_data in &config.sheets {
        validate_sheet_name(&sheet_data.name)?;

        let mut sheet = Worksheet::new();
        sheet.set_name(&sheet_data.name)?;
        sheet.set_tab_color(Color::RGB(0x4472C4));

        // Write headers if present
        if !sheet_data.headers.is_empty() {
            let header_fmt = Format::new()
                .set_bold()
                .set_background_color(Color::RGB(0xD9E2F3));
            for (col, header) in sheet_data.headers.iter().enumerate() {
                sheet.write_string_with_format(0, col as u16, header, &header_fmt)?;
            }
        }

        // Write data rows
        let start_row = if sheet_data.headers.is_empty() { 0 } else { 1 };

        for (row_idx, row) in sheet_data.rows.iter().enumerate() {
            let abs_row = start_row + row_idx;
            for (col_idx, cell) in row.cells.iter().enumerate() {
                let col = col_idx as u16;
                match cell {
                    CellValue::String(s) => {
                        sheet.write_string(abs_row as u32, col, s)?;
                    }
                    CellValue::Number(n) => {
                        sheet.write_number(abs_row as u32, col, *n)?;
                    }
                    CellValue::Integer(i) => {
                        sheet.write_number(abs_row as u32, col, *i as f64)?;
                    }
                    CellValue::Bool(b) => {
                        sheet.write_boolean(abs_row as u32, col, *b)?;
                    }
                    CellValue::Empty => {
                        let blank_fmt = Format::new();
                        sheet.write_blank(abs_row as u32, col, &blank_fmt)?;
                    }
                }
            }
        }

        workbook.push_worksheet(sheet);
    }

    let bytes = workbook.save_to_buffer()?;
    Ok(bytes)
}

/// Placeholder for when the feature is disabled.
#[cfg(not(feature = "document-excel-write"))]
pub fn write_excel_bytes(_config: &WriteExcelConfig) -> Result<Vec<u8>, ExcelWriterError> {
    Err(ExcelWriterError::FeatureDisabled)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Validate a sheet name according to Excel's constraints.
#[cfg(feature = "document-excel-write")]
fn validate_sheet_name(name: &str) -> Result<(), ExcelWriterError> {
    if name.is_empty() {
        return Err(ExcelWriterError::InvalidSheetName(
            "sheet name must not be empty".to_string(),
        ));
    }
    if name.len() > 31 {
        return Err(ExcelWriterError::InvalidSheetName(format!(
            "sheet name '{}' exceeds 31 character limit",
            name
        )));
    }
    // Forbidden characters: \ / ? * [ ] :
    if name.contains('\\')
        || name.contains('/')
        || name.contains('?')
        || name.contains('*')
        || name.contains('[')
        || name.contains(']')
        || name.contains(':')
    {
        return Err(ExcelWriterError::InvalidSheetName(format!(
            "sheet name '{}' contains forbidden characters (\\ / ? * [ ] :)",
            name
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_basic_excel() {
        let config = WriteExcelConfig {
            sheets: vec![SheetData {
                name: "Sheet1".to_string(),
                headers: vec!["Name".to_string(), "Age".to_string()],
                rows: vec![
                    Row {
                        cells: vec![
                            CellValue::String("Alice".to_string()),
                            CellValue::Integer(30),
                        ],
                    },
                    Row {
                        cells: vec![CellValue::String("Bob".to_string()), CellValue::Integer(25)],
                    },
                ],
            }],
        };

        let result = write_excel_bytes(&config);
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());
        // Should start with the ZIP magic bytes (xlsx is a ZIP archive)
        assert!(bytes.starts_with(b"PK\x03\x04"));
    }

    #[test]
    fn test_write_excel_empty_config_fails() {
        let config = WriteExcelConfig { sheets: vec![] };
        let result = write_excel_bytes(&config);
        assert!(result.is_err());
        match result.unwrap_err() {
            ExcelWriterError::NoData => {} // expected
            other => panic!("expected NoData error, got: {other}"),
        }
    }

    #[test]
    fn test_write_excel_numeric_cells() {
        let config = WriteExcelConfig {
            sheets: vec![SheetData {
                name: "Numbers".to_string(),
                headers: vec!["A".to_string(), "B".to_string()],
                rows: vec![Row {
                    cells: vec![
                        CellValue::Number(std::f64::consts::PI),
                        CellValue::Number(std::f64::consts::E),
                    ],
                }],
            }],
        };

        let result = write_excel_bytes(&config);
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(bytes.starts_with(b"PK\x03\x04"));
    }

    #[test]
    fn test_write_excel_multiple_sheets() {
        let config = WriteExcelConfig {
            sheets: vec![
                SheetData {
                    name: "Sheet1".to_string(),
                    headers: vec![],
                    rows: vec![Row {
                        cells: vec![CellValue::String("Hello".to_string())],
                    }],
                },
                SheetData {
                    name: "Sheet2".to_string(),
                    headers: vec![],
                    rows: vec![Row {
                        cells: vec![CellValue::String("World".to_string())],
                    }],
                },
            ],
        };

        let result = write_excel_bytes(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_empty_sheet_name_fails() {
        let config = WriteExcelConfig {
            sheets: vec![SheetData {
                name: "".to_string(),
                headers: vec![],
                rows: vec![Row {
                    cells: vec![CellValue::String("test".to_string())],
                }],
            }],
        };

        let result = write_excel_bytes(&config);
        assert!(result.is_err());
    }
}
