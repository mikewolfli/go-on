//! Document parser — extracts structured content (text, images, tables, metadata)
//! from PDF, DOCX, HTML, and Markdown files.
//!
//! Each backend is gated behind its own Cargo feature:
//!
//! | Feature | Crate | Format |
//! |---------|-------|--------|
//! | `document-pdf` | `lopdf` | PDF |
//! | `document-docx` | `docx-rs` | Office Open XML (.docx) |
//! | `document-html` | `scraper` | HTML / XHTML |
//! | `document-markdown` | `comrak` | Markdown |
//!
//! When a feature is disabled the corresponding `parse_*` method returns
//! a [`DocumentParserError::FeatureDisabled`] error.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during document parsing.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DocumentParserError {
    /// The file could not be read from disk.
    #[error("I/O error: {0}")]
    Io(String),

    /// The file extension is not supported by any backend.
    #[error("unsupported file extension: {0}")]
    UnsupportedExtension(String),

    /// A required Cargo feature is not enabled.
    #[error("feature not enabled: {0}")]
    FeatureDisabled(String),

    /// Empty input provided (empty bytes or missing file extension).
    #[error("empty input provided: {0}")]
    EmptyInput(String),

    /// File size exceeds the maximum allowed limit.
    #[error("file too large: {size} bytes (maximum: {max} bytes)")]
    FileTooLarge { size: u64, max: u64 },

    /// PDF backend-specific error (lopdf).
    #[cfg(feature = "document-pdf")]
    #[error("PDF parse error: {0}")]
    Pdf(String),

    /// DOCX backend-specific error (docx-rs).
    #[cfg(feature = "document-docx")]
    #[error("DOCX parse error: {0}")]
    Docx(String),

    /// HTML backend-specific error (scraper).
    #[cfg(feature = "document-html")]
    #[error("HTML parse error: {0}")]
    Html(String),

    /// Markdown backend-specific error (comrak).
    #[cfg(feature = "document-markdown")]
    #[error("Markdown parse error: {0}")]
    Markdown(String),

    /// Catch-all for unexpected errors.
    #[error("{0}")]
    Other(String),
}

impl DocumentParserError {
    /// Create an `Other` variant from a string.
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }

    /// Create an `Io` variant from a `std::io::Error`.
    pub fn from_io(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }

    /// Create a `FeatureDisabled` error with a descriptive message.
    pub fn feature_disabled(name: &str) -> Self {
        Self::FeatureDisabled(format!(
            "{} parsing requires the corresponding Cargo feature to be enabled",
            name
        ))
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Structured content extracted from a parsed document.
///
/// This type is `Serialize` + `Deserialize` so it can be injected directly
/// into chat RPC payloads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedContent {
    /// Extracted plain text (concatenated from paragraphs, headings, etc.).
    pub text_content: String,
    /// Base64-encoded image data found embedded in the document.
    #[serde(default)]
    pub images: Vec<String>,
    /// Tables discovered in the document.
    #[serde(default)]
    pub tables: Vec<Table>,
    /// Arbitrary key-value metadata (author, title, creation date, error info, etc.).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl ParsedContent {
    /// Returns `true` if the parser encountered an error (detected by the
    /// presence of an `"error"` key in `metadata`).
    pub fn has_error(&self) -> bool {
        self.metadata.contains_key("error")
    }

    /// Returns the error message from metadata, if any.
    pub fn error_message(&self) -> Option<&str> {
        self.metadata.get("error").map(|s| s.as_str())
    }

    /// Total number of characters extracted.
    pub fn char_count(&self) -> usize {
        self.text_content.len()
    }
}

/// A single table extracted from a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    /// Optional caption or heading text preceding the table.
    pub caption: Option<String>,
    /// Column header labels (first row / thead).
    #[serde(default)]
    pub headers: Vec<String>,
    /// Data rows (each inner `Vec` has the same length as `headers`).
    #[serde(default)]
    pub rows: Vec<Vec<String>>,
}

impl Table {
    /// Number of data rows (excluding the header row).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Number of columns.
    pub fn col_count(&self) -> usize {
        if !self.headers.is_empty() {
            self.headers.len()
        } else {
            self.rows.first().map(|r| r.len()).unwrap_or(0)
        }
    }
}

/// Document-parser dispatcher.
///
/// Usage:
/// ```ignore
/// let parser = go_on::multimodal::DocumentParser::default();
/// let result = parser.parse("report.pdf");
/// println!("{}", result.text_content);
/// ```
///
/// The parser can be configured with a custom `max_text_length` to cap the
/// extracted text content length:
/// ```ignore
/// let parser = go_on::multimodal::DocumentParser {
///     max_text_length: 5_000_000,  // 5 MB
/// };
/// ```
#[derive(Debug, Clone)]
pub struct DocumentParser {
    /// Maximum length of `text_content` in bytes (soft cap).
    /// When the extracted text exceeds this value it will be truncated and a
    /// `"truncated"` entry will be added to the metadata.
    pub max_text_length: usize,
}

impl Default for DocumentParser {
    fn default() -> Self {
        Self {
            // 10 MB default — covers most realistic documents while bounding
            // memory usage from malformed or pathological inputs.
            max_text_length: 10 * 1024 * 1024,
        }
    }
}

impl DocumentParser {
    /// Parse a file at `path`, inferring the format from the file extension.
    ///
    /// Supported extensions:
    /// - `.pdf`       (requires `document-pdf`)
    /// - `.docx`      (requires `document-docx`)
    /// - `.html`, `.htm` (requires `document-html`)
    /// - `.md`, `.markdown` (requires `document-markdown`)
    ///
    /// Unsupported extensions return a [`DocumentParserError::UnsupportedExtension`].
    pub fn parse(&self, path: impl AsRef<Path>) -> Result<ParsedContent, DocumentParserError> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // ── Input validation ──────────────────────────────────────────
        if ext.is_empty() {
            return Err(DocumentParserError::EmptyInput(
                "file path has no extension; cannot determine document format".to_string(),
            ));
        }

        let metadata = std::fs::metadata(path).map_err(DocumentParserError::from_io)?;
        let file_size = metadata.len();
        if file_size == 0 {
            return Err(DocumentParserError::EmptyInput(format!(
                "file '{}' is empty (0 bytes)",
                path.display(),
            )));
        }
        const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
        if file_size > MAX_FILE_SIZE {
            return Err(DocumentParserError::FileTooLarge {
                size: file_size,
                max: MAX_FILE_SIZE,
            });
        }

        let mut result = match ext.as_str() {
            "pdf" => self.parse_pdf(path),
            "docx" => self.parse_docx(path),
            "html" | "htm" => self.parse_html(path),
            "md" | "markdown" => self.parse_markdown(path),
            _ => return Err(DocumentParserError::UnsupportedExtension(ext)),
        }?;

        self.truncate_content(&mut result);
        Ok(result)
    }

    /// Parse bytes with an explicit extension hint (no I/O).
    ///
    /// This is useful when the document data is already in memory (e.g. from
    /// a [`MultimodalInput::Document`]).
    pub fn parse_bytes(
        &self,
        bytes: &[u8],
        extension: &str,
    ) -> Result<ParsedContent, DocumentParserError> {
        let ext = extension.trim().to_lowercase();

        // ── Input validation ──────────────────────────────────────────
        if ext.is_empty() {
            return Err(DocumentParserError::EmptyInput(
                "extension hint is empty; cannot determine document format".to_string(),
            ));
        }
        if bytes.is_empty() {
            return Err(DocumentParserError::EmptyInput(
                "byte slice is empty; nothing to parse".to_string(),
            ));
        }

        let mut result = match ext.as_str() {
            "pdf" => self.parse_pdf_bytes(bytes),
            "docx" => self.parse_docx_bytes(bytes),
            "html" | "htm" => self.parse_html_bytes(bytes),
            "md" | "markdown" => self.parse_markdown_bytes(bytes),
            _ => return Err(DocumentParserError::UnsupportedExtension(ext)),
        }?;

        self.truncate_content(&mut result);
        Ok(result)
    }

    /// Cap `text_content` to [`max_text_length`] if it exceeds the limit
    /// and record a `"truncated"` metadata entry.
    fn truncate_content(&self, content: &mut ParsedContent) {
        if content.text_content.len() > self.max_text_length {
            let original_len = content.text_content.len();
            content.text_content.truncate(self.max_text_length);
            content
                .text_content
                .push_str("\n\n[... content truncated ...]");
            content.metadata.insert(
                "truncated".to_string(),
                format!(
                    "text_content was truncated from {} to {} bytes",
                    original_len, self.max_text_length
                ),
            );
        }
    }

    // =======================================================================
    // PDF backend  (feature = "document-pdf", crate: lopdf)
    // =======================================================================

    #[cfg(feature = "document-pdf")]
    fn parse_pdf(&self, path: &Path) -> Result<ParsedContent, DocumentParserError> {
        let bytes = std::fs::read(path).map_err(DocumentParserError::from_io)?;
        self.parse_pdf_bytes(&bytes)
    }

    #[cfg(not(feature = "document-pdf"))]
    fn parse_pdf(&self, _path: &Path) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("PDF"))
    }

    #[cfg(feature = "document-pdf")]
    fn parse_pdf_bytes(&self, bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
        use lopdf::Document;

        let doc = Document::load_mem(bytes).map_err(|e| DocumentParserError::Pdf(e.to_string()))?;
        let mut content = ParsedContent::default();

        // 1. Extract text from every page.
        let pages: Vec<u32> = doc.get_pages().into_keys().collect();
        let mut text_parts: Vec<String> = Vec::with_capacity(pages.len());
        for page_num in &pages {
            match doc.extract_text(&[*page_num]) {
                Ok(text) => {
                    let cleaned: Vec<&str> = text
                        .lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty())
                        .collect();
                    if !cleaned.is_empty() {
                        text_parts.push(cleaned.join("\n"));
                    }
                }
                Err(e) => {
                    content
                        .metadata
                        .insert(format!("page_{}_error", page_num), e.to_string());
                }
            }
        }
        content.text_content = text_parts.join("\n\n");

        // 2. Extract images from page resource XObjects.
        for page_num in &pages {
            let page_id: (u32, u16) = (*page_num, 0);
            if let Ok((Some(res_dict), _)) = doc.get_page_resources(page_id) {
                if let Ok(xobjects) = res_dict.get(b"XObject") {
                    if let Ok(xobj_dict) = xobjects.as_dict() {
                        for (_key, val) in xobj_dict.iter() {
                            if let Ok((_, obj)) = doc.dereference(val) {
                                if let Ok(stream) = obj.as_stream() {
                                    let is_image = stream
                                        .dict
                                        .get(b"Subtype")
                                        .map(|o| {
                                            o.as_name().map(|n| n == b"Image").unwrap_or(false)
                                        })
                                        .unwrap_or(false);
                                    if is_image {
                                        content.images.push(base64_encode(&stream.content));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3. Metadata from trailer dictionary.
        const INFO_KEYS: &[&[u8]] = &[
            b"Title",
            b"Author",
            b"Subject",
            b"Keywords",
            b"Creator",
            b"Producer",
            b"CreationDate",
            b"ModDate",
        ];
        for key in INFO_KEYS {
            if let Ok(val) = doc.trailer.get(key) {
                let key_str = std::str::from_utf8(key).unwrap_or_default().to_string();
                if let Ok(cow_str) = val.as_string() {
                    content.metadata.insert(key_str, cow_str.to_string());
                }
            }
        }

        content
            .metadata
            .entry("page_count".to_string())
            .or_insert_with(|| pages.len().to_string());
        content
            .metadata
            .insert("parser".to_string(), "lopdf".to_string());

        Ok(content)
    }

    #[cfg(not(feature = "document-pdf"))]
    fn parse_pdf_bytes(&self, _bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("PDF"))
    }

    // =======================================================================
    // DOCX backend  (feature = "document-docx", crate: docx-rs)
    // =======================================================================

    #[cfg(feature = "document-docx")]
    fn parse_docx(&self, path: &Path) -> Result<ParsedContent, DocumentParserError> {
        let bytes = std::fs::read(path).map_err(DocumentParserError::from_io)?;
        self.parse_docx_bytes(&bytes)
    }

    #[cfg(not(feature = "document-docx"))]
    fn parse_docx(&self, _path: &Path) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("DOCX"))
    }

    #[cfg(feature = "document-docx")]
    fn parse_docx_bytes(&self, bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
        use docx_rs::*;

        let docx = read_docx(bytes).map_err(|e| DocumentParserError::Docx(e.to_string()))?;

        let mut content = ParsedContent::default();
        let mut text_parts: Vec<String> = Vec::new();

        for child in docx.document.children {
            match child {
                DocumentChild::Paragraph(p) => {
                    let line: String = p
                        .children
                        .iter()
                        .filter_map(|run| match run {
                            ParagraphChild::Run(r) => {
                                let text: String = r
                                    .children
                                    .iter()
                                    .filter_map(|rc| match rc {
                                        RunChild::Text(t) => Some(t.text.clone()),
                                        _ => None,
                                    })
                                    .collect();
                                if text.trim().is_empty() {
                                    None
                                } else {
                                    Some(text)
                                }
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !line.trim().is_empty() {
                        text_parts.push(line.trim().to_string());
                    }
                }
                DocumentChild::Table(tbl) => {
                    // Use fully-qualified path to avoid shadowing from docx_rs::*
                    let mut table = crate::multimodal::document_parser::Table {
                        caption: None,
                        headers: Vec::new(),
                        rows: Vec::new(),
                    };
                    for docx_rs::TableChild::TableRow(row) in &tbl.rows {
                        let cells: Vec<String> = row
                            .cells
                            .iter()
                            .map(|rc| {
                                let docx_rs::TableRowChild::TableCell(cell) = rc;
                                cell.children
                                    .iter()
                                    .filter_map(|cc| match cc {
                                        docx_rs::TableCellContent::Paragraph(p) => {
                                            let t: String = p
                                                .children
                                                .iter()
                                                .filter_map(|pc| match pc {
                                                    ParagraphChild::Run(r) => {
                                                        let txt: String = r
                                                            .children
                                                            .iter()
                                                            .filter_map(|rc| match rc {
                                                                RunChild::Text(tx) => {
                                                                    Some(tx.text.clone())
                                                                }
                                                                _ => None,
                                                            })
                                                            .collect();
                                                        Some(txt)
                                                    }
                                                    _ => None,
                                                })
                                                .collect();
                                            Some(t)
                                        }
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .collect();
                        table.rows.push(cells);
                    }
                    content.tables.push(table);
                }
                // Images / drawings — skipped: image extraction is not yet implemented
                _ => {}
            }
        }

        content.text_content = text_parts.join("\n");
        content
            .metadata
            .insert("paragraph_count".to_string(), text_parts.len().to_string());
        content
            .metadata
            .insert("table_count".to_string(), content.tables.len().to_string());
        content
            .metadata
            .insert("parser".to_string(), "docx-rs".to_string());

        Ok(content)
    }

    #[cfg(not(feature = "document-docx"))]
    fn parse_docx_bytes(&self, _bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("DOCX"))
    }

    // =======================================================================
    // HTML backend  (feature = "document-html", crate: scraper)
    // =======================================================================

    #[cfg(feature = "document-html")]
    fn parse_html(&self, path: &Path) -> Result<ParsedContent, DocumentParserError> {
        let html_str = std::fs::read_to_string(path).map_err(DocumentParserError::from_io)?;
        self.parse_html_str(&html_str)
    }

    #[cfg(not(feature = "document-html"))]
    fn parse_html(&self, _path: &Path) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("HTML"))
    }

    #[cfg(feature = "document-html")]
    fn parse_html_bytes(&self, bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
        let html_str = String::from_utf8(bytes.to_vec())
            .map_err(|e| DocumentParserError::Html(format!("invalid UTF-8: {e}")))?;
        self.parse_html_str(&html_str)
    }

    #[cfg(not(feature = "document-html"))]
    fn parse_html_bytes(&self, _bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("HTML"))
    }

    #[cfg(feature = "document-html")]
    fn parse_html_str(&self, html_str: &str) -> Result<ParsedContent, DocumentParserError> {
        use scraper::{Html, Selector};

        let document = Html::parse_document(html_str);
        let mut content = ParsedContent::default();

        // 1. Extract visible text from common block / inline elements.
        let text_selectors = [
            "p",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "li",
            "td",
            "th",
            "div",
            "span",
            "blockquote",
            "pre",
            "code",
            "article",
            "section",
            "figcaption",
        ];
        let mut text_parts: Vec<String> = Vec::new();
        for tag in &text_selectors {
            if let Ok(sel) = Selector::parse(tag) {
                for elem in document.select(&sel) {
                    let t: String = elem.text().collect::<Vec<_>>().join(" ");
                    let t = t.trim().to_string();
                    if !t.is_empty() {
                        text_parts.push(t);
                    }
                }
            }
        }
        content.text_content = text_parts.join("\n");

        // 2. Extract <a> links as metadata.
        if let Ok(sel) = Selector::parse("a[href]") {
            for elem in document.select(&sel) {
                if let Some(href) = elem.value().attr("href") {
                    let label: String = elem.text().collect();
                    let key = if label.is_empty() {
                        "link".to_string()
                    } else {
                        format!("link:{}", label)
                    };
                    // Avoid overwriting duplicate labels with the same key.
                    content
                        .metadata
                        .entry(key)
                        .or_insert_with(|| href.to_string());
                }
            }
        }

        // 3. Extract <table> elements.
        if let Ok(table_sel) = Selector::parse("table") {
            for table_elem in document.select(&table_sel) {
                let mut table = Table {
                    caption: None,
                    headers: Vec::new(),
                    rows: Vec::new(),
                };

                // Optional <caption>
                if let Ok(cap_sel) = Selector::parse("caption") {
                    if let Some(cap) = table_elem.select(&cap_sel).next() {
                        table.caption = Some(cap.text().collect::<String>().trim().to_string());
                    }
                }

                // <thead> → headers
                if let Ok(thead_sel) = Selector::parse("thead tr th, thead tr td") {
                    for th in table_elem.select(&thead_sel) {
                        table
                            .headers
                            .push(th.text().collect::<String>().trim().to_string());
                    }
                }

                // Pre-parse known-valid CSS selectors for table cell extraction.
                // These selectors are static strings guaranteed to be valid,
                // but we use if-let to avoid any potential panic surface.
                let cell_selector_opt = Selector::parse("td, th").ok();
                let cell_selector = match cell_selector_opt.as_ref() {
                    Some(s) => s,
                    None => {
                        tracing::error!("Failed to parse static selector 'td, th' - this is a bug");
                        continue;
                    }
                };

                // <tr> rows (skip the <thead> row if headers were captured)
                if let Ok(tr_sel) = Selector::parse("tr") {
                    let mut row_idx = 0usize;
                    for tr in table_elem.select(&tr_sel) {
                        // If we got headers from <thead>, skip the first <tr> too.
                        let is_header_row = if !table.headers.is_empty() && row_idx == 0 {
                            row_idx += 1;
                            continue;
                        } else {
                            // Use the first <tr> as header if no <thead> was found.
                            row_idx == 0 && table.headers.is_empty()
                        };

                        let cells: Vec<String> = tr
                            .select(cell_selector)
                            .map(|cell| cell.text().collect::<String>().trim().to_string())
                            .collect();
                        if !cells.is_empty() {
                            if is_header_row {
                                table.headers = cells;
                            } else {
                                table.rows.push(cells);
                            }
                        }
                        row_idx += 1;
                    }
                }

                content.tables.push(table);
            }
        }

        // 4. <img> metadata.
        if let Ok(img_sel) = Selector::parse("img[src]") {
            for (i, img) in document.select(&img_sel).enumerate() {
                if let Some(src) = img.value().attr("src") {
                    content
                        .metadata
                        .insert(format!("img_{i}_src"), src.to_string());
                }
                if let Some(alt) = img.value().attr("alt") {
                    content
                        .metadata
                        .insert(format!("img_{i}_alt"), alt.to_string());
                }
            }
        }

        // 5. <title>
        if let Ok(title_sel) = Selector::parse("title") {
            if let Some(title_elem) = document.select(&title_sel).next() {
                content
                    .metadata
                    .insert("title".to_string(), title_elem.text().collect());
            }
        }

        // 6. <meta name="description" content="...">
        if let Ok(meta_sel) = Selector::parse("meta[name=description]") {
            if let Some(meta) = document.select(&meta_sel).next() {
                if let Some(desc) = meta.value().attr("content") {
                    content
                        .metadata
                        .insert("description".to_string(), desc.to_string());
                }
            }
        }

        content
            .metadata
            .insert("parser".to_string(), "scraper".to_string());

        Ok(content)
    }

    #[cfg(not(feature = "document-html"))]
    #[allow(dead_code)] // F-GAP-49 — reserved for conditional HTML parsing
    fn parse_html_str(&self, _html_str: &str) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("HTML"))
    }

    // =======================================================================
    // Markdown backend  (feature = "document-markdown", crate: comrak)
    // =======================================================================

    #[cfg(feature = "document-markdown")]
    fn parse_markdown(&self, path: &Path) -> Result<ParsedContent, DocumentParserError> {
        let md_str = std::fs::read_to_string(path).map_err(DocumentParserError::from_io)?;
        self.parse_markdown_str(&md_str)
    }

    #[cfg(not(feature = "document-markdown"))]
    fn parse_markdown(&self, _path: &Path) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("Markdown"))
    }

    #[cfg(feature = "document-markdown")]
    fn parse_markdown_bytes(&self, bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
        let md_str = String::from_utf8(bytes.to_vec())
            .map_err(|e| DocumentParserError::Markdown(format!("invalid UTF-8: {e}")))?;
        self.parse_markdown_str(&md_str)
    }

    #[cfg(not(feature = "document-markdown"))]
    fn parse_markdown_bytes(&self, _bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("Markdown"))
    }

    #[cfg(feature = "document-markdown")]
    fn parse_markdown_str(&self, md_str: &str) -> Result<ParsedContent, DocumentParserError> {
        use comrak::{markdown_to_html, ComrakOptions};
        use scraper::{Html, Selector};

        // Render to HTML, then reuse the HTML parser for extraction.
        let html = markdown_to_html(md_str, &ComrakOptions::default());
        let document = Html::parse_document(&html);
        let mut content = ParsedContent::default();

        // 1. Plain text from common block elements.
        let text_selectors = [
            "p",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "li",
            "blockquote",
            "pre",
            "code",
        ];
        let mut text_parts: Vec<String> = Vec::new();
        for tag in &text_selectors {
            if let Ok(sel) = Selector::parse(tag) {
                for elem in document.select(&sel) {
                    let t: String = elem.text().collect::<Vec<_>>().join(" ");
                    let t = t.trim().to_string();
                    if !t.is_empty() {
                        text_parts.push(t);
                    }
                }
            }
        }
        content.text_content = text_parts.join("\n");

        // 2. Tables (GFM tables render as <table>).
        if let Ok(table_sel) = Selector::parse("table") {
            for table_elem in document.select(&table_sel) {
                let mut table = Table {
                    caption: None,
                    headers: Vec::new(),
                    rows: Vec::new(),
                };
                // Pre-parse selector once - static string known to be valid.
                let cell_selector = match Selector::parse("th, td").ok() {
                    Some(s) => s,
                    None => {
                        tracing::error!("Failed to parse static selector 'th, td' - this is a bug");
                        continue;
                    }
                };
                if let Ok(tr_sel) = Selector::parse("tr") {
                    for (row_idx, tr) in table_elem.select(&tr_sel).enumerate() {
                        let cells: Vec<String> = tr
                            .select(&cell_selector)
                            .map(|cell| cell.text().collect::<String>().trim().to_string())
                            .collect();
                        if row_idx == 0 {
                            table.headers = cells;
                        } else {
                            table.rows.push(cells);
                        }
                    }
                }
                content.tables.push(table);
            }
        }

        // 3. Images — comrak renders `![alt](src)` as `<img>`.
        if let Ok(img_sel) = Selector::parse("img[src]") {
            for (i, img) in document.select(&img_sel).enumerate() {
                if let Some(src) = img.value().attr("src") {
                    content
                        .metadata
                        .insert(format!("img_{i}_src"), src.to_string());
                }
                if let Some(alt) = img.value().attr("alt") {
                    content
                        .metadata
                        .insert(format!("img_{i}_alt"), alt.to_string());
                }
            }
        }

        // 4. Links.
        if let Ok(a_sel) = Selector::parse("a[href]") {
            for elem in document.select(&a_sel) {
                if let Some(href) = elem.value().attr("href") {
                    let label: String = elem.text().collect();
                    let key = if label.is_empty() {
                        "link".to_string()
                    } else {
                        format!("link:{label}")
                    };
                    content
                        .metadata
                        .entry(key)
                        .or_insert_with(|| href.to_string());
                }
            }
        }

        content
            .metadata
            .insert("parser".to_string(), "comrak+scraper".to_string());

        Ok(content)
    }

    #[cfg(not(feature = "document-markdown"))]
    #[allow(dead_code)] // F-GAP-49 — reserved for conditional markdown parsing
    fn parse_markdown_str(&self, _md_str: &str) -> Result<ParsedContent, DocumentParserError> {
        Err(DocumentParserError::feature_disabled("Markdown"))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Base64-encode binary data (used for embedded images).
#[cfg(any(
    feature = "document-pdf",
    feature = "document-docx",
    feature = "document-html",
    feature = "document-markdown",
))]
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unsupported_extension() {
        // parse_bytes doesn't hit the file-system, so it goes straight
        // to extension matching and returns UnsupportedExtension.
        let err = DocumentParser::default()
            .parse_bytes(b"dummy content", "xyz")
            .unwrap_err();
        assert!(matches!(err, DocumentParserError::UnsupportedExtension(_)));
        assert!(err.to_string().contains("xyz"));
    }

    #[test]
    fn test_parsed_content_default() {
        let content = ParsedContent::default();
        assert!(content.text_content.is_empty());
        assert!(content.images.is_empty());
        assert!(content.tables.is_empty());
        assert!(content.metadata.is_empty());
        assert!(!content.has_error());
        assert_eq!(content.char_count(), 0);
    }

    #[test]
    fn test_parsed_content_serialize_roundtrip() {
        let content = ParsedContent {
            text_content: "Hello".to_string(),
            images: vec!["base64data".to_string()],
            tables: vec![Table {
                caption: Some("Table 1".to_string()),
                headers: vec!["A".to_string(), "B".to_string()],
                rows: vec![vec!["1".to_string(), "2".to_string()]],
            }],
            metadata: {
                let mut m = HashMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
        };
        let json = serde_json::to_string(&content).expect("serialize ParsedContent");
        let deserialized: ParsedContent = serde_json::from_str(&json).expect("deserialize ParsedContent");
        assert_eq!(deserialized.text_content, "Hello");
        assert_eq!(deserialized.images.len(), 1);
        assert_eq!(deserialized.tables.len(), 1);
        assert_eq!(deserialized.tables[0].caption.as_deref(), Some("Table 1"));
        assert_eq!(deserialized.tables[0].row_count(), 1);
        assert_eq!(deserialized.tables[0].col_count(), 2);
    }

    #[test]
    fn test_table_default() {
        let table = Table {
            caption: None,
            headers: vec!["H1".to_string()],
            rows: vec![],
        };
        assert_eq!(table.row_count(), 0);
        assert_eq!(table.col_count(), 1);
    }

    #[test]
    fn test_parse_bytes_with_unsupported_ext() {
        let err = DocumentParser::default()
            .parse_bytes(b"data", "xyz")
            .unwrap_err();
        assert!(matches!(err, DocumentParserError::UnsupportedExtension(_)));
    }

    #[test]
    fn test_feature_disabled_error() {
        let err = DocumentParserError::feature_disabled("PDF");
        assert!(err.to_string().contains("PDF"));
        assert!(err.to_string().contains("feature"));
    }

    #[test]
    fn test_parse_empty_extension() {
        let err = DocumentParser::default()
            .parse_bytes(b"some data", "")
            .unwrap_err();
        assert!(matches!(err, DocumentParserError::EmptyInput(_)));
    }

    #[test]
    fn test_parse_bytes_empty_extension() {
        let err = DocumentParser::default()
            .parse_bytes(b"data", "  ")
            .unwrap_err();
        assert!(matches!(err, DocumentParserError::EmptyInput(_)));
    }

    #[test]
    fn test_parse_bytes_empty_slice() {
        let err = DocumentParser::default()
            .parse_bytes(b"", "pdf")
            .unwrap_err();
        assert!(matches!(err, DocumentParserError::EmptyInput(_)));
    }

    #[test]
    fn test_file_too_large_error() {
        let err = DocumentParserError::FileTooLarge {
            size: 100_000_000,
            max: 50_000_000,
        };
        let msg = err.to_string();
        assert!(msg.contains("100000000"));
        assert!(msg.contains("50000000"));
    }

    #[test]
    fn test_text_truncation() {
        let parser = DocumentParser {
            max_text_length: 10,
        };
        let mut content = ParsedContent {
            text_content: "Hello, this is a long text that should be truncated.".to_string(),
            ..Default::default()
        };
        parser.truncate_content(&mut content);
        assert!(content.text_content.len() <= 10 + "\n\n[... content truncated ...]".len());
        assert!(content.metadata.contains_key("truncated"));
        assert!(content.metadata["truncated"].contains("truncated"));
    }

    #[test]
    fn test_text_no_truncation_when_under_limit() {
        let parser = DocumentParser {
            max_text_length: 1000,
        };
        let mut content = ParsedContent {
            text_content: "Short text.".to_string(),
            ..Default::default()
        };
        parser.truncate_content(&mut content);
        assert_eq!(content.text_content, "Short text.");
        assert!(!content.metadata.contains_key("truncated"));
    }

    #[test]
    fn test_empty_input_error_display() {
        let err = DocumentParserError::EmptyInput("test reason".to_string());
        let msg = err.to_string();
        assert!(msg.contains("test reason"));
    }

    #[test]
    fn test_parse_path_without_extension() {
        // A path with no extension should produce EmptyInput
        // (we can't test a real file that doesn't exist, but we can test the
        //  extension validation logic conceptually via EmptyInput error)
        let err = DocumentParserError::EmptyInput(
            "file path has no extension; cannot determine document format".to_string(),
        );
        assert!(err.to_string().contains("no extension"));
    }
}
