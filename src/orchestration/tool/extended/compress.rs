//! Compression tools (gzip)
//!
//! Provides tools for compressing and decompressing data/files using gzip
//! via the `flate2` crate (full dependency, no feature gate).

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::t;
use crate::orchestration::tool::{
    sanitize_path, sanitize_path_for_write, Tool, ToolInput, ToolOutput,
};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use tracing::{debug, info};

// ── CompressTool ────────────────────────────────────────────────────────────

pub struct CompressTool;

impl Tool for CompressTool {
    fn name(&self) -> &'static str {
        "compress"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let output_path = input.payload["output_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_output_path")))?;
        let level = input.payload["level"].as_u64().unwrap_or(6); // default: level 6

        let validated_path = sanitize_path(input, path)?;

        if !validated_path.exists() {
            anyhow::bail!("input file not found: {}", validated_path.display());
        }

        let validated_output = sanitize_path_for_write(input, output_path)?;

        // Ensure parent directory exists
        if let Some(parent) = validated_output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .context("failed to create output parent directories")?;
            }
        }

        debug!(
            input = %validated_path.display(),
            output = %validated_output.display(),
            level = level,
            "tool: gzip compressing file"
        );

        let input_data = std::fs::read(&validated_path)
            .with_context(|| format!("failed to read input file: {}", validated_path.display()))?;

        let input_len = input_data.len();

        let compression_level = match level {
            0 => flate2::Compression::none(),
            1 => flate2::Compression::fast(),
            2..=8 => flate2::Compression::new(level as u32),
            _ => flate2::Compression::best(),
        };

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), compression_level);
        encoder
            .write_all(&input_data)
            .context("failed to write compressed data")?;
        let compressed_data = encoder
            .finish()
            .context("failed to finish gzip compression")?;

        std::fs::write(&validated_output, &compressed_data).with_context(|| {
            format!(
                "failed to write compressed output: {}",
                validated_output.display()
            )
        })?;

        let output_len = validated_output
            .metadata()
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);
        let ratio = if input_len > 0 {
            (output_len as f64 / input_len as f64) * 100.0
        } else {
            0.0
        };

        info!(
            input = %validated_path.display(),
            output = %validated_output.display(),
            input_size = input_len,
            output_size = output_len,
            ratio = format!("{:.1}%", ratio),
            level = level,
            "tool: file compressed successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "input_path": validated_path.to_string_lossy(),
                "output_path": validated_output.to_string_lossy(),
                "input_size_bytes": input_len,
                "output_size_bytes": output_len,
                "compression_ratio_pct": format!("{:.1}", ratio),
                "level": level,
            })),
            error: None,
            verification: Some("compressed".to_string()),
            audit_log: Some(format!(
                "Compressed '{}' ({} -> {} bytes, {:.1}%)",
                validated_path.display(),
                input_len,
                output_len,
                ratio
            )),
            pua_report: Some(tool_execution_report("compress", Some("compressed"))),
        })
    }
}

// ── DecompressTool ──────────────────────────────────────────────────────────

pub struct DecompressTool;

impl Tool for DecompressTool {
    fn name(&self) -> &'static str {
        "decompress"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let output_path = input.payload["output_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_output_path")))?;

        let validated_path = sanitize_path(input, path)?;

        if !validated_path.exists() {
            anyhow::bail!("input file not found: {}", validated_path.display());
        }

        let validated_output = sanitize_path_for_write(input, output_path)?;

        // Ensure parent directory exists
        if let Some(parent) = validated_output.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .context("failed to create output parent directories")?;
            }
        }

        debug!(
            input = %validated_path.display(),
            output = %validated_output.display(),
            "tool: gzip decompressing file"
        );

        let input_data = std::fs::read(&validated_path).with_context(|| {
            format!(
                "failed to read compressed file: {}",
                validated_path.display()
            )
        })?;

        let input_len = input_data.len();

        let mut decoder = flate2::read::GzDecoder::new(&input_data[..]);
        let mut decompressed_data = Vec::new();
        decoder
            .read_to_end(&mut decompressed_data)
            .context("failed to decompress gzip data")?;

        std::fs::write(&validated_output, &decompressed_data).with_context(|| {
            format!(
                "failed to write decompressed output: {}",
                validated_output.display()
            )
        })?;

        let output_len = validated_output
            .metadata()
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);

        info!(
            input = %validated_path.display(),
            output = %validated_output.display(),
            input_size = input_len,
            output_size = output_len,
            "tool: file decompressed successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "input_path": validated_path.to_string_lossy(),
                "output_path": validated_output.to_string_lossy(),
                "input_size_bytes": input_len,
                "output_size_bytes": output_len,
            })),
            error: None,
            verification: Some("decompressed".to_string()),
            audit_log: Some(format!(
                "Decompressed '{}' ({} -> {} bytes)",
                validated_path.display(),
                input_len,
                output_len
            )),
            pua_report: Some(tool_execution_report("decompress", Some("decompressed"))),
        })
    }
}
