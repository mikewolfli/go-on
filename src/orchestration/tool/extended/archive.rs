//! Archive inspection and extraction tools
//!
//! Provides `ArchiveInspectTool` for listing archive contents (zip, tar.gz)
//! and `ArchiveExtractTool` for extracting specific files from archives.
//! Uses the `flate2`, `tar`, and `zip` crates (no feature gate — always compiled).

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::t;
use crate::orchestration::tool::{
    sanitize_path, sanitize_path_for_write, Tool, ToolInput, ToolOutput,
};
use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use tracing::{debug, info};

// ── ArchiveInspectTool ──────────────────────────────────────────────────────

pub struct ArchiveInspectTool;

impl Tool for ArchiveInspectTool {
    fn name(&self) -> &'static str {
        "archive_inspect"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated = sanitize_path(input, path)?;

        if !validated.exists() {
            anyhow::bail!("file not found: {}", validated.display());
        }

        let file_name = validated
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        debug!(path = %validated.display(), "archive_inspect: inspecting archive");

        let entries = if file_name.ends_with(".zip") {
            inspect_zip(&validated)?
        } else if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
            inspect_tar_gz(&validated)?
        } else if file_name.ends_with(".tar") {
            inspect_tar(&validated)?
        } else if file_name.ends_with(".gz") && !file_name.ends_with(".tar.gz") {
            // Plain gzip — report as a single entry
            vec![ArchiveEntry {
                path: validated
                    .with_extension("")
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "decompressed".to_string()),
                size: validated.metadata().ok().map(|m| m.len()).unwrap_or(0),
                is_dir: false,
                compressed_size: Some(validated.metadata().ok().map(|m| m.len()).unwrap_or(0)),
            }]
        } else {
            anyhow::bail!(
                "unsupported archive format: {} (supported: .zip, .tar.gz, .tgz, .tar, .gz)",
                file_name
            );
        };

        let total_files = entries.iter().filter(|e| !e.is_dir).count();
        let total_dirs = entries.iter().filter(|e| e.is_dir).count();
        let total_size: u64 = entries.iter().map(|e| e.size).sum();
        let file_size = validated.metadata().ok().map(|m| m.len()).unwrap_or(0);

        info!(
            path = %validated.display(),
            total_entries = entries.len(),
            files = total_files,
            dirs = total_dirs,
            "archive_inspect: inspection complete"
        );

        let entries_json: Vec<serde_json::Value> = entries
            .into_iter()
            .map(|e| {
                serde_json::json!({
                    "path": e.path,
                    "size": e.size,
                    "is_dir": e.is_dir,
                    "compressed_size": e.compressed_size,
                })
            })
            .collect();

        let report = tool_execution_report("archive_inspect", Some("archive_inspected"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated.to_string_lossy(),
                "format": infer_format(&file_name),
                "file_size_bytes": file_size,
                "total_entries": total_files + total_dirs,
                "total_files": total_files,
                "total_directories": total_dirs,
                "total_uncompressed_bytes": total_size,
                "entries": entries_json,
            })),
            error: None,
            verification: Some("archive_inspected".to_string()),
            audit_log: Some(format!(
                "Inspected archive '{}': {} files, {} dirs",
                validated.display(),
                total_files,
                total_dirs,
            )),
            pua_report: Some(report),
        })
    }
}

// ── ArchiveExtractTool ──────────────────────────────────────────────────────

pub struct ArchiveExtractTool;

impl Tool for ArchiveExtractTool {
    fn name(&self) -> &'static str {
        "archive_extract"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let output_dir = input.payload["output_dir"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'output_dir'"))?;
        let filter_pattern = input.payload["filter"].as_str(); // optional glob filter

        let validated = sanitize_path(input, path)?;

        if !validated.exists() {
            anyhow::bail!("file not found: {}", validated.display());
        }

        let validated_output = sanitize_path_for_write(input, output_dir)?;

        // Create output directory
        fs::create_dir_all(&validated_output).with_context(|| {
            format!(
                "failed to create output directory: {}",
                validated_output.display()
            )
        })?;

        let file_name = validated
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        debug!(
            path = %validated.display(),
            output = %validated_output.display(),
            filter = filter_pattern.unwrap_or("*"),
            "archive_extract: extracting archive"
        );

        let extracted = if file_name.ends_with(".zip") {
            extract_zip(&validated, &validated_output, filter_pattern)?
        } else if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
            extract_tar_gz(&validated, &validated_output, filter_pattern)?
        } else if file_name.ends_with(".tar") {
            extract_tar(&validated, &validated_output, filter_pattern)?
        } else if file_name.ends_with(".gz") && !file_name.ends_with(".tar.gz") {
            extract_gzip_single(&validated, &validated_output, filter_pattern)?
        } else {
            anyhow::bail!(
                "unsupported archive format: {} (supported: .zip, .tar.gz, .tgz, .tar, .gz)",
                file_name
            );
        };

        info!(
            path = %validated.display(),
            output = %validated_output.display(),
            extracted = extracted,
            "archive_extract: extraction complete"
        );

        let report = tool_execution_report("archive_extract", Some("archive_extracted"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "archive_path": validated.to_string_lossy(),
                "output_dir": validated_output.to_string_lossy(),
                "extracted_count": extracted,
            })),
            error: None,
            verification: Some("archive_extracted".to_string()),
            audit_log: Some(format!(
                "Extracted {} files from '{}' to '{}'",
                extracted,
                validated.display(),
                validated_output.display(),
            )),
            pua_report: Some(report),
        })
    }
}

// ── Internal types ──────────────────────────────────────────────────────────

struct ArchiveEntry {
    path: String,
    size: u64,
    is_dir: bool,
    compressed_size: Option<u64>,
}

// ── Format helpers ──────────────────────────────────────────────────────────

fn infer_format(file_name: &str) -> &'static str {
    if file_name.ends_with(".zip") {
        "zip"
    } else if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
        "tar.gz"
    } else if file_name.ends_with(".tar") {
        "tar"
    } else if file_name.ends_with(".gz") {
        "gzip"
    } else {
        "unknown"
    }
}

// ── Zip inspection ──────────────────────────────────────────────────────────

fn inspect_zip(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open zip file: {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("failed to read zip archive: {}", path.display()))?;

    let mut entries = Vec::with_capacity(archive.len());

    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .with_context(|| format!("failed to read zip entry {i}"))?;
        let is_dir = entry.is_dir();
        let entry_path = entry.name().trim_end_matches('/').to_string();
        let size = entry.size();
        let compressed_size = entry.compressed_size();

        entries.push(ArchiveEntry {
            path: entry_path,
            size,
            is_dir,
            compressed_size: Some(compressed_size),
        });
    }

    Ok(entries)
}

// ── Tar.gz / Tar inspection ─────────────────────────────────────────────────

fn inspect_tar_gz(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open file: {}", path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    inspect_tar_impl(decoder, path)
}

fn inspect_tar(path: &Path) -> Result<Vec<ArchiveEntry>> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open file: {}", path.display()))?;
    inspect_tar_impl(file, path)
}

fn inspect_tar_impl<R: Read>(reader: R, path: &Path) -> Result<Vec<ArchiveEntry>> {
    let mut archive = tar::Archive::new(reader);
    let mut entries = Vec::new();

    for entry_result in archive
        .entries()
        .with_context(|| format!("failed to read tar entries from: {}", path.display()))?
    {
        let entry = entry_result
            .with_context(|| format!("failed to read tar entry in: {}", path.display()))?;

        let entry_path = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let is_dir = entry.header().entry_type().is_dir();
        let size = entry.size();

        entries.push(ArchiveEntry {
            path: entry_path.trim_end_matches('/').to_string(),
            size,
            is_dir,
            compressed_size: None,
        });
    }

    Ok(entries)
}

// ── Zip extraction ──────────────────────────────────────────────────────────

fn extract_zip(path: &Path, output_dir: &Path, filter: Option<&str>) -> Result<usize> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open zip file: {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("failed to read zip archive: {}", path.display()))?;

    let mut extracted_count = 0usize;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("failed to read zip entry {i}"))?;

        let entry_path = entry.name().trim_end_matches('/').to_string();

        // Apply filter if specified
        if let Some(pattern) = filter {
            let glob = glob::Pattern::new(pattern)
                .map_err(|_| anyhow::anyhow!("invalid glob pattern: {pattern}"))?;
            if !glob.matches(&entry_path) {
                continue;
            }
        }

        let target = output_dir.join(&entry_path);

        if entry.is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create directory: {}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directory: {}", parent.display())
                })?;
            }
            let mut outfile = fs::File::create(&target)
                .with_context(|| format!("failed to create file: {}", target.display()))?;
            std::io::copy(&mut entry, &mut outfile)
                .with_context(|| format!("failed to extract '{}'", entry_path))?;
            extracted_count += 1;
        }
    }

    Ok(extracted_count)
}

// ── Tar.gz / Tar extraction ─────────────────────────────────────────────────

fn extract_tar_gz(path: &Path, output_dir: &Path, filter: Option<&str>) -> Result<usize> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open file: {}", path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    extract_tar_impl(decoder, output_dir, filter, path)
}

fn extract_tar(path: &Path, output_dir: &Path, filter: Option<&str>) -> Result<usize> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open file: {}", path.display()))?;
    extract_tar_impl(file, output_dir, filter, path)
}

fn extract_tar_impl<R: Read>(
    reader: R,
    output_dir: &Path,
    filter: Option<&str>,
    path: &Path,
) -> Result<usize> {
    let mut archive = tar::Archive::new(reader);
    let mut extracted_count = 0usize;

    for entry_result in archive
        .entries()
        .with_context(|| format!("failed to read tar entries from: {}", path.display()))?
    {
        let mut entry = entry_result
            .with_context(|| format!("failed to read tar entry in: {}", path.display()))?;

        let entry_path = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // Apply filter if specified
        if let Some(pattern) = filter {
            let glob = glob::Pattern::new(pattern)
                .map_err(|_| anyhow::anyhow!("invalid glob pattern: {pattern}"))?;
            if !glob.matches(&entry_path) {
                continue;
            }
        }

        let target = output_dir.join(&entry_path);

        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create directory: {}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directory: {}", parent.display())
                })?;
            }
            entry
                .unpack(&target)
                .with_context(|| format!("failed to extract '{}'", entry_path))?;
            extracted_count += 1;
        }
    }

    Ok(extracted_count)
}

// ── Plain gzip extraction ───────────────────────────────────────────────────

fn extract_gzip_single(path: &Path, output_dir: &Path, _filter: Option<&str>) -> Result<usize> {
    let input_data =
        fs::read(path).with_context(|| format!("failed to read gzip file: {}", path.display()))?;

    let mut decoder = flate2::read::GzDecoder::new(&input_data[..]);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .with_context(|| format!("failed to decompress gzip: {}", path.display()))?;

    // Output filename: strip .gz extension
    let out_name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "decompressed".to_string());

    let target = output_dir.join(&out_name);
    let mut outfile = fs::File::create(&target)
        .with_context(|| format!("failed to create output file: {}", target.display()))?;
    outfile
        .write_all(&decompressed)
        .with_context(|| format!("failed to write decompressed data to: {}", target.display()))?;

    Ok(1)
}
