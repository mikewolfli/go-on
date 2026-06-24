//! Office document tools (Excel, PowerPoint, DOCX)
//!
//! Provides `ReadExcelTool` for reading `.xlsx` files, `WriteExcelTool` for
//! writing `.xlsx` files, `ReadPptTool` for reading `.pptx` files, `WritePptTool`
//! for creating `.pptx` files, and `WriteDocxTool` for creating `.docx` files.
//!
//! - `ReadExcelTool` is only compiled when `feature = "document-excel"` is enabled.
//! - `WriteExcelTool` is only compiled when `feature = "document-excel-write"` is enabled.
//! - `ReadPptTool` / `WritePptTool` is only compiled when `feature = "document-ppt"` is enabled.
//! - `WriteDocxTool` is only compiled when `feature = "document-docx"` is enabled.

#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write",
    feature = "document-docx"
))]
use crate::governance::pua::tool_execution_report;
#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write",
    feature = "document-docx"
))]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write",
    feature = "document-docx"
))]
use anyhow::{Context, Result};
#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write",
    feature = "document-docx"
))]
use std::fs;
#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write",
    feature = "document-docx"
))]
use tracing::info;

// ── ReadExcelTool ──────────────────────────────────────────────────────────

#[cfg(feature = "document-excel")]
pub struct ReadExcelTool;

#[cfg(feature = "document-excel")]
impl Tool for ReadExcelTool {
    fn name(&self) -> &'static str {
        "read_excel"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' in payload"))?;

        let validated = sanitize_path(input, path)?;

        let bytes = fs::read(&validated).context("failed to read Excel file")?;

        let parsed = crate::multimodal::excel_processor::parse_excel_bytes(&bytes)
            .map_err(|e| anyhow::anyhow!("Excel parse error: {e}"))?;

        info!(
            path = %validated.display(),
            char_count = parsed.char_count(),
            "tool: Excel file read successfully"
        );

        Ok(ToolOutput {
            success: !parsed.has_error(),
            result: Some(serde_json::json!({
                "text": parsed.text_content,
                "metadata": parsed.metadata,
                "char_count": parsed.char_count(),
            })),
            error: parsed.error_message().map(|s| s.to_string()),
            verification: Some("excel_read".to_string()),
            audit_log: Some(format!(
                "Read Excel file: {} ({} chars, {} sheets)",
                validated.display(),
                parsed.char_count(),
                parsed.metadata.get("sheet_count").unwrap_or(&String::new()),
            )),
            pua_report: Some(tool_execution_report("read_excel", Some("excel_read"))),
        })
    }
}

// ── ReadPptTool ────────────────────────────────────────────────────────────

#[cfg(feature = "document-ppt")]
pub struct ReadPptTool;

#[cfg(feature = "document-ppt")]
impl Tool for ReadPptTool {
    fn name(&self) -> &'static str {
        "read_ppt"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' in payload"))?;

        let validated = sanitize_path(input, path)?;

        let bytes = fs::read(&validated).context("failed to read PowerPoint file")?;

        let parsed = crate::multimodal::ppt_processor::parse_pptx_bytes(&bytes)
            .map_err(|e| anyhow::anyhow!("PPT parse error: {e}"))?;

        info!(
            path = %validated.display(),
            char_count = parsed.char_count(),
            "tool: PowerPoint file read successfully"
        );

        Ok(ToolOutput {
            success: !parsed.has_error(),
            result: Some(serde_json::json!({
                "text": parsed.text_content,
                "metadata": parsed.metadata,
                "char_count": parsed.char_count(),
            })),
            error: parsed.error_message().map(|s| s.to_string()),
            verification: Some("ppt_read".to_string()),
            audit_log: Some(format!(
                "Read PowerPoint file: {} ({} chars, {} slides)",
                validated.display(),
                parsed.char_count(),
                parsed.metadata.get("slide_count").unwrap_or(&String::new()),
            )),
            pua_report: Some(tool_execution_report("read_ppt", Some("ppt_read"))),
        })
    }
}

// ── WriteExcelTool ─────────────────────────────────────────────────────────

#[cfg(feature = "document-excel-write")]
pub struct WriteExcelTool;

#[cfg(feature = "document-excel-write")]
impl Tool for WriteExcelTool {
    fn name(&self) -> &'static str {
        "write_excel"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' in payload"))?;

        let validated = sanitize_path(input, path)?;

        // Parse the sheet configuration from the payload
        let config: crate::multimodal::excel_writer::WriteExcelConfig =
            serde_json::from_value(input.payload["config"].clone())
                .map_err(|e| anyhow::anyhow!("invalid 'config' in payload: {e}"))?;

        let bytes = crate::multimodal::excel_writer::write_excel_bytes(&config)
            .map_err(|e| anyhow::anyhow!("Excel write error: {e}"))?;

        fs::write(&validated, &bytes).context("failed to write Excel file")?;

        let sheet_count = config.sheets.len();
        let row_count: usize = config.sheets.iter().map(|s| s.rows.len()).sum();

        info!(
            path = %validated.display(),
            sheets = sheet_count,
            rows = row_count,
            "tool: Excel file written successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated.to_string_lossy(),
                "sheets": sheet_count,
                "rows": row_count,
                "size_bytes": bytes.len(),
            })),
            error: None,
            verification: Some("excel_write".to_string()),
            audit_log: Some(format!(
                "Wrote Excel file: {} ({} sheets, {} rows, {} bytes)",
                validated.display(),
                sheet_count,
                row_count,
                bytes.len(),
            )),
            pua_report: Some(tool_execution_report("write_excel", Some("excel_write"))),
        })
    }
}

// ── WritePptTool ────────────────────────────────────────────────────────────

#[cfg(feature = "document-ppt")]
pub struct WritePptTool;

#[cfg(feature = "document-ppt")]
impl Tool for WritePptTool {
    fn name(&self) -> &'static str {
        "write_ppt"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' in payload"))?;

        let validated = sanitize_path(input, path)?;

        let slides = input.payload["slides"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing or invalid 'slides' array in payload"))?;

        let slide_count = slides.len();
        let bytes = build_pptx(slides).map_err(|e| anyhow::anyhow!("PPTX build error: {e}"))?;

        fs::write(&validated, &bytes).context("failed to write PPTX file")?;

        info!(
            path = %validated.display(),
            slides = slide_count,
            bytes = bytes.len(),
            "tool: PPTX file written successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated.to_string_lossy(),
                "slides": slide_count,
                "size_bytes": bytes.len(),
            })),
            error: None,
            verification: Some("ppt_write".to_string()),
            audit_log: Some(format!(
                "Wrote PPTX file: {} ({} slides, {} bytes)",
                validated.display(),
                slide_count,
                bytes.len(),
            )),
            pua_report: Some(tool_execution_report("write_ppt", Some("ppt_write"))),
        })
    }
}

#[cfg(feature = "document-ppt")]
fn build_pptx(slides: &[serde_json::Value]) -> Result<Vec<u8>> {
    use std::io::Write;
    use zip::write::FileOptions;

    let mut buf = Vec::new();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));

    // ── [Content_Types].xml ───────────────────────────────────────────────
    zip.start_file("[Content_Types].xml", FileOptions::<'_, ()>::default())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>
  <Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>
  <Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
  <Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
  <Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
"#
    )?;
    for i in 1..=slides.len() {
        writeln!(
            zip,
            "  <Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>"
        )?;
    }
    write!(zip, "</Types>")?;

    // ── _rels/.rels ───────────────────────────────────────────────────────
    zip.start_file("_rels/.rels", FileOptions::<'_, ()>::default())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#
    )?;

    // ── ppt/presentation.xml ──────────────────────────────────────────────
    zip.start_file("ppt/presentation.xml", FileOptions::<'_, ()>::default())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldMasterIdLst>
    <p:sldMasterId id="2147483648" r:id="rId1"/>
  </p:sldMasterIdLst>
  <p:sldIdLst>
"#
    )?;
    for i in 0..slides.len() {
        writeln!(
            zip,
            "    <p:sldId id=\"{}\" r:id=\"rId{}\"/>",
            i + 256,
            i + 2
        )?;
    }
    write!(
        zip,
        r#"  </p:sldIdLst>
  <p:sldSz cx="9144000" cy="6858000"/>
  <p:notesSz cx="6858000" cy="9144000"/>
</p:presentation>"#
    )?;

    // ── ppt/_rels/presentation.xml.rels ───────────────────────────────────
    zip.start_file(
        "ppt/_rels/presentation.xml.rels",
        FileOptions::<'_, ()>::default(),
    )?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>
"#
    )?;
    for i in 0..slides.len() {
        writeln!(
            zip,
            "  <Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{}.xml\"/>",
            i + 2,
            i + 1
        )?;
    }
    write!(zip, "</Relationships>")?;

    // ── ppt/slideMasters/slideMaster1.xml ─────────────────────────────────
    zip.start_file(
        "ppt/slideMasters/slideMaster1.xml",
        FileOptions::<'_, ()>::default(),
    )?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:name>DefaultDesign</p:name>
  <p:sldLayoutIdLst>
    <p:sldLayoutId id="2147483649" r:id="rId1"/>
  </p:sldLayoutIdLst>
</p:sldMaster>"#
    )?;

    // ── ppt/slideLayouts/slideLayout1.xml ─────────────────────────────────
    zip.start_file(
        "ppt/slideLayouts/slideLayout1.xml",
        FileOptions::<'_, ()>::default(),
    )?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank">
  <p:name>Blank</p:name>
</p:sldLayout>"#
    )?;

    // ── ppt/theme/theme1.xml ──────────────────────────────────────────────
    zip.start_file("ppt/theme/theme1.xml", FileOptions::<'_, ()>::default())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="DefaultTheme">
  <a:themeElements>
    <a:clrScheme name="Default">
      <a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
      <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
      <a:dk2><a:srgbClr val="44546A"/></a:dk2>
      <a:lt2><a:srgbClr val="E7E6E6"/></a:lt2>
      <a:accent1><a:srgbClr val="4472C4"/></a:accent1>
      <a:accent2><a:srgbClr val="ED7D31"/></a:accent2>
      <a:accent3><a:srgbClr val="A5A5A5"/></a:accent3>
      <a:accent4><a:srgbClr val="FFC000"/></a:accent4>
      <a:accent5><a:srgbClr val="5B9BD5"/></a:accent5>
      <a:accent6><a:srgbClr val="70AD47"/></a:accent6>
      <a:hlink><a:srgbClr val="0563C1"/></a:hlink>
      <a:folHlink><a:srgbClr val="954F72"/></a:folHlink>
    </a:clrScheme>
    <a:fontScheme name="Default">
      <a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>
      <a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>
    </a:fontScheme>
    <a:fmtScheme name="Default"/>
  </a:themeElements>
</a:theme>"#
    )?;

    // ── ppt/slides/ ───────────────────────────────────────────────────────
    for (i, slide) in slides.iter().enumerate() {
        let title = slide["title"].as_str().unwrap_or("Slide");
        let body = slide["body"].as_str().unwrap_or("");

        zip.start_file(
            format!("ppt/slides/slide{}.xml", i + 1),
            FileOptions::<'_, ()>::default(),
        )?;
        write!(
            zip,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:spTree>
    <p:nvGrpSpPr>
      <p:cNvPr id="1" name="Slide"/>
      <p:cNvGrpSpPr/>
      <p:nvPr/>
    </p:nvGrpSpPr>
    <p:grpSpPr/>
    <p:sp>
      <p:nvSpPr>
        <p:cNvPr id="2" name="Title"/>
        <p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>
        <p:nvPr>
          <p:ph type="title"/>
        </p:nvPr>
      </p:nvSpPr>
      <p:spPr/>
      <p:txBody>
        <a:bodyPr/>
        <a:lstStyle/>
        <a:p>
          <a:r>
            <a:rPr sz="4400" b="1"/>
            <a:t>{title}</a:t>
          </a:r>
        </a:p>
      </p:txBody>
    </p:sp>
    <p:sp>
      <p:nvSpPr>
        <p:cNvPr id="3" name="Body"/>
        <p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>
        <p:nvPr>
          <p:ph type="body"/>
        </p:nvPr>
      </p:nvSpPr>
      <p:spPr/>
      <p:txBody>
        <a:bodyPr/>
        <a:lstStyle/>
        <a:p>
          <a:r>
            <a:rPr sz="2800"/>
            <a:t>{body}</a:t>
          </a:r>
        </a:p>
      </p:txBody>
    </p:sp>
  </p:spTree>
</p:sld>"#,
            title = escape_xml(title),
            body = escape_xml(body),
        )?;
    }

    // ── docProps/app.xml ──────────────────────────────────────────────────
    zip.start_file("docProps/app.xml", FileOptions::<'_, ()>::default())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
  <Application>go-on</Application>
  <Slides>{}</Slides>
</Properties>"#,
        slides.len()
    )?;

    // ── docProps/core.xml ─────────────────────────────────────────────────
    zip.start_file("docProps/core.xml", FileOptions::<'_, ()>::default())?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <dc:creator>go-on</dc:creator>
  <dc:title>Presentation</dc:title>
  <cp:contentStatus>Draft</cp:contentStatus>
</cp:coreProperties>"#
    )?;

    zip.finish()?;
    Ok(buf)
}

/// Minimal XML escaping for text content in PPTX generation.
#[cfg(feature = "document-ppt")]
fn escape_xml(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&apos;".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

// ── WriteDocxTool ───────────────────────────────────────────────────────────

#[cfg(feature = "document-docx")]
pub struct WriteDocxTool;

#[cfg(feature = "document-docx")]
impl Tool for WriteDocxTool {
    fn name(&self) -> &'static str {
        "write_docx"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' in payload"))?;

        let validated = sanitize_path(input, path)?;

        let title = input.payload["title"].as_str().unwrap_or("Document");
        let paragraphs = input.payload["paragraphs"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing or invalid 'paragraphs' array in payload"))?;

        let (bytes, paragraph_count) = build_docx_bytes(title, paragraphs)?;

        fs::write(&validated, &bytes).context("failed to write DOCX file")?;

        info!(
            path = %validated.display(),
            paragraphs = paragraph_count,
            bytes = bytes.len(),
            "tool: DOCX file written successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated.to_string_lossy(),
                "paragraphs": paragraph_count,
                "size_bytes": bytes.len(),
            })),
            error: None,
            verification: Some("docx_write".to_string()),
            audit_log: Some(format!(
                "Wrote DOCX file: {} ({} paragraphs, {} bytes)",
                validated.display(),
                paragraph_count,
                bytes.len(),
            )),
            pua_report: Some(tool_execution_report("write_docx", Some("docx_write"))),
        })
    }
}

#[cfg(feature = "document-docx")]
fn build_docx_bytes(title: &str, paragraphs: &[serde_json::Value]) -> Result<(Vec<u8>, usize)> {
    use docx_rs::*;

    let mut doc = Docx::new();

    // Add title as a heading paragraph
    doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_text(title).bold().size(32)));

    // Add body paragraphs
    for p in paragraphs {
        let text = p.as_str().unwrap_or("");
        doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_text(text).size(22)));
    }

    let mut buf = Vec::new();
    doc.build().pack(std::io::Cursor::new(&mut buf))?;
    let bytes = buf;
    let paragraph_count = 1 + paragraphs.len(); // title + body

    Ok((bytes, paragraph_count))
}
