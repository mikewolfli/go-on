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
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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

    let mut entries = Vec::with_capacity(archive.len().min(MAX_ARCHIVE_ENTRIES));

    for i in 0..archive.len() {
        if entries.len() >= MAX_ARCHIVE_ENTRIES {
            anyhow::bail!("archive_inspect: archive has more than {MAX_ARCHIVE_ENTRIES} entries");
        }
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
        if entries.len() >= MAX_ARCHIVE_ENTRIES {
            anyhow::bail!("archive_inspect: archive has more than {MAX_ARCHIVE_ENTRIES} entries");
        }
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

/// Max entries an `archive_extract` call will process (zip/tar): a hostile
/// archive with millions of tiny entries would otherwise grind the extractor.
const MAX_ARCHIVE_ENTRIES: usize = 10_000;

/// Max total decompressed bytes an `archive_extract` call will write (zip/tar):
/// disk-fill protection, aligned with the shared 1 GiB input guard
/// (`exec_common::MAX_TOOL_FILE_READ_BYTES` — single source for the value).
const MAX_ARCHIVE_EXTRACT_BYTES: u64 =
    crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES as u64;

fn extract_zip(path: &Path, output_dir: &Path, filter: Option<&str>) -> Result<usize> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open zip file: {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("failed to read zip archive: {}", path.display()))?;

    let mut extracted_count = 0usize;
    let mut total_bytes: u64 = 0;

    for i in 0..archive.len() {
        if i >= MAX_ARCHIVE_ENTRIES {
            anyhow::bail!("archive_extract: archive has more than {MAX_ARCHIVE_ENTRIES} entries");
        }
        let entry = archive
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

        // Zip-slip guard: refuse entries that would escape `output_dir`.
        // `PathBuf::join` with an absolute entry replaces the base entirely
        // (e.g. `/etc/passwd`), and `..` components climb out of it; a
        // backslash separator is the Windows-style variant of the same
        // attack. Fail closed on the whole archive rather than extracting
        // around a malicious entry.
        if entry_path.split('/').any(|seg| seg == "..")
            || entry_path.contains('\\')
            || Path::new(&entry_path).is_absolute()
        {
            anyhow::bail!(
                "archive_extract: refusing to extract entry '{entry_path}' (path traversal)"
            );
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
            // Disk-fill guard: cap both the per-entry read and the total
            // extracted size (`take` stops reading once the cap is reached).
            let remaining = MAX_ARCHIVE_EXTRACT_BYTES.saturating_sub(total_bytes);
            let mut outfile = fs::File::create(&target)
                .with_context(|| format!("failed to create file: {}", target.display()))?;
            let copied = std::io::copy(&mut entry.take(remaining + 1), &mut outfile)
                .with_context(|| format!("failed to extract '{entry_path}'"))?;
            total_bytes += copied;
            if copied > remaining {
                anyhow::bail!(
                    "archive_extract: extraction exceeds the {} byte limit",
                    MAX_ARCHIVE_EXTRACT_BYTES
                );
            }
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
    let mut total_bytes: u64 = 0;

    // Count every processed entry (including dirs and filter-skipped ones),
    // so a hostile archive of a million directory entries cannot grind the
    // extractor — the zip path counts by archive index.
    for (processed_entries, entry_result) in archive
        .entries()
        .with_context(|| format!("failed to read tar entries from: {}", path.display()))?
        .enumerate()
    {
        if processed_entries >= MAX_ARCHIVE_ENTRIES {
            anyhow::bail!("archive_extract: archive has more than {MAX_ARCHIVE_ENTRIES} entries");
        }
        let entry = entry_result
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

        // Path-traversal guard (same as the zip path): `tar::Entry::unpack`
        // writes exactly to the given path WITHOUT validating it (only
        // `unpack_in` does), so a hostile archive with `..`/absolute/
        // backslash entries would escape `output_dir`.
        if entry_path.split('/').any(|seg| seg == "..")
            || entry_path.contains('\\')
            || Path::new(&entry_path).is_absolute()
        {
            anyhow::bail!(
                "archive_extract: refusing to extract entry '{entry_path}' (path traversal)"
            );
        }

        let target = output_dir.join(&entry_path);

        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create directory: {}", target.display()))?;
        } else if entry.header().entry_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directory: {}", parent.display())
                })?;
            }
            // Disk-fill guard: cap the per-entry read and the total
            // extracted size (`take` stops reading once the cap is reached);
            // the mode from the tar header is preserved to match `unpack`
            // semantics for regular files.
            let remaining = MAX_ARCHIVE_EXTRACT_BYTES.saturating_sub(total_bytes);
            let mode = entry.header().mode().ok();
            let mut outfile = fs::File::create(&target)
                .with_context(|| format!("failed to create file: {}", target.display()))?;
            let copied = std::io::copy(&mut entry.take(remaining + 1), &mut outfile)
                .with_context(|| format!("failed to extract '{entry_path}'"))?;
            drop(outfile);
            total_bytes += copied;
            if copied > remaining {
                anyhow::bail!(
                    "archive_extract: extraction exceeds the {} byte limit",
                    MAX_ARCHIVE_EXTRACT_BYTES
                );
            }
            if let Some(mode) = mode {
                if let Err(e) = fs::set_permissions(&target, fs::Permissions::from_mode(mode)) {
                    debug!(
                        "archive_extract: failed to set permissions on {}: {}",
                        target.display(),
                        e
                    );
                }
            }
            extracted_count += 1;
        } else {
            // Symlinks / hardlinks / special entries: rejected outright.
            // `tar::Entry::unpack` does not validate a link's target, so a
            // hostile archive could plant a link pointing outside
            // `output_dir` and then write THROUGH it with a later entry
            // (e.g. symlink `ln` → `/etc` + regular file `ln/evil`).
            anyhow::bail!(
                "archive_extract: refusing entry '{entry_path}' (symlink/hardlink/special file not supported)"
            );
        }
    }

    Ok(extracted_count)
}

// ── Plain gzip extraction ───────────────────────────────────────────────────

fn extract_gzip_single(path: &Path, output_dir: &Path, _filter: Option<&str>) -> Result<usize> {
    // Input-side cap (aligned with the decompression output cap): a huge
    // .gz input must not be fully buffered.
    let input_data = crate::orchestration::tool::exec_common::read_file_capped(
        path,
        super::compress::MAX_DECOMPRESSED_TOOL_BYTES,
    )
    .with_context(|| format!("failed to read gzip file: {}", path.display()))?;

    // Shared gzip decompression (single implementation with the `decompress` tool).
    let decompressed = super::compress::decompress_gzip_bytes(&input_data)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn extract_zip_rejects_path_traversal_entries() {
        // Regression: zip-slip — zip entry names carrying `..` components,
        // absolute paths, or Windows backslash separators must be refused
        // instead of escaping `output_dir` (PathBuf::join replaces the base
        // for absolute entries).
        let tmp = TempDir::new().unwrap();
        let out_dir = tmp.path().join("out");

        for evil_name in [
            "../evil.txt",
            "/tmp/evil.txt",
            "a/../../evil.txt",
            "..\\evil.txt",
        ] {
            let zip_path = tmp.path().join(format!(
                "evil-{}.zip",
                evil_name.replace(['/', '\\', '.'], "_")
            ));
            {
                let file = fs::File::create(&zip_path).unwrap();
                let mut writer = zip::ZipWriter::new(file);
                let options = zip::write::SimpleFileOptions::default();
                writer.start_file(evil_name, options).unwrap();
                writer.write_all(b"pwned").unwrap();
                writer.finish().unwrap();
            }
            let err = extract_zip(&zip_path, &out_dir, None).unwrap_err();
            assert!(
                err.to_string().contains("path traversal"),
                "entry {evil_name:?} should be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn extract_tar_rejects_path_traversal_entries() {
        // Regression (tar-slip): `tar::Entry::unpack` writes exactly to the
        // given path without validating it, so traversal entries must be
        // rejected before unpacking. The tar crate's own Builder refuses to
        // WRITE such names, but a hostile archive produced by other tools
        // must still be rejected by the extractor.
        let tmp = TempDir::new().unwrap();
        let out_dir = tmp.path().join("out");
        let tar_path = tmp.path().join("evil.tar");

        // Manually craft a minimal ustar archive containing "../evil.txt"
        // (512-byte header + 5-byte payload + two zero end blocks).
        let mut header = [0u8; 512];
        let name = b"../evil.txt";
        header[..name.len()].copy_from_slice(name);
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(b"00000000005\0"); // size 5 (octal)
        header[136..148].copy_from_slice(b"00000000000\0"); // mtime
        header[156] = b'0'; // typeflag: regular file
        header[257..263].copy_from_slice(b"ustar\0"); // magic
                                                      // ustar checksum: sum of all bytes with the checksum field as spaces.
        let sum: u32 = header
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if (148..156).contains(&i) {
                    b' ' as u32
                } else {
                    *b as u32
                }
            })
            .sum();
        let chksum = format!("{:06o}\0 ", sum);
        header[148..156].copy_from_slice(chksum.as_bytes());
        let mut tar_bytes = Vec::new();
        tar_bytes.extend_from_slice(&header);
        tar_bytes.extend_from_slice(b"pwned");
        tar_bytes.extend_from_slice(&[0u8; 1024]);
        fs::write(&tar_path, &tar_bytes).unwrap();

        let err = extract_tar(&tar_path, &out_dir, None).unwrap_err();
        assert!(
            err.to_string().contains("path traversal"),
            "tar entry should be rejected, got: {err}"
        );
    }
}
