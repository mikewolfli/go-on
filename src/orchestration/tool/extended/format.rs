//! Code formatting tools.
//!
//! Runs external formatters (rustfmt, prettier, black, gofmt, etc.)
//! to auto-format code files.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::json;

use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

pub struct FormatCodeTool;

impl Tool for FormatCodeTool {
    fn name(&self) -> &'static str {
        "format_code"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("format_code requires arguments.path"))?;
        let path = PathBuf::from(path);
        let check = input.payload["check"].as_bool().unwrap_or(false);

        // Detect formatter
        let formatter = input.payload["formatter"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| detect_formatter(&path));

        // Build command
        let mut cmd = Command::new(&formatter);
        if path.is_dir() {
            cmd.current_dir(&path);
        } else {
            cmd.arg(&path);
        }
        if check {
            cmd.arg("--check");
        }

        let output = cmd
            .output()
            .with_context(|| format!("Failed to run formatter '{formatter}'"))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok(ToolOutput {
            success: output.status.success(),
            result: Some(json!({
                "formatter": formatter,
                "stdout": stdout,
                "stderr": stderr,
                "check": check,
                "exit_code": output.status.code(),
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
