//! Filesystem tools (list_directory, file_move, file_delete)

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{
    sanitize_path, sanitize_path_for_write, Tool, ToolInput, ToolOutput,
};
use anyhow::{Context, Result};
use std::fs;
use tracing::{info, warn};

// ── ListDirectoryTool ──────────────────────────────────────────────────────

pub struct ListDirectoryTool;

impl Tool for ListDirectoryTool {
    fn name(&self) -> &'static str {
        "list_directory"
    }
    fn description(&self) -> &str {
        "List files and directories in a given path"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated = sanitize_path(input, path)?;
        let mut entries: Vec<serde_json::Value> = Vec::new();
        // Entry bound: a directory with millions of entries must not produce
        // a million-element JSON response (the executor truncates the text,
        // but the JSON itself would still be built in full). The bound is
        // reported explicitly (`truncated`), never silent.
        const MAX_LIST_ENTRIES: usize = 10_000;

        for entry in fs::read_dir(&validated).context("failed to read directory")? {
            if entries.len() >= MAX_LIST_ENTRIES {
                break;
            }
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
        let truncated = entries.len() >= MAX_LIST_ENTRIES;

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "entries": entries,
                "count": entries.len(),
                "truncated": truncated,
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
        "move_path"
    }
    fn description(&self) -> &str {
        "Move or rename a file from source to destination"
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
            pua_report: Some(tool_execution_report("move_path", Some("file_moved"))),
        })
    }
}

// ── FileDeleteTool ─────────────────────────────────────────────────────────

pub struct FileDeleteTool;

impl Tool for FileDeleteTool {
    fn name(&self) -> &'static str {
        "delete_path"
    }
    fn description(&self) -> &str {
        "Delete a file (requires confirmation)"
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
            pua_report: Some(tool_execution_report("delete_path", Some("file_deleted"))),
        })
    }
}

// ── CreateDirectoryTool ──────────────────────────────────────────────────────

pub struct CreateDirectoryTool;

impl Tool for CreateDirectoryTool {
    fn name(&self) -> &'static str {
        "create_directory"
    }
    fn description(&self) -> &str {
        "Create a new directory (and all parent directories)"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated = sanitize_path_for_write(input, path)?;

        if validated.exists() {
            if validated.is_dir() {
                return Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "created_path": validated.to_string_lossy(),
                        "already_exists": true,
                    })),
                    error: None,
                    verification: Some("directory_created".to_string()),
                    audit_log: Some(format!("Directory already exists: {}", validated.display())),
                    pua_report: Some(tool_execution_report(
                        "create_directory",
                        Some("directory_created"),
                    )),
                });
            }
            anyhow::bail!("{} is not a directory", validated.display());
        }

        fs::create_dir_all(&validated).context("failed to create directory")?;

        info!(
            path = %validated.display(),
            "tool: directory created"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "created_path": validated.to_string_lossy(),
                "already_exists": false,
            })),
            error: None,
            verification: Some("directory_created".to_string()),
            audit_log: Some(format!("Created directory: {}", validated.display())),
            pua_report: Some(tool_execution_report(
                "create_directory",
                Some("directory_created"),
            )),
        })
    }
}

// ── EditFileTool ────────────────────────────────────────────────────────

pub struct EditFileTool;

impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }
    fn description(&self) -> &str {
        "Edit a file by replacing exact text with new text (precision text replacement)"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let old_text = input.payload["old_text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("edit_file requires arguments.old_text"))?;
        let new_text = input.payload["new_text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("edit_file requires arguments.new_text"))?;

        let validated = sanitize_path_for_write(input, path)?;

        if !validated.exists() {
            anyhow::bail!("{}", tf("error.path_not_found", &[("path", path)]));
        }

        // Read the file content (byte-capped: a model-picked huge file must
        // not be fully buffered before the replacement check).
        let content =
            String::from_utf8_lossy(&crate::orchestration::tool::exec_common::read_file_capped(
                &validated,
                crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
            )?)
            .into_owned();

        // Count occurrences of old_text
        let occurrences = content.matches(old_text).count();

        if occurrences == 0 {
            anyhow::bail!("edit_file: old_text not found in '{}'", validated.display());
        }

        if occurrences > 1 {
            anyhow::bail!(
                "edit_file: old_text found {} times in '{}'; expected exactly one occurrence",
                occurrences,
                validated.display()
            );
        }

        // Perform the single replacement
        let new_content = content.replace(old_text, new_text);

        // Same LAYER-2 write sandbox as write_file/apply_patch: the 50 MiB
        // disk-exhaustion cap and system-path blocklist must not be bypassable
        // via the edit path (previously only path containment was enforced).
        crate::orchestration::tool::enforce_write_sandbox(&validated, &new_content)?;

        fs::write(&validated, &new_content).context("failed to write file after edit")?;

        info!(
            path = %validated.display(),
            "tool: file edited successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated.to_string_lossy(),
                "replacement_count": 1,
            })),
            error: None,
            verification: Some("file_edited".to_string()),
            audit_log: Some(format!(
                "Edited file: {} (1 replacement)",
                validated.display()
            )),
            pua_report: Some(tool_execution_report("edit_file", Some("file_edited"))),
        })
    }
}

// ── CopyPathTool ────────────────────────────────────────────────────────────

pub struct CopyPathTool;

impl Tool for CopyPathTool {
    fn name(&self) -> &'static str {
        "copy_path"
    }
    fn description(&self) -> &str {
        "Copy a file or directory from source to destination"
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

        if source_path.is_dir() {
            super::utils::copy_dir_recursive(&source_path, &dest_path)?;
        } else {
            fs::copy(&source_path, &dest_path).context("failed to copy file")?;
        }

        info!(
            source = %source_path.display(),
            dest = %dest_path.display(),
            "tool: path copied successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "source": source_path.to_string_lossy(),
                "destination": dest_path.to_string_lossy(),
                "is_directory": source_path.is_dir(),
            })),
            error: None,
            verification: Some("path_copied".to_string()),
            audit_log: Some(format!(
                "Copied '{}' -> '{}'",
                source_path.display(),
                dest_path.display()
            )),
            pua_report: Some(tool_execution_report("copy_path", Some("path_copied"))),
        })
    }
}
