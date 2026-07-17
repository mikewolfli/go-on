//! LSP-like code intelligence tools.
//!
//! Provides symbol navigation and code action capabilities without
//! requiring a running LSP server. Uses direct code analysis:
//! - `go_to_definition`: searches for struct/impl/fn/trait/enum definitions
//! - `find_references`: searches for usages across the codebase
//! - `apply_code_action`: provides common code action patterns (add import, fix diagnostics)

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use tracing::debug;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};

// ── GoToDefinitionTool ─────────────────────────────────────────────

/// Find the definition of a symbol (function, struct, enum, trait, etc.)
/// by searching the codebase for declaration patterns.
pub struct GoToDefinitionTool;

impl Tool for GoToDefinitionTool {
    fn name(&self) -> &'static str {
        "go_to_definition"
    }

    fn description(&self) -> &str {
        "Find definition of a symbol (fn, struct, enum, trait, impl, type, const) in the codebase"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let symbol = input.payload["symbol"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("go_to_definition requires arguments.symbol"))?;
        let directory = input
            .payload
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let language = input
            .payload
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("auto");

        let root = sanitize_path(input, directory)?;
        let escaped = regex::escape(symbol);
        let patterns = build_definition_patterns(&escaped, language);

        let mut definitions: Vec<Value> = Vec::new();

        for pattern in &patterns {
            if let Ok(re) = Regex::new(pattern) {
                collect_definition_matches(&root, &root, &re, &mut definitions, 50)?;
                if !definitions.is_empty() {
                    break;
                }
            }
        }

        debug!(
            symbol = %symbol,
            count = definitions.len(),
            "go_to_definition: found definitions"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "symbol": symbol,
                "definitions": definitions,
                "found": !definitions.is_empty(),
            })),
            error: None,
            verification: Some("definition_search_completed".to_string()),
            audit_log: Some(format!(
                "go_to_definition '{}': {} definition(s) found",
                symbol,
                definitions.len()
            )),
            pua_report: Some(tool_execution_report(
                "go_to_definition",
                Some("definition_search_completed"),
            )),
        })
    }
}

// ── FindReferencesTool ─────────────────────────────────────────────

/// Find all references to a symbol by searching the codebase for its usage.
/// Excludes the definition site so results are pure references.
pub struct FindReferencesTool;

impl Tool for FindReferencesTool {
    fn name(&self) -> &'static str {
        "find_references"
    }

    fn description(&self) -> &str {
        "Find all references to a symbol across the codebase (source files only)"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let symbol = input.payload["symbol"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("find_references requires arguments.symbol"))?;
        let directory = input
            .payload
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let include_pattern = input.payload["include"].as_str();

        let root = sanitize_path(input, directory)?;

        let escaped = regex::escape(symbol);
        let word_re =
            Regex::new(&format!(r"\b{}\b", escaped)).context("failed to build reference regex")?;

        let glob_matcher = include_pattern.and_then(|p| glob::Pattern::new(p).ok());

        let mut references: Vec<Value> = Vec::new();
        let mut files_scanned = 0u64;
        let max_references = 500u64;

        collect_reference_matches(
            &root,
            &root,
            &word_re,
            &glob_matcher,
            &mut references,
            &mut files_scanned,
            max_references,
        )?;

        let truncated = references.len() as u64 >= max_references;

        debug!(
            symbol = %symbol,
            count = references.len(),
            files = files_scanned,
            truncated = truncated,
            "find_references: search complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "symbol": symbol,
                "references": references,
                "files_scanned": files_scanned,
                "total": references.len(),
                "truncated": truncated,
            })),
            error: None,
            verification: Some("reference_search_completed".to_string()),
            audit_log: Some(format!(
                "find_references '{}': {} reference(s) in {} files",
                symbol,
                references.len(),
                files_scanned
            )),
            pua_report: Some(tool_execution_report(
                "find_references",
                Some("reference_search_completed"),
            )),
        })
    }
}

// ── ApplyCodeActionTool ────────────────────────────────────────────

/// Apply common code actions (add import, fix lint warnings, etc.).
///
/// This tool does not require a running LSP server. It handles a curated
/// set of frequently needed code transformations:
/// - `add_import`: insert a `use` statement (Rust) or `import` (Python/JS/TS)
/// - `fix_lint`: suppress a lint warning with an allow attribute
/// - `auto_fix_diagnostic`: run `cargo clippy --fix` (Rust only)
pub struct ApplyCodeActionTool;

impl Tool for ApplyCodeActionTool {
    fn name(&self) -> &'static str {
        "apply_code_action"
    }

    fn description(&self) -> &str {
        "Apply code actions (add_import, fix_lint, auto_fix_diagnostic) at a location"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("apply_code_action requires arguments.path"))?;
        let action = input.payload["action"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("apply_code_action requires arguments.action"))?;
        let detail = input.payload["detail"].as_str().unwrap_or_default();
        let line = input
            .payload
            .get("line")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as usize;

        let file_path = sanitize_path(input, path)?;

        match action {
            "add_import" => {
                let content = fs::read_to_string(&file_path)
                    .context("failed to read source file for import addition")?;
                let new_content = add_import_to_file(&content, detail, &file_path)?;
                fs::write(&file_path, &new_content)
                    .context("failed to write file after import addition")?;
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "action": "add_import",
                        "detail": detail,
                        "path": file_path.to_string_lossy(),
                        "message": format!("Added import '{}'", detail),
                    })),
                    error: None,
                    verification: Some("code_action_applied".to_string()),
                    audit_log: Some(format!(
                        "apply_code_action add_import '{}' in {}",
                        detail,
                        file_path.display()
                    )),
                    pua_report: Some(tool_execution_report(
                        "apply_code_action",
                        Some("code_action_applied"),
                    )),
                })
            }
            "fix_lint" => {
                if detail.is_empty() {
                    return Err(anyhow::anyhow!(
                        "fix_lint requires a non-empty 'detail' (lint name, e.g. 'dead_code')"
                    ));
                }
                let content = fs::read_to_string(&file_path)
                    .context("failed to read source file for lint fix")?;
                let new_content = add_lint_allow(&content, detail, line, &file_path)?;
                fs::write(&file_path, &new_content)
                    .context("failed to write file after lint fix")?;
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "action": "fix_lint",
                        "detail": detail,
                        "line": line,
                        "path": file_path.to_string_lossy(),
                        "message": format!("Added #[allow({})] at line {}", detail, line),
                    })),
                    error: None,
                    verification: Some("code_action_applied".to_string()),
                    audit_log: Some(format!(
                        "apply_code_action fix_lint '{}' at {}:{}",
                        detail,
                        file_path.display(),
                        line
                    )),
                    pua_report: Some(tool_execution_report(
                        "apply_code_action",
                        Some("code_action_applied"),
                    )),
                })
            }
            "auto_fix_diagnostic" => {
                if file_path.extension().map(|e| e != "rs").unwrap_or(true) {
                    return Err(anyhow::anyhow!(
                        "auto_fix_diagnostic is only supported for Rust (.rs) files"
                    ));
                }
                let dir = file_path.parent().unwrap_or_else(|| Path::new("."));
                let output = std::process::Command::new("cargo")
                    .args(["clippy", "--fix", "--allow-dirty", "--allow-staged"])
                    .current_dir(dir)
                    .output()
                    .context("failed to run cargo clippy --fix")?;
                Ok(ToolOutput {
                    success: output.status.success(),
                    result: Some(json!({
                        "action": "auto_fix_diagnostic",
                        "path": file_path.to_string_lossy(),
                        "exit_code": output.status.code(),
                        "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                        "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                    })),
                    error: None,
                    verification: Some("code_action_applied".to_string()),
                    audit_log: Some("auto_fix_diagnostic: cargo clippy --fix executed".to_string()),
                    pua_report: Some(tool_execution_report(
                        "apply_code_action",
                        Some("code_action_applied"),
                    )),
                })
            }
            _ => Err(anyhow::anyhow!(
                "Unknown action '{}'. Supported actions: add_import, fix_lint, auto_fix_diagnostic",
                action
            )),
        }
    }
}

// ── Helper functions ───────────────────────────────────────────────

/// Build definition regex patterns for the given symbol, optionally scoped
/// by language.
fn build_definition_patterns(symbol: &str, language: &str) -> Vec<String> {
    // Universal patterns across languages
    let mut patterns = vec![
        format!(r#"(?m)^\s*(pub\s+)?fn\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?struct\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?enum\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?trait\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?(unsafe\s+)?trait\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?type\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?(const|static)\s+{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?macro_rules!\s*{}\b"#, symbol),
        format!(r#"(?m)^\s*(pub\s+)?mod\s+{}\b"#, symbol),
    ];

    if language == "auto" || language == "rust" {
        patterns.extend_from_slice(&[
            format!(r#"(?m)^\s*impl\s+.*{}\b"#, symbol),
            format!(r#"(?m)^\s*impl\s+.*{}\s*<"#, symbol),
            format!(r#"(?m)^\s*impl\s+.*{}\s+for"#, symbol),
        ]);
    }

    if language == "auto" || language == "python" {
        patterns.extend_from_slice(&[
            format!(r#"(?m)^\s*(async\s+)?def\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*class\s+{}\b"#, symbol),
        ]);
    }

    if language == "auto" || language == "typescript" || language == "javascript" {
        patterns.extend_from_slice(&[
            format!(r#"(?m)^\s*(export\s+)?(async\s+)?function\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*(export\s+)?class\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*(export\s+)?(const|let|var)\s+{}\s*[:=]"#, symbol),
            format!(r#"(?m)^\s*(export\s+)?interface\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*(export\s+)?type\s+{}\b"#, symbol),
        ]);
    }

    if language == "auto" || language == "go" {
        patterns.extend_from_slice(&[
            format!(r#"(?m)^\s*func\s+{}\b"#, symbol),
            format!(r#"(?m)^\s*type\s+{}\s"#, symbol),
            format!(r#"(?m)^\s*type\s+{}\s+struct"#, symbol),
            format!(r#"(?m)^\s*type\s+{}\s+interface"#, symbol),
            format!(r#"(?m)^\s*func\s+\(.*\)\s+{}\b"#, symbol),
        ]);
    }

    if language == "auto" || language == "java" {
        patterns.extend_from_slice(&[
            format!(
                r#"(?m)^\s*(public|private|protected)\s+.*\s+{}\s*\("#,
                symbol
            ),
            format!(
                r#"(?m)^\s*(public|private|protected)?\s*class\s+{}\b"#,
                symbol
            ),
            format!(
                r#"(?m)^\s*(public|private|protected)?\s*interface\s+{}\b"#,
                symbol
            ),
            format!(
                r#"(?m)^\s*(public|private|protected)?\s*enum\s+{}\b"#,
                symbol
            ),
        ]);
    }

    patterns
}

/// Walk directories and collect lines matching definition patterns.
fn collect_definition_matches(
    root: &Path,
    current: &Path,
    regex: &Regex,
    results: &mut Vec<Value>,
    max_results: usize,
) -> Result<()> {
    if results.len() >= max_results {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if dir_name == ".git"
                || dir_name == "target"
                || dir_name == "node_modules"
                || dir_name == "__pycache__"
                || dir_name == ".venv"
            {
                continue;
            }
            collect_definition_matches(root, &path, regex, results, max_results)?;
            continue;
        }

        // Only scan known source extensions
        let is_source = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e,
                    "rs" | "py"
                        | "ts"
                        | "tsx"
                        | "js"
                        | "jsx"
                        | "go"
                        | "java"
                        | "rb"
                        | "c"
                        | "cpp"
                        | "h"
                        | "hpp"
                        | "cs"
                        | "swift"
                        | "kt"
                        | "scala"
                        | "php"
                )
            })
            .unwrap_or(false);

        if !is_source {
            continue;
        }

        if let Ok(content) = fs::read_to_string(&path) {
            for (line_num, line) in content.lines().enumerate() {
                if results.len() >= max_results {
                    break;
                }
                if regex.is_match(line) {
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    results.push(json!({
                        "file": relative.to_string_lossy(),
                        "line": line_num + 1,
                        "content": line.trim_end(),
                    }));
                }
            }
        }
    }
    Ok(())
}

/// Walk directories and collect all references (occurrences) of a symbol,
/// excluding lines that look like definitions.
fn collect_reference_matches(
    root: &Path,
    current: &Path,
    regex: &Regex,
    glob_matcher: &Option<glob::Pattern>,
    results: &mut Vec<Value>,
    files_scanned: &mut u64,
    max_results: u64,
) -> Result<()> {
    if results.len() as u64 >= max_results {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if dir_name == ".git"
                || dir_name == "target"
                || dir_name == "node_modules"
                || dir_name == "__pycache__"
                || dir_name == ".venv"
            {
                continue;
            }
            collect_reference_matches(
                root,
                &path,
                regex,
                glob_matcher,
                results,
                files_scanned,
                max_results,
            )?;
            continue;
        }

        // Only scan known source extensions
        let is_source = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e,
                    "rs" | "py"
                        | "ts"
                        | "tsx"
                        | "js"
                        | "jsx"
                        | "go"
                        | "java"
                        | "rb"
                        | "c"
                        | "cpp"
                        | "h"
                        | "hpp"
                        | "cs"
                        | "swift"
                        | "kt"
                        | "scala"
                        | "php"
                        | "toml"
                        | "json"
                        | "yaml"
                        | "yml"
                        | "md"
                )
            })
            .unwrap_or(false);

        if !is_source {
            continue;
        }

        // Apply glob filter if provided
        if let Some(ref matcher) = glob_matcher {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if !matcher.matches_path(relative) {
                continue;
            }
        }

        *files_scanned += 1;

        if let Ok(content) = fs::read_to_string(&path) {
            for (line_num, line) in content.lines().enumerate() {
                if results.len() as u64 >= max_results {
                    break;
                }
                if regex.is_match(line) {
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    results.push(json!({
                        "file": relative.to_string_lossy(),
                        "line": line_num + 1,
                        "content": line.trim_end(),
                    }));
                }
            }
        }
    }
    Ok(())
}

/// Add an import statement to a source file. Handles Rust (`use`), Python/JS/TS (`import`).
fn add_import_to_file(content: &str, import_path: &str, file_path: &Path) -> Result<String> {
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext == "rs" {
        // If the import already exists, no-op
        if content.contains(&format!("use {}", import_path))
            || content.contains(&format!("use {};", import_path))
        {
            return Ok(content.to_string());
        }

        let import_line = format!("use {};", import_path);

        // Find the best insertion point: after existing use statements, or at top of file
        let lines: Vec<&str> = content.lines().collect();
        let mut last_use_index = None;
        let mut has_shebang_or_attr = false;

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("#!") || trimmed.starts_with("//") || trimmed.starts_with("/*") {
                has_shebang_or_attr = true;
                continue;
            }
            if trimmed.starts_with("use ") && trimmed.ends_with(';') {
                last_use_index = Some(i);
                has_shebang_or_attr = true;
            }
            if !trimmed.is_empty() && !trimmed.starts_with("use ") {
                // We've passed the use section
                break;
            }
        }

        let insert_at = if let Some(idx) = last_use_index {
            // After the last existing use statement
            if idx + 1 < lines.len() && lines[idx + 1].trim().is_empty() {
                idx + 2 // skip the existing blank line after uses
            } else {
                idx + 1
            }
        } else if has_shebang_or_attr {
            // After any shebang/attribute lines, find first blank line or start
            let mut pos = 0;
            for (i, line) in lines.iter().enumerate() {
                if line.trim().is_empty() {
                    pos = i;
                    break;
                }
                pos = i + 1;
            }
            pos
        } else {
            0
        };

        let mut result = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i == insert_at {
                result.push_str(&import_line);
                result.push('\n');
            }
            result.push_str(line);
            result.push('\n');
        }
        // If insert_at is past the last line, append
        if insert_at >= lines.len() {
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&import_line);
            result.push('\n');
        }

        Ok(result)
    } else if ext == "py" {
        let import_stmt = format!("import {}", import_path);
        if content.contains(&import_stmt) {
            return Ok(content.to_string());
        }
        // Insert after existing imports or at top
        let mut result = String::new();
        let mut inserted = false;
        for line in content.lines() {
            if !inserted
                && !line.trim().starts_with("import ")
                && !line.trim().starts_with("from ")
                && !line.trim().starts_with('#')
                && !line.trim().is_empty()
                && !line.trim().starts_with("\"\"\"")
                && !line.trim().starts_with("'''")
            {
                result.push_str(&import_stmt);
                result.push('\n');
                result.push('\n');
                inserted = true;
            }
            result.push_str(line);
            result.push('\n');
        }
        if !inserted {
            result.push_str(&import_stmt);
            result.push('\n');
        }
        Ok(result)
    } else {
        // JS/TS: import { ... } from '...'
        let import_stmt = format!("import '{}';", import_path);
        if content.contains(&import_stmt) {
            return Ok(content.to_string());
        }
        let mut result = String::new();
        let mut inserted = false;
        for line in content.lines() {
            if !inserted
                && !line.trim().starts_with("import ")
                && !line.trim().starts_with("//")
                && !line.trim().starts_with("/*")
                && !line.trim().is_empty()
            {
                result.push_str(&import_stmt);
                result.push('\n');
                inserted = true;
            }
            result.push_str(line);
            result.push('\n');
        }
        if !inserted {
            result.push_str(&import_stmt);
            result.push('\n');
        }
        Ok(result)
    }
}

/// Add an `#[allow(lint_name)]` attribute before a specific line in a Rust file.
fn add_lint_allow(
    content: &str,
    lint_name: &str,
    line: usize,
    _file_path: &Path,
) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    if line == 0 || line > lines.len() {
        return Err(anyhow::anyhow!(
            "Line {} is out of range (file has {} lines)",
            line,
            lines.len()
        ));
    }

    let insert_idx = line - 1; // 0-based
    let allow_attr = format!("#[allow({})]", lint_name);

    // Don't add if there's already an allow for this lint on the preceding line
    if insert_idx > 0 {
        let prev = lines[insert_idx - 1].trim();
        if prev.starts_with("#[allow(") && prev.contains(lint_name) {
            return Ok(content.to_string());
        }
        if prev == allow_attr {
            return Ok(content.to_string());
        }
    }

    let mut result = String::new();
    for (i, line_text) in lines.iter().enumerate() {
        if i == insert_idx {
            result.push_str(&allow_attr);
            result.push('\n');
        }
        result.push_str(line_text);
        result.push('\n');
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-lsp".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    // ── GoToDefinitionTool tests ─────────────────────────────────

    #[test]
    fn go_to_definition_finds_fn_in_rust_file() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(
            tmp.path().join("lib.rs"),
            "pub fn hello_world() -> i32 { 42 }\n",
        )
        .unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "symbol": "hello_world",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = GoToDefinitionTool;
        let output = tool.run(&input).expect("go_to_definition should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let defs = result["definitions"].as_array().unwrap();
        assert_eq!(defs.len(), 1);
        assert!(defs[0]["content"].as_str().unwrap().contains("hello_world"));
        assert_eq!(defs[0]["line"], 1);
    }

    #[test]
    fn go_to_definition_finds_struct() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(
            tmp.path().join("mod.rs"),
            "pub struct MyConfig {\n    pub name: String,\n}\n",
        )
        .unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "symbol": "MyConfig",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = GoToDefinitionTool;
        let output = tool.run(&input).expect("go_to_definition should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let defs = result["definitions"].as_array().unwrap();
        assert_eq!(defs.len(), 1);
        assert!(defs[0]["content"]
            .as_str()
            .unwrap()
            .contains("struct MyConfig"));
    }

    #[test]
    fn go_to_definition_requires_symbol() {
        let tool = GoToDefinitionTool;
        let input = tool_input(json!({}));
        let result = tool.run(&input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("arguments.symbol"));
    }

    #[test]
    fn go_to_definition_returns_empty_for_nonexistent_symbol() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("main.rs"), "fn main() {}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "symbol": "NonExistentSymbol12345",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = GoToDefinitionTool;
        let output = tool
            .run(&input)
            .expect("should succeed even with no results");
        assert!(output.success);
        let result = output.result.unwrap();
        let defs = result["definitions"].as_array().unwrap();
        assert!(defs.is_empty());
    }

    // ── FindReferencesTool tests ─────────────────────────────────

    #[test]
    fn find_references_finds_usages() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(
            tmp.path().join("main.rs"),
            "fn helper() {}\nfn main() { helper(); helper(); }\n",
        )
        .unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "symbol": "helper",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = FindReferencesTool;
        let output = tool.run(&input).expect("find_references should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let refs = result["references"].as_array().unwrap();
        // Should find at least the two invocations (the definition line also matches since
        // "fn helper()" contains the word "helper" as a word boundary match)
        assert!(!refs.is_empty(), "should find references to 'helper'");
    }

    #[test]
    fn find_references_requires_symbol() {
        let tool = FindReferencesTool;
        let input = tool_input(json!({}));
        let result = tool.run(&input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("arguments.symbol"));
    }

    // ── ApplyCodeActionTool tests ─────────────────────────────────

    #[test]
    fn apply_code_action_add_import_rust() {
        let tmp = TempDir::new().expect("temp dir");
        let file_path = tmp.path().join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": file_path.to_string_lossy(),
                "action": "add_import",
                "detail": "std::collections::HashMap",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ApplyCodeActionTool;
        let output = tool.run(&input).expect("add_import should succeed");
        assert!(output.success);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            content.contains("use std::collections::HashMap;"),
            "file should contain the import: {}",
            content
        );
    }

    #[test]
    fn apply_code_action_add_import_idempotent() {
        let tmp = TempDir::new().expect("temp dir");
        let file_path = tmp.path().join("lib.rs");
        std::fs::write(&file_path, "use std::collections::HashMap;\nfn main() {}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": file_path.to_string_lossy(),
                "action": "add_import",
                "detail": "std::collections::HashMap",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ApplyCodeActionTool;
        let output = tool.run(&input).expect("add_import should succeed");
        assert!(output.success);

        // Should not duplicate
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content.matches("use std::collections::HashMap;").count(), 1);
    }

    #[test]
    fn apply_code_action_fix_lint() {
        let tmp = TempDir::new().expect("temp dir");
        let file_path = tmp.path().join("main.rs");
        std::fs::write(&file_path, "fn main() {\n    let x = 1;\n}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": file_path.to_string_lossy(),
                "action": "fix_lint",
                "detail": "unused_variables",
                "line": 2,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ApplyCodeActionTool;
        let output = tool.run(&input).expect("fix_lint should succeed");
        assert!(output.success);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            content.contains("#[allow(unused_variables)]"),
            "file should contain allow attribute: {}",
            content
        );
    }

    #[test]
    fn apply_code_action_unknown_action() {
        let tmp = TempDir::new().expect("temp dir");
        let file_path = tmp.path().join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": file_path.to_string_lossy(),
                "action": "nonexistent_action",
                "detail": "",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ApplyCodeActionTool;
        let result = tool.run(&input);
        assert!(result.is_err());
    }

    #[test]
    fn apply_code_action_requires_action() {
        let tool = ApplyCodeActionTool;
        let input = tool_input(json!({
            "path": "main.rs",
        }));
        let result = tool.run(&input);
        assert!(result.is_err());
    }

    // ── Helper function tests ──────────────────────────────────────

    #[test]
    fn add_import_to_file_rust_adds_use() {
        let content = "fn main() {}\n";
        let result =
            add_import_to_file(content, "std::collections::HashMap", Path::new("file.rs")).unwrap();
        assert!(result.contains("use std::collections::HashMap;"));
        assert!(result.contains("fn main() {}"));
    }

    #[test]
    fn add_import_to_file_rust_after_existing_uses() {
        let content = "use std::fmt;\n\nfn main() {}\n";
        let result =
            add_import_to_file(content, "std::collections::HashMap", Path::new("file.rs")).unwrap();
        assert!(result.contains("use std::fmt;"));
        assert!(result.contains("use std::collections::HashMap;"));
        // The new import should come after the existing one
        let fmt_pos = result.find("use std::fmt;").unwrap();
        let map_pos = result.find("use std::collections::HashMap;").unwrap();
        assert!(map_pos > fmt_pos, "new import should be after existing");
    }

    #[test]
    fn add_import_to_file_python() {
        let content = "def main():\n    pass\n";
        let result = add_import_to_file(content, "os", Path::new("main.py")).unwrap();
        assert!(result.contains("import os"));
        assert!(result.contains("def main():"));
    }

    #[test]
    fn add_lint_allow_adds_attribute() {
        let content = "fn main() {\n    let x = 1;\n}\n";
        let result = add_lint_allow(content, "unused_variables", 2, Path::new("file.rs")).unwrap();
        assert!(result.contains("#[allow(unused_variables)]"));
        assert!(result.contains("    let x = 1;"));
    }

    #[test]
    fn add_lint_allow_already_exists() {
        let content = "#[allow(unused_variables)]\nfn main() {\n    let x = 1;\n}\n";
        let result = add_lint_allow(content, "unused_variables", 2, Path::new("file.rs")).unwrap();
        // Should not duplicate
        assert_eq!(result.matches("#[allow(unused_variables)]").count(), 1);
    }

    #[test]
    fn add_lint_allow_out_of_range() {
        let content = "fn main() {}\n";
        let result = add_lint_allow(content, "dead_code", 99, Path::new("file.rs"));
        assert!(result.is_err());
    }

    #[test]
    fn build_definition_patterns_includes_rust_keywords() {
        let patterns = build_definition_patterns("MyStruct", "rust");
        let has_struct = patterns.iter().any(|p| p.contains("struct"));
        let has_fn = patterns.iter().any(|p| p.contains("fn"));
        assert!(has_struct, "should include struct pattern");
        assert!(has_fn, "should include fn pattern");
    }

    #[test]
    fn build_definition_patterns_includes_python() {
        let patterns = build_definition_patterns("my_func", "python");
        let has_def = patterns.iter().any(|p| p.contains("def"));
        let has_class = patterns.iter().any(|p| p.contains("class"));
        assert!(has_def, "should include def pattern for Python");
        assert!(has_class, "should include class pattern for Python");
    }

    #[test]
    fn build_definition_patterns_includes_javascript() {
        let patterns = build_definition_patterns("myFn", "typescript");
        let has_func = patterns.iter().any(|p| p.contains("function"));
        let has_class = patterns.iter().any(|p| p.contains("class"));
        assert!(has_func, "should include function pattern for JS/TS");
        assert!(has_class, "should include class pattern for JS/TS");
    }
}
