//! Build and lint tools.
//!
//! Generic build runner and linter that work across languages.
//! Detects build system from project files.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::json;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use tracing::{debug, info, warn};

/// Run a build/lint/dependency command with capped output (OOM guard + LLM
/// context bound) and an explicit truncation warning. `args` are `String` so
/// the package name can be appended without lifetime gymnastics.
fn run_capped_tool<S: AsRef<std::ffi::OsStr>>(
    tool: &str,
    args: &[S],
    cwd: &Path,
    what: &str,
) -> Result<crate::orchestration::tool::exec_common::CappedCommandOutput> {
    let mut cmd = Command::new(tool);
    cmd.args(args).current_dir(cwd);
    let capped = crate::orchestration::tool::exec_common::run_command_capped(
        &mut cmd,
        crate::orchestration::tool::exec_common::MAX_OUTPUT_BYTES,
    )
    .with_context(|| format!("failed to execute '{}'", tool))?;
    if capped.stdout_truncated || capped.stderr_truncated {
        warn!(
            "{what}: output truncated at {} bytes (stdout={}, stderr={})",
            crate::orchestration::tool::exec_common::MAX_OUTPUT_BYTES,
            capped.stdout_truncated,
            capped.stderr_truncated
        );
    }
    Ok(capped)
}

/// Detect the build system for a given project directory.
/// Returns (build_tool, build_command_args) if detected.
fn detect_build_system(dir: &Path) -> Option<(&'static str, &'static [&'static str])> {
    if dir.join("Cargo.toml").exists() {
        Some(("cargo", &["build"]))
    } else if dir.join("package.json").exists() {
        Some(("npm", &["run", "build"]))
    } else if dir.join("pyproject.toml").exists() || dir.join("setup.py").exists() {
        Some(("python", &["-m", "build"]))
    } else if dir.join("Makefile").exists() || dir.join("makefile").exists() {
        Some(("make", &[]))
    } else {
        debug!("no build system detected in {}: none of Cargo.toml, package.json, pyproject.toml, setup.py, Makefile found", dir.display());
        None
    }
}

/// Detect the linter for a given project directory.
/// Returns (linter_name, linter_command_args) if detected.
fn detect_linter(dir: &Path) -> Option<(&'static str, &'static [&'static str])> {
    if dir.join("Cargo.toml").exists() {
        Some(("cargo", &["clippy"]))
    } else if dir.join("package.json").exists() {
        // Prefer eslint, fall back to npx eslint
        Some(("npx", &["eslint", "."]))
    } else if dir.join("pyproject.toml").exists() {
        // Prefer ruff, fall back to pylint
        Some(("ruff", &["check", "."]))
    } else if dir.join("setup.cfg").exists() || dir.join("setup.py").exists() {
        Some(("pylint", &["."]))
    } else {
        debug!("no linter detected in {}: none of Cargo.toml, package.json, pyproject.toml, setup.cfg, setup.py found", dir.display());
        None
    }
}

/// Detect the package manager for dependency addition.
/// Returns (manager_name, add_args_prefix) if detected.
fn detect_package_manager(dir: &Path) -> Option<(&'static str, &'static [&'static str])> {
    if dir.join("Cargo.toml").exists() {
        Some(("cargo", &["add"]))
    } else if dir.join("package.json").exists() {
        Some(("npm", &["install"]))
    } else if dir.join("requirements.txt").exists() {
        Some(("pip", &["install"]))
    } else if dir.join("go.mod").exists() {
        Some(("go", &["get"]))
    } else {
        debug!("no package manager detected in {}: none of Cargo.toml, package.json, requirements.txt, go.mod found", dir.display());
        None
    }
}

// ── RunBuildTool ─────────────────────────────────────────────────────────────

pub struct RunBuildTool;

impl Tool for RunBuildTool {
    fn name(&self) -> &'static str {
        "build_run"
    }
    fn description(&self) -> &str {
        "Detect and run the project's build system: cargo build, npm run build, python -m build, or make"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let current_dir = sanitize_path(input, directory)?;

        let (build_tool, args) = detect_build_system(&current_dir)
            .ok_or_else(|| anyhow::anyhow!(
                "unable to detect build system in '{}': no Cargo.toml, package.json, pyproject.toml, setup.py, or Makefile found",
                current_dir.display()
            ))?;

        let args_str = args.join(" ");
        debug!(
            build_tool = %build_tool,
            args = %args_str,
            directory = %current_dir.display(),
            "tool: build_run starting"
        );

        let capped = run_capped_tool(build_tool, args, &current_dir, "build_run")?;

        let success = capped.status == Some(0);
        let stdout = capped.stdout_lossy();
        let stderr = capped.stderr_lossy();
        let exit_code = capped.status;

        if success {
            info!(
                build_tool = %build_tool,
                exit_code = ?exit_code,
                "tool: build_run succeeded"
            );
        } else {
            warn!(
                build_tool = %build_tool,
                exit_code = ?exit_code,
                stderr = %stderr.trim(),
                "tool: build_run failed"
            );
        }

        Ok(ToolOutput {
            success,
            result: Some(json!({
                "build_tool": build_tool,
                "args": args,
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "directory": current_dir.to_string_lossy(),
            })),
            error: if success {
                None
            } else {
                Some(stderr.trim().to_string())
            },
            verification: Some("build_executed".to_string()),
            audit_log: Some(format!(
                "build_run {} {} in {} (exit: {:?})",
                build_tool,
                args_str,
                current_dir.display(),
                exit_code
            )),
            pua_report: Some(tool_execution_report("build_run", Some("build_executed"))),
        })
    }
}

// ── LintCodeTool ─────────────────────────────────────────────────────────────

pub struct LintCodeTool;

impl Tool for LintCodeTool {
    fn name(&self) -> &'static str {
        "lint_run"
    }
    fn description(&self) -> &str {
        "Detect and run the project's linter: cargo clippy, npx eslint, ruff, or pylint"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let current_dir = sanitize_path(input, directory)?;

        let (linter, args) = detect_linter(&current_dir)
            .ok_or_else(|| anyhow::anyhow!(
                "unable to detect linter in '{}': no Cargo.toml, package.json, pyproject.toml, setup.cfg, or setup.py found",
                current_dir.display()
            ))?;

        let args_str = args.join(" ");
        debug!(
            linter = %linter,
            args = %args_str,
            directory = %current_dir.display(),
            "tool: lint_run starting"
        );

        let capped = run_capped_tool(linter, args, &current_dir, "lint_run")?;

        let success = capped.status == Some(0);
        let stdout = capped.stdout_lossy();
        let stderr = capped.stderr_lossy();
        let exit_code = capped.status;

        info!(
            linter = %linter,
            exit_code = ?exit_code,
            success = %success,
            "tool: lint_run completed"
        );

        Ok(ToolOutput {
            success,
            result: Some(json!({
                "linter": linter,
                "args": args,
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "directory": current_dir.to_string_lossy(),
            })),
            error: if success {
                None
            } else {
                Some(stderr.trim().to_string())
            },
            verification: Some("lint_executed".to_string()),
            audit_log: Some(format!(
                "lint_run {} {} in {} (exit: {:?})",
                linter,
                args_str,
                current_dir.display(),
                exit_code
            )),
            pua_report: Some(tool_execution_report("lint_run", Some("lint_executed"))),
        })
    }
}

// ── AddDependencyTool ────────────────────────────────────────────────────────

pub struct AddDependencyTool;

impl Tool for AddDependencyTool {
    fn name(&self) -> &'static str {
        "dependency_add"
    }
    fn description(&self) -> &str {
        "Add a dependency to the project: cargo add, npm install, pip install, or go get"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let package = input.payload["package"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required field: package"))?;

        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let current_dir = sanitize_path(input, directory)?;

        let (manager, args_prefix) = detect_package_manager(&current_dir)
            .ok_or_else(|| anyhow::anyhow!(
                "unable to detect package manager in '{}': no Cargo.toml, package.json, requirements.txt, or go.mod found",
                current_dir.display()
            ))?;

        // Build the full argument list: prefix args + package name
        let mut full_args: Vec<&str> = Vec::from(args_prefix);
        full_args.push(package);

        let args_str = full_args.join(" ");
        debug!(
            manager = %manager,
            package = %package,
            args = %args_str,
            directory = %current_dir.display(),
            "tool: dependency_add starting"
        );

        let capped = run_capped_tool(manager, &full_args, &current_dir, "dependency_add")?;

        let success = capped.status == Some(0);
        let stdout = capped.stdout_lossy();
        let stderr = capped.stderr_lossy();
        let exit_code = capped.status;

        if success {
            info!(
                manager = %manager,
                package = %package,
                "tool: dependency_add succeeded"
            );
        } else {
            warn!(
                manager = %manager,
                package = %package,
                stderr = %stderr.trim(),
                "tool: dependency_add failed"
            );
        }

        Ok(ToolOutput {
            success,
            result: Some(json!({
                "package_manager": manager,
                "package": package,
                "args": full_args,
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "directory": current_dir.to_string_lossy(),
            })),
            error: if success {
                None
            } else {
                Some(stderr.trim().to_string())
            },
            verification: Some("dependency_added".to_string()),
            audit_log: Some(format!(
                "dependency_add {} {} in {} (exit: {:?})",
                manager,
                package,
                current_dir.display(),
                exit_code
            )),
            pua_report: Some(tool_execution_report(
                "dependency_add",
                Some("dependency_added"),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-build".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    fn init_rust_project(tmp: &TempDir) {
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            r#"[package]
name = "test-project"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        std::fs::write(tmp.path().join("src").join("lib.rs"), "").unwrap();
    }

    #[test]
    fn detect_build_system_rust() {
        let tmp = TempDir::new().expect("temp dir");
        init_rust_project(&tmp);
        let detected = detect_build_system(tmp.path());
        assert!(detected.is_some());
        let (tool, args) = detected.unwrap();
        assert_eq!(tool, "cargo");
        assert_eq!(args, &["build"]);
    }

    #[test]
    fn detect_linter_rust() {
        let tmp = TempDir::new().expect("temp dir");
        init_rust_project(&tmp);
        let detected = detect_linter(tmp.path());
        assert!(detected.is_some());
        let (linter, args) = detected.unwrap();
        assert_eq!(linter, "cargo");
        assert_eq!(args, &["clippy"]);
    }

    #[test]
    fn detect_package_manager_rust() {
        let tmp = TempDir::new().expect("temp dir");
        init_rust_project(&tmp);
        let detected = detect_package_manager(tmp.path());
        assert!(detected.is_some());
        let (manager, args) = detected.unwrap();
        assert_eq!(manager, "cargo");
        assert_eq!(args, &["add"]);
    }

    #[test]
    fn build_run_fails_without_project() {
        let tmp = TempDir::new().expect("temp dir");
        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };
        let tool = RunBuildTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "build_run should fail without a project file"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unable to detect build system"),
            "error should mention detection failure, got: {}",
            err
        );
    }

    #[test]
    fn lint_run_fails_without_project() {
        let tmp = TempDir::new().expect("temp dir");
        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };
        let tool = LintCodeTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "lint_run should fail without a project file"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unable to detect linter"),
            "error should mention detection failure, got: {}",
            err
        );
    }

    #[test]
    fn dependency_add_requires_package() {
        let input = tool_input(json!({}));
        let tool = AddDependencyTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "dependency_add should fail without package"
        );
    }

    #[test]
    fn dependency_add_fails_without_project() {
        let tmp = TempDir::new().expect("temp dir");
        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({
                "package": "some-package",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };
        let tool = AddDependencyTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "dependency_add should fail without a project file"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unable to detect package manager"),
            "error should mention detection failure, got: {}",
            err
        );
    }
}
