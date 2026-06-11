//! Filesystem tools (list_directory, file_move, file_delete)

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{sanitize_path, sanitize_path_for_write, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use std::fs;
use tracing::{info, warn};

// ── ListDirectoryTool ──────────────────────────────────────────────────────

pub struct ListDirectoryTool;

impl Tool for ListDirectoryTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated = sanitize_path(input, path)?;
        let mut entries: Vec<serde_json::Value> = Vec::new();

        for entry in fs::read_dir(&validated).context("failed to read directory")? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type().ok();
            let metadata = entry.metadata().ok();

            let entry_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let is_dir = file_type.as_ref().map(|ft| ft.is_dir()).unwrap_or(false);

            let size = metadata
                .as_ref()
                .and_then(|m| if m.is_file() { Some(m.len()) } else { None });

            entries.push(serde_json::json!({
                "name": entry_name,
                "is_directory": is_dir,
                "size_bytes": size,
            }));
        }

        // Sort: directories first, then alphabetical
        entries.sort_by(|a, b| {
            let a_dir = a["is_directory"].as_bool().unwrap_or(false);
            let b_dir = b["is_directory"].as_bool().unwrap_or(false);
            b_dir
                .cmp(&a_dir)
                .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
        });

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "entries": entries,
                "count": entries.len(),
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: Some("directory_listed".to_string()),
            audit_log: Some(format!(
                "Listed directory: {} ({} entries)",
                validated.display(),
                entries.len()
            )),
            pua_report: Some(tool_execution_report(
                "list_directory",
                Some("directory_listed"),
            )),
        })
    }
}

// ── FileMoveTool ───────────────────────────────────────────────────────────

pub struct FileMoveTool;

impl Tool for FileMoveTool {
    fn name(&self) -> &'static str {
        "file_move"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let source = input.payload["source"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_source_path")))?;
        let destination = input.payload["destination"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_destination_path")))?;

        let source_path = sanitize_path(input, source)?;
        let dest_path = sanitize_path_for_write(input, destination)?;

        if !source_path.exists() {
            anyhow::bail!("{}", tf("error.source_not_found", &[("source", source)]));
        }

        // Create parent directories if they don't exist
        if let Some(parent) = dest_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .context("failed to create destination parent directories")?;
            }
        }

        fs::rename(&source_path, &dest_path).context("failed to move file")?;

        info!(
            source = %source_path.display(),
            dest = %dest_path.display(),
            "tool: file moved successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "source": source_path.to_string_lossy(),
                "destination": dest_path.to_string_lossy(),
            })),
            error: None,
            verification: Some("file_moved".to_string()),
            audit_log: Some(format!(
                "Moved '{}' -> '{}'",
                source_path.display(),
                dest_path.display()
            )),
            pua_report: Some(tool_execution_report("file_move", Some("file_moved"))),
        })
    }
}

// ── FileDeleteTool ─────────────────────────────────────────────────────────

pub struct FileDeleteTool;

impl Tool for FileDeleteTool {
    fn name(&self) -> &'static str {
        "file_delete"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let confirm = input.payload["confirm"].as_bool().unwrap_or(false);

        if !confirm {
            anyhow::bail!("{}", t("error.delete_not_confirmed"));
        }

        let validated = sanitize_path(input, path)?;

        if !validated.exists() {
            anyhow::bail!("{}", tf("error.path_not_found", &[("path", path)]));
        }

        let is_dir = validated.is_dir();

        if is_dir {
            fs::remove_dir_all(&validated).context("failed to delete directory")?;
        } else {
            fs::remove_file(&validated).context("failed to delete file")?;
        }

        warn!(
            path = %validated.display(),
            is_dir = %is_dir,
            "tool: file/directory deleted"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "deleted_path": validated.to_string_lossy(),
                "is_directory": is_dir,
            })),
            error: None,
            verification: Some("file_deleted".to_string()),
            audit_log: Some(format!(
                "Deleted {} '{}'",
                if is_dir { "directory" } else { "file" },
                validated.display()
            )),
            pua_report: Some(tool_execution_report("file_delete", Some("file_deleted"))),
        })
    }
}
