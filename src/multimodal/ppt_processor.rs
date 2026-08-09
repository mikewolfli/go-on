//! PowerPoint (.pptx) document parser using `quick-xml`.
//!
//! A `.pptx` file is a ZIP archive containing XML files. The slide content
//! lives in `ppt/slides/slideN.xml` files, while notes live in
//! `ppt/notesSlides/notesSlideN.xml`. This module extracts:
//!
//! - Text from `<a:t>` (text run) elements within each slide
//! - Speaker notes from each slide's notes slide
//! - Tables discovered in slides
//! - Image metadata (name, position, size) from `<p:pic>` elements
//!
//! # Feature gate
//!
//! ```toml
//! document-ppt = ["dep:quick-xml", "dep:zip"]
//! ```
//!
//! # Public types
//!
//! - [`ParsedPresentation`] — rich representation of a parsed presentation
//!   with per-slide metadata, notes, tables, and image information.

use serde::{Deserialize, Serialize};

use crate::multimodal::document_parser::{DocumentParserError, ParsedContent, Table};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Rich representation of a parsed PowerPoint presentation.
///
/// This struct exposes per-slide metadata (notes, table/image counts, extracted
/// tables and image metadata) in addition to the plain-text content that is
/// returned via [`ParsedContent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedPresentation {
    /// Per-slide information in slide-number order.
    pub slides: Vec<Slide>,
    /// Total number of slides parsed.
    pub slide_count: usize,
    /// Whether any slide contains speaker notes.
    pub has_notes: bool,
    /// Whether any slide contains tables.
    pub has_tables: bool,
    /// Whether any slide contains images.
    pub has_images: bool,
}

/// Information extracted from a single slide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slide {
    /// Slide number (1-based).
    pub slide_number: u32,
    /// Plain text content extracted from text elements.
    pub text_content: String,
    /// Speaker notes for this slide, if any.
    pub notes: Option<String>,
    /// Number of tables found on this slide.
    pub table_count: usize,
    /// Number of images found on this slide.
    pub image_count: usize,
    /// Tables discovered on this slide (rows of cell text).
    #[serde(default)]
    pub tables: Vec<Table>,
    /// Image metadata entries for this slide.
    #[serde(default)]
    pub images: Vec<SlideImage>,
}

/// Metadata about an image placed on a slide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideImage {
    /// Image name from the slide XML (`cNvPr` name attribute).
    pub name: String,
    /// Horizontal position in EMUs (English Metric Units).
    /// 1 inch = 914400 EMUs, 1 cm = 360000 EMUs.
    pub x: i64,
    /// Vertical position in EMUs.
    pub y: i64,
    /// Image width in EMUs.
    pub width: i64,
    /// Image height in EMUs.
    pub height: i64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse PowerPoint `.pptx` bytes and return extracted content.
///
/// Opens the ZIP archive, reads `ppt/slides/slideN.xml` entries, extracts text
/// from `<a:t>` elements, speaker notes from `ppt/notesSlides/`, tables from
/// `<a:tbl>` elements, and image metadata from `<p:pic>` elements. Metadata
/// includes the number of slides, notes, tables, and images found.
#[cfg(feature = "document-ppt")]
pub fn parse_pptx_bytes(bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
    use std::io::Read;

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| DocumentParserError::Other(format!("PPTX ZIP error: {e}")))?;

    // ── Collect all ZIP entry bytes we need ────────────────────────────
    // We need: slide XML, notes slide XML.
    let mut slide_entries: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut notes_entries: Vec<(u32, Vec<u8>)> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.name().to_string();

        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            if let Some(num_str) = name
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .split('.')
                .next()
            {
                if let Ok(num) = num_str.parse::<u32>() {
                    let mut xml_bytes = Vec::new();
                    if entry.read_to_end(&mut xml_bytes).is_ok() {
                        slide_entries.push((num, xml_bytes));
                    }
                }
            }
        } else if name.starts_with("ppt/notesSlides/notesSlide") && name.ends_with(".xml") {
            if let Some(num_str) = name
                .trim_start_matches("ppt/notesSlides/notesSlide")
                .trim_end_matches(".xml")
                .split('.')
                .next()
            {
                if let Ok(num) = num_str.parse::<u32>() {
                    let mut xml_bytes = Vec::new();
                    if entry.read_to_end(&mut xml_bytes).is_ok() {
                        notes_entries.push((num, xml_bytes));
                    }
                }
            }
        }
    }

    slide_entries.sort_by_key(|(num, _)| *num);
    notes_entries.sort_by_key(|(num, _)| *num);

    // Build a quick-lookup map for notes by slide number.
    let notes_map: std::collections::HashMap<u32, Vec<u8>> = notes_entries.into_iter().collect();

    // ── Parse each slide ──────────────────────────────────────────────
    let mut slides: Vec<Slide> = Vec::new();
    let mut has_notes = false;
    let mut has_tables = false;
    let mut has_images = false;
    let mut all_slide_texts: Vec<String> = Vec::new();

    for (slide_num, xml_bytes) in &slide_entries {
        let xml_str = String::from_utf8(xml_bytes.clone()).map_err(|e| {
            DocumentParserError::Other(format!("PPTX slide XML not valid UTF-8: {e}"))
        })?;

        // ── Extract text ──────────────────────────────────────────────
        let text = extract_text_from_slide_xml(&xml_str);

        // ── Extract tables ────────────────────────────────────────────
        let tables = extract_tables_from_slide_xml(&xml_str);
        if !tables.is_empty() {
            has_tables = true;
        }

        // ── Extract image metadata ────────────────────────────────────
        let images = extract_images_from_slide_xml(&xml_str);
        if !images.is_empty() {
            has_images = true;
        }

        // ── Extract notes ─────────────────────────────────────────────
        let notes = notes_map.get(slide_num).and_then(|notes_bytes| {
            let notes_str = String::from_utf8(notes_bytes.clone()).ok()?;
            let notes_text = extract_text_from_slide_xml(&notes_str);
            let trimmed = notes_text.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });
        if notes.is_some() {
            has_notes = true;
        }

        let slide = Slide {
            slide_number: *slide_num,
            text_content: text.clone(),
            notes: notes.clone(),
            table_count: tables.len(),
            image_count: images.len(),
            tables,
            images,
        };

        // Build the flat text representation.
        let mut slide_parts = Vec::new();
        if !slide.text_content.is_empty() {
            slide_parts.push(slide.text_content.clone());
        }
        if let Some(ref n) = slide.notes {
            slide_parts.push(format!("[Notes]\n{n}"));
        }
        if slide.table_count > 0 {
            slide_parts.push(format!("[Tables: {} found]", slide.table_count));
        }
        if slide.image_count > 0 {
            slide_parts.push(format!("[Images: {} found]", slide.image_count));
        }

        let combined = if slide_parts.is_empty() {
            String::new()
        } else {
            slide_parts.join("\n\n")
        };

        if !combined.is_empty() {
            all_slide_texts.push(format!("--- Slide {slide_num} ---\n{combined}"));
        }

        slides.push(slide);
    }

    let presentation = ParsedPresentation {
        slide_count: slides.len(),
        slides,
        has_notes,
        has_tables,
        has_images,
    };

    Ok(parsed_presentation_to_content(
        &presentation,
        &all_slide_texts,
    ))
}

// ---------------------------------------------------------------------------
// Text extraction from slide XML
// ---------------------------------------------------------------------------

/// Extract visible text from a single slide's XML content.
///
/// Looks for `<a:t>` / `<t>` elements (text runs in DrawingML / PowerPoint) and
/// collects their text content. Each paragraph (delimited by `<a:p>` / `<p>`)
/// is joined as a separate line.
#[cfg(feature = "document-ppt")]
fn extract_text_from_slide_xml(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    let mut in_text_tag = false;
    let mut text_runs: Vec<String> = Vec::new();
    let mut current_paragraph: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let tag_name = e.name().as_ref().to_ascii_lowercase();
                if tag_name == b"a:t" || tag_name == b"t" {
                    in_text_tag = true;
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_text_tag {
                    if let Ok(t) = e.decode() {
                        let trimmed = t.trim().to_string();
                        if !trimmed.is_empty() {
                            current_paragraph.push(trimmed);
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag_name = e.name().as_ref().to_ascii_lowercase();
                if tag_name == b"a:t" || tag_name == b"t" {
                    in_text_tag = false;
                } else if (tag_name == b"a:p" || tag_name == b"p") && !current_paragraph.is_empty()
                {
                    text_runs.push(current_paragraph.join(" "));
                    current_paragraph.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!(error = %e, "PPTX XML parse warning");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    if !current_paragraph.is_empty() {
        text_runs.push(current_paragraph.join(" "));
    }

    text_runs.join("\n")
}

// ---------------------------------------------------------------------------
// Table extraction
// ---------------------------------------------------------------------------

/// Extract tables from a single slide's XML.
///
/// Looks for `<a:tbl>` elements and extracts cell text from each `<a:tc>`.
/// Each table row (`<a:tr>`) becomes a data row in the returned [`Table`].
/// PPTX tables do not have a semantic header row, so `headers` is left empty.
#[cfg(feature = "document-ppt")]
fn extract_tables_from_slide_xml(xml: &str) -> Vec<Table> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    let mut tables: Vec<Table> = Vec::new();
    let mut in_table = false;

    // Current table being built.
    let mut current_rows: Vec<Vec<String>> = Vec::new();
    let mut current_cells: Vec<String> = Vec::new();
    let mut in_row = false;
    let mut in_cell = false;
    let mut in_cell_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag_name = e.name().as_ref().to_ascii_lowercase();

                if tag_name == b"a:tbl" && !in_table {
                    in_table = true;
                    current_rows.clear();
                    current_cells.clear();
                } else if tag_name == b"a:tr" && in_table {
                    in_row = true;
                    current_cells.clear();
                } else if tag_name == b"a:tc" && in_row {
                    in_cell = true;
                    current_cells.clear();
                } else if (tag_name == b"a:t" || tag_name == b"t") && in_cell {
                    in_cell_text = true;
                }
            }
            Ok(Event::Empty(ref e)) => {
                let tag_name = e.name().as_ref().to_ascii_lowercase();
                if tag_name == b"a:tc" && in_row {
                    // Empty cell — push an empty string.
                    current_cells.push(String::new());
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_cell_text {
                    if let Ok(t) = e.decode() {
                        let trimmed = t.trim().to_string();
                        if !trimmed.is_empty() {
                            current_cells.push(trimmed);
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag_name = e.name().as_ref().to_ascii_lowercase();

                if tag_name == b"a:tbl" && in_table {
                    // Finish this table.
                    if !current_rows.is_empty() {
                        let max_cols = current_rows.iter().map(|r| r.len()).max().unwrap_or(0);
                        // Pad rows to equal column count.
                        let padded_rows: Vec<Vec<String>> = current_rows
                            .iter()
                            .map(|r| {
                                let mut row = r.clone();
                                while row.len() < max_cols {
                                    row.push(String::new());
                                }
                                row
                            })
                            .collect();
                        tables.push(Table {
                            caption: None,
                            headers: Vec::new(),
                            rows: padded_rows,
                        });
                    }
                    in_table = false;
                    current_rows.clear();
                } else if tag_name == b"a:tr" && in_row {
                    in_row = false;
                    if !current_cells.is_empty() {
                        current_rows.push(std::mem::take(&mut current_cells));
                    }
                } else if tag_name == b"a:tc" && in_cell {
                    in_cell = false;
                    // Collect any text accumulated for this cell.
                    // There could be multiple `<a:t>` runs in one cell.
                    // If nothing was pushed, push an empty string.
                    // Actually, since we collect text as individual pushes,
                    // if multiple text runs exist they'll be separate entries.
                    // Join them.
                    if current_cells.len() > 1 {
                        let joined = current_cells.join(" ");
                        current_cells.clear();
                        current_cells.push(joined);
                    } else if current_cells.is_empty() {
                        // Empty cell with an End event (non-self-closing).
                        current_cells.push(String::new());
                    }
                    // If exactly one entry, it's already correct.
                } else if (tag_name == b"a:t" || tag_name == b"t") && in_cell_text {
                    in_cell_text = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!(error = %e, "PPTX table XML parse warning");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    tables
}

// ---------------------------------------------------------------------------
// Image metadata extraction
// ---------------------------------------------------------------------------

/// Extract image metadata from a single slide's XML.
///
/// Looks for `<p:pic>` elements and extracts:
/// - Image name from `<p:cNvPr name="...">`
/// - Position (`<a:off x="..." y="...">`) and size (`<a:ext cx="..." cy="...">`)
///   from `<p:xfrm>` or `<a:xfrm>`
#[cfg(feature = "document-ppt")]
fn extract_images_from_slide_xml(xml: &str) -> Vec<SlideImage> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    let mut images: Vec<SlideImage> = Vec::new();
    let mut in_pic = false;
    let mut _in_cnv_pr = false;
    let mut in_xfrm = false;

    let mut current_name = String::new();
    let mut current_x: i64 = 0;
    let mut current_y: i64 = 0;
    let mut current_cx: i64 = 0;
    let mut current_cy: i64 = 0;
    let mut _in_ext = false;
    let mut _in_off = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag_name = e.name().as_ref().to_ascii_lowercase();

                if tag_name == b"p:pic" && !in_pic {
                    in_pic = true;
                    current_name.clear();
                    current_x = 0;
                    current_y = 0;
                    current_cx = 0;
                    current_cy = 0;
                } else if tag_name == b"p:cnvpr" && in_pic {
                    _in_cnv_pr = true;
                    // Try to get the name attribute.
                    for attr in e.attributes().flatten() {
                        let attr_name = attr.key.as_ref().to_ascii_lowercase();
                        if attr_name == b"name" || attr_name == b"descr" {
                            // In practice, `name` is the attribute on `cNvPr`.
                            // quick-xml prefixes namespaced attrs, so we check
                            // the local part.
                        }
                        // Access key.local_name() or check the full key.
                        let key_ref = attr.key.as_ref();
                        if key_ref.ends_with(b"name") || key_ref.ends_with(b":name") {
                            if let Ok(v) = std::str::from_utf8(attr.value.as_ref()) {
                                current_name = v.to_string();
                            }
                        }
                    }
                } else if (tag_name == b"p:xfrm" || tag_name == b"a:xfrm") && in_pic {
                    in_xfrm = true;
                } else if tag_name == b"a:off" && in_xfrm {
                    _in_off = true;
                    for attr in e.attributes().flatten() {
                        let key_ref = attr.key.as_ref();
                        if key_ref.ends_with(b"x") || key_ref.ends_with(b":x") {
                            if let Ok(v) = std::str::from_utf8(attr.value.as_ref()) {
                                current_x = v.parse().unwrap_or(0);
                            }
                        }
                        if key_ref.ends_with(b"y") || key_ref.ends_with(b":y") {
                            if let Ok(v) = std::str::from_utf8(attr.value.as_ref()) {
                                current_y = v.parse().unwrap_or(0);
                            }
                        }
                    }
                } else if tag_name == b"a:ext" && in_xfrm {
                    _in_ext = true;
                    for attr in e.attributes().flatten() {
                        let key_ref = attr.key.as_ref();
                        // cx = width, cy = height in DrawingML.
                        if key_ref.ends_with(b"cx") || key_ref.ends_with(b":cx") {
                            if let Ok(v) = std::str::from_utf8(attr.value.as_ref()) {
                                current_cx = v.parse().unwrap_or(0);
                            }
                        }
                        if key_ref.ends_with(b"cy") || key_ref.ends_with(b":cy") {
                            if let Ok(v) = std::str::from_utf8(attr.value.as_ref()) {
                                current_cy = v.parse().unwrap_or(0);
                            }
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag_name = e.name().as_ref().to_ascii_lowercase();

                if tag_name == b"p:pic" && in_pic {
                    images.push(SlideImage {
                        name: if current_name.is_empty() {
                            format!("Image_{}", images.len() + 1)
                        } else {
                            current_name.clone()
                        },
                        x: current_x,
                        y: current_y,
                        width: current_cx,
                        height: current_cy,
                    });
                    in_pic = false;
                    _in_cnv_pr = false;
                    in_xfrm = false;
                    _in_off = false;
                    _in_ext = false;
                } else if tag_name == b"p:cnvpr" {
                    _in_cnv_pr = false;
                } else if tag_name == b"p:xfrm" || tag_name == b"a:xfrm" {
                    in_xfrm = false;
                    _in_off = false;
                    _in_ext = false;
                } else if tag_name == b"a:off" {
                    _in_off = false;
                } else if tag_name == b"a:ext" {
                    _in_ext = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!(error = %e, "PPTX image XML parse warning");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    images
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a rich [`ParsedPresentation`] into a flat [`ParsedContent`] suitable
/// for the caller.
#[cfg(feature = "document-ppt")]
fn parsed_presentation_to_content(
    presentation: &ParsedPresentation,
    slide_texts: &[String],
) -> ParsedContent {
    // Collect all tables from all slides.
    let all_tables: Vec<Table> = presentation
        .slides
        .iter()
        .flat_map(|s| s.tables.clone())
        .collect();

    let table_count = all_tables.len();
    let total_images: usize = presentation.slides.iter().map(|s| s.image_count).sum();

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "slide_count".to_string(),
        presentation.slide_count.to_string(),
    );
    metadata.insert("has_notes".to_string(), presentation.has_notes.to_string());
    metadata.insert(
        "has_tables".to_string(),
        presentation.has_tables.to_string(),
    );
    metadata.insert("table_count".to_string(), table_count.to_string());
    metadata.insert(
        "has_images".to_string(),
        presentation.has_images.to_string(),
    );
    metadata.insert("image_count".to_string(), total_images.to_string());
    metadata.insert("parser".to_string(), "quick-xml".to_string());

    ParsedContent {
        text_content: slide_texts.join("\n\n"),
        tables: all_tables,
        metadata,
        ..Default::default()
    }
}
