//! PowerPoint (.pptx) file reader using `quick-xml` and `zip`.
//!
//! This module provides a straightforward API for reading text content from
//! PowerPoint `.pptx` files. A `.pptx` file is a ZIP archive whose slides are
//! stored as XML files in `ppt/slides/`. This reader extracts:
//!
//! - Visible text from `<a:t>` (text run) elements on each slide
//! - Speaker notes from `ppt/notesSlides/` (when present)
//!
//! # Feature gate
//!
//! ```toml
//! document-ppt = ["dep:quick-xml", "dep:zip"]
//! ```
//!
//! # Usage
//!
//! ```no_run
//! use go_on::multimodal::ppt_reader::read_pptx_file;
//!
//! match read_pptx_file("/path/to/presentation.pptx") {
//!     Ok(content) => println!("{}", content),
//!     Err(e) => eprintln!("Error: {e}"),
//! }
//! ```

use std::io::Read;
use std::path::Path;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Error type for PPT reading operations.
#[derive(Debug, Error)]
#[allow(dead_code, reason = "F-GAP reserved: PPT reader API")]
pub enum PptReaderError {
    /// I/O error when reading the file.
    #[error("PPTX I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a valid ZIP archive.
    #[error("PPTX ZIP error: {0}")]
    Zip(String),
    /// The XML content is not valid UTF-8.
    #[error("PPTX XML encoding error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    /// No slides found in the presentation.
    #[error("no slides found in PPTX archive")]
    NoSlides,
    /// Feature is not enabled.
    #[error("feature document-ppt is not enabled")]
    FeatureDisabled,
}

/// The text content extracted from a PowerPoint presentation.
#[derive(Debug, Clone)]
#[allow(dead_code, reason = "F-GAP reserved: PPT reader API")]
pub struct PptxContent {
    /// The combined text of all slides, separated by slide boundaries.
    pub full_text: String,
    /// Number of slides parsed.
    pub slide_count: usize,
    /// Whether any speaker notes were found.
    pub has_notes: bool,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read a `.pptx` file from disk and return its text content.
///
/// Opens the file, decompresses the ZIP archive, parses each slide's XML,
/// and extracts visible text content plus speaker notes.
///
/// # Errors
///
/// Returns `PptReaderError` if:
/// - The file cannot be opened or read.
/// - The file is not a valid ZIP archive.
/// - Slide XML is not valid UTF-8.
/// - No slides are found in the archive.
#[cfg(feature = "document-ppt")]
#[allow(dead_code, reason = "F-GAP reserved: PPT reader API")]
pub fn read_pptx_file<P: AsRef<Path>>(path: P) -> Result<PptxContent, PptReaderError> {
    let file = std::fs::File::open(path.as_ref())?;
    let mut reader = std::io::BufReader::new(file);
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    read_pptx_bytes(&buffer)
}

/// Read a `.pptx` file from a byte slice and return its text content.
///
/// This is useful when the file bytes have already been loaded into memory
/// (e.g., downloaded from a remote source).
///
/// # Errors
///
/// Same as [`read_pptx_file`].
#[cfg(feature = "document-ppt")]
#[allow(dead_code, reason = "F-GAP reserved: PPT reader API")]
pub fn read_pptx_bytes(bytes: &[u8]) -> Result<PptxContent, PptReaderError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| PptReaderError::Zip(format!("failed to open ZIP archive: {e}")))?;

    // ── Collect slide and notes entries ────────────────────────────────
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

    if slide_entries.is_empty() {
        return Err(PptReaderError::NoSlides);
    }

    let notes_map: std::collections::HashMap<u32, Vec<u8>> = notes_entries.into_iter().collect();

    // ── Parse each slide ──────────────────────────────────────────────
    let mut slide_texts: Vec<String> = Vec::new();
    let mut has_notes = false;

    for (slide_num, xml_bytes) in &slide_entries {
        let xml_str = String::from_utf8(xml_bytes.clone())?;
        let text = extract_text_from_slide(&xml_str);

        // Extract notes if available.
        let notes = notes_map.get(slide_num).and_then(|notes_bytes| {
            let notes_str = String::from_utf8(notes_bytes.clone()).ok()?;
            let notes_text = extract_text_from_slide(&notes_str);
            let trimmed = notes_text.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        let mut combined = text.trim().to_string();
        if let Some(ref n) = notes {
            combined.push_str("\n\n[Notes]\n");
            combined.push_str(n);
            has_notes = true;
        }

        if !combined.is_empty() {
            slide_texts.push(format!("--- Slide {slide_num} ---\n{combined}"));
        }
    }

    Ok(PptxContent {
        full_text: slide_texts.join("\n\n"),
        slide_count: slide_texts.len(),
        has_notes,
    })
}

/// Placeholder for when the feature is disabled.
#[cfg(not(feature = "document-ppt"))]
pub fn read_pptx_file<P: AsRef<Path>>(_path: P) -> Result<PptxContent, PptReaderError> {
    Err(PptReaderError::FeatureDisabled)
}

/// Placeholder for when the feature is disabled.
#[cfg(not(feature = "document-ppt"))]
pub fn read_pptx_bytes(_bytes: &[u8]) -> Result<PptxContent, PptReaderError> {
    Err(PptReaderError::FeatureDisabled)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract visible text from a slide's XML content.
///
/// Finds `<a:t>` / `<t>` text-run elements and joins them by paragraph
/// (delimited by `<a:p>` / `<p>`).
#[cfg(feature = "document-ppt")]
#[allow(dead_code, reason = "F-GAP reserved: PPT reader API")]
fn extract_text_from_slide(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    let mut in_text_tag = false;
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current_para: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let tag = e.name().as_ref().to_ascii_lowercase();
                if tag == b"a:t" || tag == b"t" {
                    in_text_tag = true;
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_text_tag {
                    if let Ok(t) = e.unescape() {
                        let trimmed = t.trim().to_string();
                        if !trimmed.is_empty() {
                            current_para.push(trimmed);
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag = e.name().as_ref().to_ascii_lowercase();
                if tag == b"a:t" || tag == b"t" {
                    in_text_tag = false;
                } else if (tag == b"a:p" || tag == b"p") && !current_para.is_empty() {
                    paragraphs.push(current_para.join(" "));
                    current_para.clear();
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

    // Flush any remaining paragraph.
    if !current_para.is_empty() {
        paragraphs.push(current_para.join(" "));
    }

    paragraphs.join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "document-ppt")]
mod tests {
    use super::*;
    use std::io::Write;

    /// Create a minimal valid .pptx in memory with a single slide.
    fn create_minimal_pptx(slide_xml: &str) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));

            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            // [Content_Types].xml
            zip.start_file("[Content_Types].xml", opts.clone()).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>"#,
            )
            .unwrap();

            // ppt/presentation.xml
            zip.start_file("ppt/presentation.xml", opts.clone())
                .unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>"#,
            )
            .unwrap();

            // ppt/_rels/presentation.xml.rels
            zip.start_file("ppt/_rels/presentation.xml.rels", opts.clone())
                .unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"#,
            )
            .unwrap();

            // The actual slide.
            zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
            zip.write_all(slide_xml.as_bytes()).unwrap();

            zip.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn test_read_simple_slide() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
        xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:spTree>
    <p:nvGrpSpPr><p:cNvPr name=""/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr/>
    <p:sp>
      <p:nvSpPr><p:cNvPr name="Title 1"/><p:nvSpPr><p:ph type="title"/></p:nvSpPr></p:nvSpPr>
      <p:spPr/><p:txBody>
        <a:p><a:r><a:t>Hello, World!</a:t></a:r></a:p>
        <a:p><a:r><a:t>Second paragraph.</a:t></a:r></a:p>
      </p:txBody>
    </p:sp>
  </p:spTree>
</p:sld>"#;

        let bytes = create_minimal_pptx(slide_xml);
        let result = read_pptx_bytes(&bytes).unwrap();
        assert_eq!(result.slide_count, 1);
        assert!(result.full_text.contains("Hello, World!"));
        assert!(result.full_text.contains("Second paragraph."));
        assert!(!result.has_notes);
    }

    #[test]
    fn test_read_pptx_with_notes() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
        xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:spTree>
    <p:nvGrpSpPr><p:cNvPr name=""/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr/>
    <p:sp>
      <p:nvSpPr><p:cNvPr name="Content"/><p:nvSpPr><p:ph type="body"/></p:nvSpPr></p:nvSpPr>
      <p:spPr/><p:txBody>
        <a:p><a:r><a:t>Slide content</a:t></a:r></a:p>
      </p:txBody>
    </p:sp>
  </p:spTree>
</p:sld>"#;

        let notes_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
          xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:nvNotePr><p:cNvPr name="Notes"/><p:nvPr/></p:nvNotePr>
  <p:txBody>
    <a:p><a:r><a:t>These are speaker notes.</a:t></a:r></a:p>
  </p:txBody>
</p:notes>"#;

        // Build a PPTX with both slide and notes.
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            zip.start_file("[Content_Types].xml", opts.clone()).unwrap();
            zip.write_all(b"").unwrap();

            zip.start_file("ppt/presentation.xml", opts.clone())
                .unwrap();
            zip.write_all(b"").unwrap();

            zip.start_file("ppt/_rels/presentation.xml.rels", opts.clone())
                .unwrap();
            zip.write_all(b"").unwrap();

            zip.start_file("ppt/slides/slide1.xml", opts.clone())
                .unwrap();
            zip.write_all(slide_xml.as_bytes()).unwrap();

            zip.start_file("ppt/notesSlides/notesSlide1.xml", opts)
                .unwrap();
            zip.write_all(notes_xml.as_bytes()).unwrap();

            zip.finish().unwrap();
        }

        let result = read_pptx_bytes(&buffer).unwrap();
        assert_eq!(result.slide_count, 1);
        assert!(result.full_text.contains("Slide content"));
        assert!(result.full_text.contains("These are speaker notes."));
        assert!(result.has_notes);
    }

    #[test]
    fn test_read_pptx_no_slides_fails() {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("[Content_Types].xml", opts).unwrap();
            zip.write_all(b"").unwrap();
            zip.finish().unwrap();
        }

        let result = read_pptx_bytes(&buffer);
        assert!(result.is_err());
        match result.unwrap_err() {
            PptReaderError::NoSlides => {} // expected
            other => panic!("expected NoSlides error, got: {other}"),
        }
    }

    #[test]
    fn test_read_pptx_invalid_zip() {
        let result = read_pptx_bytes(b"not a zip file at all");
        assert!(result.is_err());
        match result.unwrap_err() {
            PptReaderError::Zip(_) => {} // expected
            other => panic!("expected Zip error, got: {other}"),
        }
    }

    #[test]
    fn test_read_pptx_multiple_slides() {
        let slide1_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
        xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:spTree>
    <p:nvGrpSpPr><p:cNvPr name=""/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr/>
    <p:sp><p:txBody><a:p><a:r><a:t>Slide 1 Content</a:t></a:r></a:p></p:txBody></p:sp>
  </p:spTree>
</p:sld>"#;
        let slide2_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
        xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:spTree>
    <p:nvGrpSpPr><p:cNvPr name=""/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr/>
    <p:sp><p:txBody><a:p><a:r><a:t>Slide 2 Content</a:t></a:r></a:p></p:txBody></p:sp>
  </p:spTree>
</p:sld>"#;

        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            zip.start_file("[Content_Types].xml", opts.clone()).unwrap();
            zip.write_all(b"").unwrap();

            zip.start_file("ppt/presentation.xml", opts.clone())
                .unwrap();
            zip.write_all(b"").unwrap();

            zip.start_file("ppt/_rels/presentation.xml.rels", opts.clone())
                .unwrap();
            zip.write_all(b"").unwrap();

            zip.start_file("ppt/slides/slide1.xml", opts.clone())
                .unwrap();
            zip.write_all(slide1_xml.as_bytes()).unwrap();

            zip.start_file("ppt/slides/slide2.xml", opts).unwrap();
            zip.write_all(slide2_xml.as_bytes()).unwrap();

            zip.finish().unwrap();
        }

        let result = read_pptx_bytes(&buffer).unwrap();
        assert_eq!(result.slide_count, 2);
        assert!(result.full_text.contains("Slide 1 Content"));
        assert!(result.full_text.contains("Slide 2 Content"));
    }
}
