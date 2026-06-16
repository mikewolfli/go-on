//! PowerPoint (.pptx) document parser using `quick-xml`.
//!
//! A `.pptx` file is a ZIP archive containing XML files. The slide content
//! lives in `ppt/slides/slideN.xml` files. This module extracts text from
//! `<a:t>` (text run) elements within each slide.
//!
//! # Feature gate
//!
//! ```toml
//! document-ppt = ["dep:quick-xml", "dep:zip"]
//! ```

use crate::multimodal::document_parser::{DocumentParserError, ParsedContent};

/// Parse PowerPoint `.pptx` bytes and return extracted content.
///
/// Opens the ZIP archive, reads `ppt/slides/slideN.xml` entries, and extracts
/// all `<a:t>` text elements. Metadata includes the number of slides found.
#[cfg(feature = "document-ppt")]
pub fn parse_pptx_bytes(bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
    use std::io::Read;

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| DocumentParserError::Other(format!("PPTX ZIP error: {e}")))?;

    let mut content = ParsedContent::default();
    let mut slide_texts: Vec<String> = Vec::new();

    // Collect all slide XML entries sorted by slide number, then read them.
    let mut slide_entries: Vec<(u32, Vec<u8>)> = Vec::new();
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
        }
    }
    slide_entries.sort_by_key(|(num, _)| *num);

    for (slide_num, xml_bytes) in &slide_entries {
        let xml_str = String::from_utf8(xml_bytes.clone()).map_err(|e| {
            DocumentParserError::Other(format!("PPTX slide XML not valid UTF-8: {e}"))
        })?;

        let text = extract_text_from_slide_xml(&xml_str);
        if !text.is_empty() {
            slide_texts.push(format!("--- Slide {slide_num} ---\n{text}"));
        }
    }

    content.text_content = slide_texts.join("\n\n");
    content
        .metadata
        .insert("slide_count".to_string(), slide_texts.len().to_string());
    content
        .metadata
        .insert("parser".to_string(), "quick-xml".to_string());

    Ok(content)
}

/// Extract visible text from a single slide's XML content.
///
/// Looks for `<a:t>` elements (text runs in DrawingML / PowerPoint) and
/// collects their text content. Each paragraph (delimited by `<a:p>`) is
/// joined as a separate line.
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
                    if let Ok(t) = e.unescape() {
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

    // Flush any remaining paragraph content.
    if !current_paragraph.is_empty() {
        text_runs.push(current_paragraph.join(" "));
    }

    text_runs.join("\n")
}

/// Placeholder for when the feature is disabled.
#[cfg(not(feature = "document-ppt"))]
pub fn parse_pptx_bytes(_bytes: &[u8]) -> Result<ParsedContent, DocumentParserError> {
    Err(DocumentParserError::feature_disabled("PPT"))
}
