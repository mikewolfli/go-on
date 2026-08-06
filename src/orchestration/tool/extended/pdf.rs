//! PDF document tools
//!
//! Provides `ReadPdfTool` for extracting text from PDF files using lopdf.
//! Only compiled when `feature = "document-pdf"` is enabled.

#[cfg(feature = "document-pdf")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "document-pdf")]
use crate::orchestration::tool::{
    sanitize_path, sanitize_path_for_write, Tool, ToolInput, ToolOutput,
};
#[cfg(feature = "document-pdf")]
use anyhow::{Context, Result};
#[cfg(feature = "document-pdf")]
use std::fs;
#[cfg(feature = "document-pdf")]
use tracing::info;

#[cfg(feature = "document-pdf")]
pub struct ReadPdfTool;

/// Alias for backward compatibility with builder.rs.
#[cfg(feature = "document-pdf")]
pub type PdfReadTool = ReadPdfTool;

#[cfg(feature = "document-pdf")]
impl Tool for ReadPdfTool {
    fn name(&self) -> &'static str {
        "read_pdf"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content = fs::read(&validated)
            .with_context(|| format!("failed to read PDF: {}", validated.display()))?;

        // Single PDF extraction implementation (shared with the multimodal
        // pipeline — the previous inline lopdf copy was removed).
        let parser = crate::multimodal::document_parser::DocumentParser::default();
        let parsed = parser
            .parse_bytes(&content, "pdf")
            .map_err(|e| anyhow::anyhow!("PDF parse error: {}", e))?;

        let page_count = parsed
            .metadata
            .get("page_count")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let byte_size = content.len();

        info!(
            path = %validated.to_string_lossy(),
            pages = page_count,
            images = parsed.images.len(),
            "PDF text extracted"
        );

        let report = tool_execution_report("read_pdf", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "text": parsed.text_content,
                "page_count": page_count,
                "byte_size": byte_size,
                "images": parsed.images,
                "metadata": parsed.metadata,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "read_pdf: {} pages from {}",
                page_count,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

// ── PdfMergeTool ────────────────────────────────────────────────────────────

#[cfg(feature = "document-pdf")]
pub struct PdfMergeTool;

#[cfg(feature = "document-pdf")]
impl Tool for PdfMergeTool {
    fn name(&self) -> &'static str {
        "pdf_merge"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let paths = input.payload["paths"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing 'paths' array"))?;

        if paths.len() < 2 {
            anyhow::bail!("'paths' must contain at least 2 PDF files to merge");
        }

        let output_path = input.payload["output_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'output_path'"))?;

        let validated_output = sanitize_path_for_write(input, output_path)?;

        if let Some(parent) = validated_output.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).context("failed to create output parent directories")?;
            }
        }

        let validated_paths: Vec<std::path::PathBuf> = paths
            .iter()
            .filter_map(|v| v.as_str())
            .map(|p| sanitize_path(input, p))
            .collect::<Result<Vec<_>>>()?;

        let mut documents: Vec<lopdf::Document> = Vec::new();
        let mut pages_per_source: Vec<usize> = Vec::new();
        for p in &validated_paths {
            let doc = load_pdf_document(p)?;
            pages_per_source.push(doc.get_pages().len());
            documents.push(doc);
        }

        // Build merged document by copying all objects into a new document
        // and constructing a combined page tree
        let mut merged = merge_pdf_documents(&documents)?;

        save_pdf_document(&mut merged, &validated_output)?;

        let total_pages = merged.get_pages().len();

        info!(
            output = validated_output.to_string_lossy().as_ref(),
            source_count = validated_paths.len(),
            total_pages = total_pages,
            "PDF merge complete"
        );

        let report = tool_execution_report("pdf_merge", Some("pdf_merged"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "output_path": validated_output.to_string_lossy(),
                "source_count": validated_paths.len(),
                "total_pages": total_pages,
                "pages_per_source": pages_per_source,
            })),
            error: None,
            verification: Some("pdf_merged".to_string()),
            audit_log: Some(format!(
                "Merged {} PDFs into '{}' ({} total pages)",
                validated_paths.len(),
                validated_output.display(),
                total_pages,
            )),
            pua_report: Some(report),
        })
    }
}

/// Merge multiple PDF documents into one by copying all objects and building
/// a combined page tree.
#[cfg(feature = "document-pdf")]
fn merge_pdf_documents(docs: &[lopdf::Document]) -> Result<lopdf::Document> {
    use std::collections::HashMap;

    let mut merged = lopdf::Document::with_version("1.5");
    let pages_root_id = merged.new_object_id();
    let mut all_page_refs: Vec<lopdf::Object> = Vec::new();

    for doc in docs {
        // Map old ObjectId -> new ObjectId
        let mut id_map: HashMap<lopdf::ObjectId, lopdf::ObjectId> = HashMap::new();
        for (old_id, obj) in &doc.objects {
            let new_id = merged.add_object(obj.clone());
            id_map.insert(*old_id, new_id);
        }

        // For each page in the source doc, add to the merged page tree
        for (_, page_id) in doc.get_pages() {
            if let Some(&new_page_id) = id_map.get(&page_id) {
                // Update the Parent reference in the page dictionary
                if let Ok(page_dict) = merged.get_dictionary_mut(new_page_id) {
                    page_dict.set("Parent", lopdf::Object::Reference(pages_root_id));
                }
                all_page_refs.push(lopdf::Object::Reference(new_page_id));
            }
        }
    }

    // Build the combined Pages tree node
    let pages_obj: lopdf::Object = lopdf::Dictionary::from_iter(vec![
        (b"Type".to_vec(), lopdf::Object::Name(b"Pages".to_vec())),
        (
            b"Kids".to_vec(),
            lopdf::Object::Array(all_page_refs.clone()),
        ),
        (
            b"Count".to_vec(),
            lopdf::Object::Integer(all_page_refs.len() as i64),
        ),
        (
            b"MediaBox".to_vec(),
            lopdf::Object::Array(vec![
                lopdf::Object::Integer(0),
                lopdf::Object::Integer(0),
                lopdf::Object::Integer(612),
                lopdf::Object::Integer(792),
            ]),
        ),
    ])
    .into();
    merged.objects.insert(pages_root_id, pages_obj);

    // Build the Catalog
    let catalog_obj: lopdf::Object = lopdf::Dictionary::from_iter(vec![
        (b"Type".to_vec(), lopdf::Object::Name(b"Catalog".to_vec())),
        (b"Pages".to_vec(), lopdf::Object::Reference(pages_root_id)),
    ])
    .into();
    let catalog_id = merged.add_object(catalog_obj);
    merged.trailer.set("Root", catalog_id);

    Ok(merged)
}

/// Load a PDF document from disk (shared by merge/split — single read+parse).
#[cfg(feature = "document-pdf")]
fn load_pdf_document(path: &std::path::Path) -> Result<lopdf::Document> {
    let content =
        fs::read(path).with_context(|| format!("failed to read PDF: {}", path.display()))?;
    lopdf::Document::load_mem(&content)
        .with_context(|| format!("failed to parse PDF: {}", path.display()))
}

/// Save a PDF document to disk (shared by merge/split).
#[cfg(feature = "document-pdf")]
fn save_pdf_document(doc: &mut lopdf::Document, path: &std::path::Path) -> Result<()> {
    doc.save(path)
        .map(|_| ())
        .with_context(|| format!("failed to save PDF: {}", path.display()))
}

// ── PdfSplitTool ────────────────────────────────────────────────────────────

#[cfg(feature = "document-pdf")]
pub struct PdfSplitTool;

#[cfg(feature = "document-pdf")]
impl Tool for PdfSplitTool {
    fn name(&self) -> &'static str {
        "pdf_split"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let output_path = input.payload["output_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'output_path'"))?;
        let start_page = input.payload["start_page"].as_u64().unwrap_or(1) as u32;
        let end_page = input.payload["end_page"].as_u64();

        let validated = sanitize_path(input, path)?;
        let validated_output = sanitize_path_for_write(input, output_path)?;

        if let Some(parent) = validated_output.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).context("failed to create output parent directories")?;
            }
        }

        // Single read+parse: page count is derived from the same document that
        // gets the page deletions (previously the file was parsed twice).
        let mut doc = load_pdf_document(&validated)?;

        let total_pages = doc.get_pages().len() as u32;

        if start_page < 1 || start_page > total_pages {
            anyhow::bail!(
                "start_page must be between 1 and {}, got {}",
                total_pages,
                start_page
            );
        }

        let end = end_page
            .unwrap_or(total_pages as u64)
            .min(total_pages as u64) as u32;

        if end < start_page {
            anyhow::bail!("end_page ({}) must be >= start_page ({})", end, start_page);
        }

        // Build pages to delete
        let mut pages_to_delete: Vec<u32> = Vec::new();
        // Delete pages AFTER end
        for p in (end + 1)..=total_pages {
            pages_to_delete.push(p);
        }
        // Delete pages BEFORE start
        for p in 1..start_page {
            pages_to_delete.push(p);
        }
        pages_to_delete.sort();

        doc.delete_pages(&pages_to_delete);

        save_pdf_document(&mut doc, &validated_output)?;

        let output_page_count = (end - start_page + 1) as usize;

        info!(
            source = validated.to_string_lossy().as_ref(),
            output = validated_output.to_string_lossy().as_ref(),
            pages = output_page_count,
            range = format!("{}..{}", start_page, end),
            "PDF split complete"
        );

        let report = tool_execution_report("pdf_split", Some("pdf_split"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "output_path": validated_output.to_string_lossy(),
                "source_path": validated.to_string_lossy(),
                "source_total_pages": total_pages,
                "page_range": format!("{}..{}", start_page, end),
                "page_count": output_page_count,
            })),
            error: None,
            verification: Some("pdf_split".to_string()),
            audit_log: Some(format!(
                "Split PDF '{}' pages {}..{} -> '{}' ({} pages)",
                validated.display(),
                start_page,
                end,
                validated_output.display(),
                output_page_count,
            )),
            pua_report: Some(report),
        })
    }
}
