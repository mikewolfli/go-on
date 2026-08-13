//! Code metrics and analysis tools.
//!
//! Computes cyclomatic complexity, line counts, function sizes,
//! and other code quality metrics.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tracing::debug;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};

pub struct CodeMetricsTool;

impl Tool for CodeMetricsTool {
    fn name(&self) -> &'static str {
        "code_metrics"
    }

    fn description(&self) -> &str {
        "Analyze source code files and compute code quality metrics (lines of code, \
         cyclomatic complexity, function/class sizes)"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input
            .payload
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let glob_pattern = input
            .payload
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("**/*.rs");

        let base_dir = sanitize_path(input, directory)?;
        debug!(
            directory = %directory,
            pattern = %glob_pattern,
            "tool: code_metrics"
        );

        // Collect matching files using the `glob` crate.
        // Sandbox: the pattern is joined onto the sanitized base dir, so an
        // absolute pattern (or one containing `..`) would escape — reject
        // both, mirroring search_files' root-relative semantics.
        if glob_pattern.starts_with('/') || glob_pattern.starts_with('\\') {
            anyhow::bail!("code_metrics: pattern must be relative to the search directory");
        }
        if glob_pattern.split(['/', '\\']).any(|seg| seg == "..") {
            anyhow::bail!("code_metrics: pattern must not contain '..' segments");
        }
        let full_pattern = base_dir.join(glob_pattern);
        let pattern_str = full_pattern.to_string_lossy().to_string();
        // File-count bound: a `**/*` pattern over a huge tree must not read
        // every file into memory. The bound is reported explicitly.
        const MAX_METRICS_FILES: usize = 5_000;
        let mut globber: Vec<PathBuf> = glob::glob(&pattern_str)
            .context("failed to parse glob pattern")?
            .filter_map(Result::ok)
            .filter(|p| p.is_file())
            .take(MAX_METRICS_FILES + 1)
            .collect();
        let truncated = globber.len() > MAX_METRICS_FILES;
        globber.truncate(MAX_METRICS_FILES);

        if globber.is_empty() {
            return Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "files_analyzed": 0,
                    "total_lines": 0,
                    "metrics": [],
                    "note": "No files matched the given pattern"
                })),
                error: None,
                verification: Some("code_metrics_completed".to_string()),
                audit_log: Some("code_metrics: no files matched".to_string()),
                pua_report: Some(tool_execution_report(
                    "code_metrics",
                    Some("code_metrics_completed"),
                )),
            });
        }

        let mut all_metrics = Vec::new();
        let mut total_lines = 0usize;

        for path in &globber {
            let metrics = analyze_file(path).unwrap_or_else(|e| {
                json!({
                    "file": path.to_string_lossy(),
                    "error": e.to_string()
                })
            });
            if let Some(lines) = metrics.get("total_lines").and_then(|v| v.as_u64()) {
                total_lines += lines as usize;
            }
            all_metrics.push(metrics);
        }

        debug!(
            files = %all_metrics.len(),
            total_lines = %total_lines,
            "tool: code_metrics complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "files_analyzed": all_metrics.len(),
                "total_lines": total_lines,
                "truncated": truncated,
                "metrics": all_metrics,
            })),
            error: None,
            verification: Some("code_metrics_completed".to_string()),
            audit_log: Some(format!(
                "code_metrics: {} files, {} total lines",
                all_metrics.len(),
                total_lines
            )),
            pua_report: Some(tool_execution_report(
                "code_metrics",
                Some("code_metrics_completed"),
            )),
        })
    }
}

/// Analyze a single source file for code metrics.
fn analyze_file(path: &std::path::Path) -> Result<Value> {
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .with_context(|| format!("failed to read {}", path.display()))?;

    let total_lines = content.lines().count();
    let code_lines = content
        .lines()
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("//")
                && !trimmed.starts_with('#')
                && !trimmed.starts_with("/*")
                && !trimmed.starts_with('*')
        })
        .count();
    let blank_lines = content.lines().filter(|l| l.trim().is_empty()).count();
    let comment_lines = total_lines - code_lines - blank_lines;

    // Estimate cyclomatic complexity by counting branching keywords.
    let complexity = content
        .lines()
        .map(|l| {
            let trimmed = l.trim();
            // Ignore commented-out lines.
            if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") {
                return 0usize;
            }
            let mut count = 0usize;
            // Each if/for/while/&&/||/match arm contributes to complexity.
            count += trimmed.matches(" if ").count();
            count += trimmed.matches(" for ").count();
            count += trimmed.matches(" while ").count();
            count += trimmed.matches(" match ").count();
            count += trimmed.matches(" && ").count();
            count += trimmed.matches(" || ").count();
            count += trimmed.matches(" catch ").count();
            count += trimmed.matches("case ").count();
            // Default base complexity is 1.
            count
        })
        .sum::<usize>()
        .max(1);

    // Detect function/method definitions (Rust `fn`, JS `function`, Python `def`, etc.).
    let functions: Vec<&str> = content
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.starts_with("fn ")
                || t.starts_with("pub fn ")
                || t.starts_with("pub(crate) fn ")
                || t.starts_with("function ")
                || t.starts_with("def ")
                || t.starts_with("func ")
        })
        .collect();

    let function_count = functions.len();
    // Approximate max function size by looking for lines between `fn` and the next `fn`.
    let max_function_size = estimate_max_function_size(&content, &functions);

    let mut result = json!({
        "file": path.to_string_lossy(),
        "total_lines": total_lines,
        "code_lines": code_lines,
        "blank_lines": blank_lines,
        "comment_lines": comment_lines,
        "cyclomatic_complexity": complexity,
        "function_count": function_count,
        "estimated_max_function_size": max_function_size,
    });

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        result["extension"] = json!(ext);
    }

    Ok(result)
}

/// Roughly estimate the largest function size by counting lines between
/// consecutive function definitions (or end-of-file).
fn estimate_max_function_size(content: &str, functions: &[&str]) -> usize {
    if functions.is_empty() {
        return 0;
    }

    let lines: Vec<&str> = content.lines().collect();
    let fn_line_numbers: Vec<usize> = functions
        .iter()
        .filter_map(|f| {
            lines.iter().position(|l| *l == f.trim())
            // Use a simple scan for the first matching line.
        })
        .collect();

    if fn_line_numbers.is_empty() {
        // Fallback: find by substring.
        let fn_lines: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                let t = l.trim();
                functions.iter().any(|f| {
                    let f_trimmed = f.trim();
                    t == f_trimmed || t.starts_with(f_trimmed)
                })
            })
            .map(|(i, _)| i)
            .collect();

        if fn_lines.is_empty() {
            return 0;
        }

        let mut max_size = 0usize;
        for i in 0..fn_lines.len() {
            let start = fn_lines[i];
            let end = fn_lines.get(i + 1).copied().unwrap_or(lines.len());
            let size = end.saturating_sub(start);
            if size > max_size {
                max_size = size;
            }
        }
        max_size
    } else {
        let mut max_size = 0usize;
        for i in 0..fn_line_numbers.len() {
            let start = fn_line_numbers[i];
            let end = fn_line_numbers.get(i + 1).copied().unwrap_or(lines.len());
            let size = end.saturating_sub(start);
            if size > max_size {
                max_size = size;
            }
        }
        max_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-metrics".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn code_metrics_analyzes_single_file() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(
            tmp.path().join("example.rs"),
            "fn main() {\n    let x = 1;\n    if x > 0 {\n        println!(\"ok\");\n    }\n}\n",
        )
        .unwrap();

        let input = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
            "pattern": "*.rs",
        }));
        let tool = CodeMetricsTool;
        let output = tool.run(&input).expect("code_metrics should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert_eq!(result["files_analyzed"].as_u64().unwrap(), 1);
        let metrics = result["metrics"].as_array().unwrap();
        let entry = &metrics[0];
        assert!(entry["total_lines"].as_u64().unwrap() >= 6);
        assert!(entry["cyclomatic_complexity"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn code_metrics_returns_empty_for_no_match() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("data.txt"), "hello").unwrap();

        let input = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
            "pattern": "*.py",
        }));
        let tool = CodeMetricsTool;
        let output = tool
            .run(&input)
            .expect("code_metrics should succeed even with no matches");
        assert!(output.success);
        let result = output.result.unwrap();
        assert_eq!(result["files_analyzed"].as_u64().unwrap(), 0);
    }
}
