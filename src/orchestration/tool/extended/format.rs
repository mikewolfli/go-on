//! Code formatting tools.
//!
//! Runs external formatters (rustfmt, prettier, black, gofmt, etc.)
//! to auto-format code files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};

/// Formatter binaries permitted for `format_code` (the formatter name is
/// model-influenced, so anything outside the known set is rejected instead of
/// running an arbitrary binary — previously `Command::new(formatter)` executed
/// any model-provided program name without a sandbox or output cap).
const ALLOWED_FORMATTERS: &[&str] = &[
    "rustfmt",
    "prettier",
    "black",
    "gofmt",
    "google-java-format",
    "clang-format",
    "rubocop",
    "taplo",
];

pub struct FormatCodeTool;

impl Tool for FormatCodeTool {
    fn name(&self) -> &'static str {
        "format_code"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("format_code requires arguments.path"))?;
        let check = input.payload["check"].as_bool().unwrap_or(false);

        // Sandbox the path like every other filesystem tool (previously the
        // raw payload path was used directly).
        let path = sanitize_path(input, path)?;

        // Detect formatter
        let formatter = input.payload["formatter"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| detect_formatter(&path));

        if !ALLOWED_FORMATTERS.contains(&formatter.as_str()) {
            anyhow::bail!(
                "format_code: formatter '{formatter}' is not allowed; \
                 supported formatters: {}",
                ALLOWED_FORMATTERS.join(", ")
            );
        }

        // Build command: directories become the working directory, files are
        // passed as an argument (original semantics, now under the shared
        // sandbox + output cap used by every other command tool).
        let (workspace, mut args) = if path.is_dir() {
            (path.clone(), Vec::new())
        } else {
            let workspace = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let args = vec![path.to_string_lossy().into_owned()];
            (workspace, args)
        };
        if check {
            args.push("--check".to_string());
        }

        let output = crate::orchestration::tool::exec_common::run_sandboxed_capped(
            &workspace,
            &formatter,
            &args,
            crate::orchestration::tool::exec_common::MAX_OUTPUT_BYTES,
            |_| {},
        )
        .with_context(|| format!("Failed to run formatter '{formatter}'"))?;

        let stdout = output.stdout_lossy();
        let stderr = output.stderr_lossy();

        Ok(ToolOutput {
            success: output.status == Some(0),
            result: Some(json!({
                "formatter": formatter,
                "stdout": stdout,
                "stderr": stderr,
                "check": check,
                "exit_code": output.status,
                "stdout_truncated": output.stdout_truncated,
                "stderr_truncated": output.stderr_truncated,
            })),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

fn detect_formatter(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "rs" => return "rustfmt".to_string(),
        "js" | "ts" | "jsx" | "tsx" | "json" | "css" | "scss" | "html" | "md" => {
            return "prettier".to_string()
        }
        "py" => return "black".to_string(),
        "go" => return "gofmt".to_string(),
        "java" => return "google-java-format".to_string(),
        "c" | "h" | "cpp" | "hpp" => return "clang-format".to_string(),
        "rb" => return "rubocop".to_string(),
        "toml" if name == "Cargo.toml" => return "taplo".to_string(),
        // Unknown extension — fall through to project-level detection below.
        _ => {}
    }

    // Project-level detection for directories
    if path.is_dir() {
        if path.join(".prettierrc").exists() || path.join(".prettierrc.json").exists() {
            return "prettier".to_string();
        }
        if path.join("Cargo.toml").exists() {
            return "rustfmt".to_string();
        }
        if path.join("pyproject.toml").exists() {
            return "black".to_string();
        }
    }

    "prettier".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-format".to_string(),
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
    fn format_code_requires_path() {
        let tool = FormatCodeTool;
        let input = tool_input(json!({}));
        let result = tool.run(&input);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires arguments.path"));
    }

    #[test]
    fn detect_formatter_by_extension() {
        assert_eq!(detect_formatter(&PathBuf::from("main.rs")), "rustfmt");
        assert_eq!(detect_formatter(&PathBuf::from("app.js")), "prettier");
        assert_eq!(detect_formatter(&PathBuf::from("app.ts")), "prettier");
        assert_eq!(detect_formatter(&PathBuf::from("app.tsx")), "prettier");
        assert_eq!(detect_formatter(&PathBuf::from("main.py")), "black");
        assert_eq!(detect_formatter(&PathBuf::from("main.go")), "gofmt");
        assert_eq!(
            detect_formatter(&PathBuf::from("Main.java")),
            "google-java-format"
        );
        assert_eq!(detect_formatter(&PathBuf::from("lib.c")), "clang-format");
        assert_eq!(detect_formatter(&PathBuf::from("lib.h")), "clang-format");
        assert_eq!(detect_formatter(&PathBuf::from("lib.cpp")), "clang-format");
        assert_eq!(detect_formatter(&PathBuf::from("Gemfile.rb")), "rubocop");
    }

    #[test]
    fn detect_formatter_falls_back_to_prettier() {
        assert_eq!(detect_formatter(&PathBuf::from("makefile")), "prettier");
    }
}
