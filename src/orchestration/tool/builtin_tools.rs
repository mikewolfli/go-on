//! Built-in tools for the tool orchestration system.
//!
//! This module contains the built-in tool implementations registered by
//! default in `ToolRegistry::new()`: file I/O, search, patch, test, git diff,
//! and skill management tools.
//!
//! Each tool implements the [`Tool`] trait and is registered via
//! [`ToolRegistry::register_with_profile`].

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::lock::LockMode;
use crate::orchestration::tool::types::*;
use crate::orchestration::tool::SKILL_REGISTRY;
use anyhow::Result;
use glob::Pattern;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, warn};

// Re-export the shared statics used by built-in tools from the parent module.
use crate::orchestration::tool::tool_lock_manager;

// ============================================================================
// Helper functions
// ============================================================================

/// Record a tool execution metric via the global performance monitor.
///
/// Tracks tool call count, latency, and success/failure for observability.
/// Uses a single `trace!` macro instead of creating a separate span, which
/// avoids per-call overhead from span construction and enter/exit (P3-17).
pub fn record_tool_execution(
    metric_name: &str,
    tool: &str,
    success: bool,
    latency_ms: u64,
    input_size: Option<usize>,
) {
    crate::observability::performance::record_global_operation(success, latency_ms as f64);

    tracing::trace!(
        target: "tool_execution",
        metric = %metric_name,
        tool = %tool,
        input_size = input_size.unwrap_or(0),
        latency_ms = latency_ms,
        success = success,
        "tool execution metric"
    );
}

/// Sanitize and validate a file path against the allowed base directory.
///
/// 1. Resolves the path relative to the current working directory.
/// 2. Canonicalizes (or normalizes) the resolved path.
/// 3. If `allowed_base_dir` is set, verifies the resolved path starts with it.
pub fn sanitize_path(input: &ToolInput, path: &str) -> Result<PathBuf> {
    let resolved = PathBuf::from(path);
    let canonical = if resolved.is_absolute() {
        std::fs::canonicalize(&resolved)
            .map_err(|e| anyhow::anyhow!("path canonicalization failed: {e}"))?
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| anyhow::anyhow!("unable to determine current directory: {e}"))?;
        let joined = cwd.join(&resolved);
        std::fs::canonicalize(&joined)
            .map_err(|e| anyhow::anyhow!("path canonicalization failed: {e}"))?
    };

    if let Some(ref base_dir) = input.allowed_base_dir {
        let base_canonical = std::fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.clone());
        if !canonical.starts_with(&base_canonical) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.path_traversal_denied",
                    &[("path", path), ("base", &base_dir.display().to_string())]
                )
            );
        }
    }

    Ok(canonical)
}

/// Maximum size for model-supplied write payloads (write_file content,
/// apply_patch patch) — disk-exhaustion guard, shared by the sandbox
/// functions so the two write tools cannot drift.
pub const MAX_WRITE_PAYLOAD_BYTES: usize = 50 * 1024 * 1024;

/// LAYER 2 runtime write sandbox shared by the sync and async write paths.
///
/// Enforces a maximum write size (disk-exhaustion guard) and blocks writes to
/// sensitive system paths. Both `WriteFileTool::run` and `run_async` must go
/// through this gate so the model cannot bypass the sandbox via the async path.
pub fn enforce_write_sandbox(path: &std::path::Path, content: &str) -> Result<()> {
    // Limit file size to prevent disk exhaustion (default 50MB).
    if content.len() > MAX_WRITE_PAYLOAD_BYTES {
        anyhow::bail!(
            "write_file BLOCKED: content {} bytes exceeds maximum {} bytes",
            content.len(),
            MAX_WRITE_PAYLOAD_BYTES
        );
    }

    // Block writing to sensitive system paths.
    let path_str = path.to_string_lossy().to_lowercase();
    let blocked_paths = [
        "/etc/",
        "/sys/",
        "/proc/",
        "/dev/",
        "/boot/",
        "/var/log/",
        "/var/db/",
        "/usr/lib/",
        "/usr/bin/",
        "C:\\windows\\",
        "C:\\Program Files\\",
    ];
    for blocked in &blocked_paths {
        if path_str.contains(blocked) {
            anyhow::bail!(
                "write_file BLOCKED: writing to system path '{}' is not allowed",
                blocked
            );
        }
    }
    Ok(())
}

/// Sanitize and validate a path that may not exist yet (e.g. destination for
/// move/write operations). Resolves the parent directory and joins the
/// filename, then validates against the allowed base directory.
pub fn sanitize_path_for_write(input: &ToolInput, path: &str) -> Result<PathBuf> {
    let resolved = PathBuf::from(path);

    // Try canonicalizing the resolved path first; if it exists, use it directly.
    let canonical = if resolved.is_absolute() {
        std::fs::canonicalize(&resolved).unwrap_or_else(|_| {
            // Path doesn't exist — resolve via parent directory
            let parent = resolved
                .parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .unwrap_or_else(|| {
                    // If parent can't be canonicalized either, return resolved as-is
                    PathBuf::from(path)
                });
            parent.join(resolved.file_name().unwrap_or_default())
        })
    } else {
        let cwd = match std::env::current_dir() {
            Ok(dir) => dir,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "unable to determine current directory: {e}"
                ));
            }
        };
        let joined = cwd.join(&resolved);
        std::fs::canonicalize(&joined).unwrap_or_else(|_| {
            let parent = joined
                .parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .unwrap_or_else(|| cwd.clone());
            parent.join(joined.file_name().unwrap_or_default())
        })
    };

    // Dangling-symlink guard: when the leaf itself is a symlink whose target
    // does not exist yet, `canonicalize` above failed and the fallback
    // rejoined the unresolved leaf name — writing would follow the link to
    // an arbitrary target outside the base dir. Reject symlink leaves
    // outright. (When canonicalize succeeds the path is already resolved, so
    // this never fires on real files/dirs.)
    if let Ok(meta) = std::fs::symlink_metadata(&canonical) {
        if meta.file_type().is_symlink() {
            anyhow::bail!(
                "{}",
                tf(
                    "error.path_traversal_denied",
                    &[
                        ("path", path),
                        ("base", "symbolic links are not writable targets")
                    ]
                )
            );
        }
    }

    // Fallback paths may still contain unresolved `..` components (when an
    // intermediate directory doesn't exist yet, canonicalize fails and the
    // raw string is rejoined). `Path::starts_with` compares components
    // without normalizing `..`, so such a path could pass the base check
    // below and then escape the base when the OS resolves the `..` on write.
    // Reject outright — a canonicalize-success path never contains `..`.
    if canonical
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        anyhow::bail!(
            "{}",
            tf(
                "error.path_traversal_denied",
                &[("path", path), ("base", ".. components are not allowed")]
            )
        );
    }

    if let Some(ref base_dir) = input.allowed_base_dir {
        let base_canonical = std::fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.clone());
        if !canonical.starts_with(&base_canonical) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.path_traversal_denied",
                    &[("path", path), ("base", &base_dir.display().to_string())]
                )
            );
        }
    }

    Ok(canonical)
}

// ============================================================================
// ReadFileTool
// ============================================================================

pub struct ReadFileTool;
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let validated_path = sanitize_path(input, path)?;

        // Non-blocking try_acquire read lock to prevent concurrent writes.
        // If lock is contended, read proceeds without lock — the OS file
        // system provides coherence for concurrent reads.
        let _lock =
            tool_lock_manager().try_acquire(&validated_path.to_string_lossy(), LockMode::Read);

        let content = crate::orchestration::tool::exec_common::read_file_capped(
            &validated_path,
            crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
        )?;
        let content = String::from_utf8_lossy(&content).into_owned();
        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", true);
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"content": content})),
            error: None,
            verification: Some("file_read".to_string()),
            audit_log: Some(format!("Read file: {}", validated_path.display())),
            pua_report: Some(tool_execution_report("read_file", Some("file_read"))),
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let path = input.payload["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
            let validated_path = sanitize_path(&input, path)?;

            let _lock =
                tool_lock_manager().try_acquire(&validated_path.to_string_lossy(), LockMode::Read);

            let content = crate::orchestration::tool::exec_common::read_file_capped_async(
                &validated_path,
                crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
            )
            .await?;
            let content = String::from_utf8_lossy(&content).into_owned();

            Ok(ToolOutput {
                success: true,
                result: Some(serde_json::json!({"content": content})),
                error: None,
                verification: Some("file_read".to_string()),
                audit_log: Some(format!("Read file: {}", validated_path.display())),
                pua_report: Some(tool_execution_report("read_file", Some("file_read"))),
            })
        })
    }
}

// ============================================================================
// WriteFileTool
// ============================================================================

pub struct WriteFileTool;
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let content = input.payload["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_content")))?;
        let mode = input.payload["mode"].as_str().unwrap_or("overwrite");
        let path_buf = sanitize_path_for_write(input, path)?;

        // ── LAYER 2: Runtime sandbox (shared with run_async) ────────────
        enforce_write_sandbox(&path_buf, content)?;

        // Non-blocking try_acquire write lock.
        // If lock is already held by another operation, return a transient
        // error so the TAO loop can retry.
        let _lock = tool_lock_manager()
            .try_acquire(&path_buf.to_string_lossy(), LockMode::Write)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "write lock contended for '{}' — another tool is modifying this file",
                    path_buf.display()
                )
            })?;

        if let Some(parent) = path_buf.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        match mode {
            "append" => {
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path_buf)?;
                file.write_all(content.as_bytes())?;
            }
            "overwrite" => {
                fs::write(&path_buf, content)?;
            }
            other => {
                anyhow::bail!("{}", tf("error.unsupported_write_mode", &[("mode", other)]));
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", true);
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"path": path, "mode": mode})),
            error: None,
            verification: Some("file_written".to_string()),
            audit_log: Some(format!("Wrote file: {} ({})", path_buf.display(), mode)),
            pua_report: Some(tool_execution_report("write_file", Some("file_written"))),
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let path = input.payload["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
            let content = input.payload["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_content")))?;
            let mode = input.payload["mode"].as_str().unwrap_or("overwrite");
            let path_buf = sanitize_path_for_write(&input, path)?;

            // ── LAYER 2: Runtime sandbox (shared with run) ────────────────
            enforce_write_sandbox(&path_buf, content)?;

            let _lock = tool_lock_manager()
                .try_acquire(&path_buf.to_string_lossy(), LockMode::Write)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "write lock contended for '{}' — another tool is modifying this file",
                        path_buf.display()
                    )
                })?;

            if let Some(parent) = path_buf.parent() {
                if !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent).await?;
                }
            }

            match mode {
                "append" => {
                    let mut file = tokio::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path_buf)
                        .await?;
                    tokio::io::AsyncWriteExt::write_all(&mut file, content.as_bytes()).await?;
                }
                "overwrite" => {
                    tokio::fs::write(&path_buf, content).await?;
                }
                other => {
                    anyhow::bail!("{}", tf("error.unsupported_write_mode", &[("mode", other)]));
                }
            }

            Ok(ToolOutput {
                success: true,
                result: Some(serde_json::json!({"path": path, "mode": mode})),
                error: None,
                verification: Some("file_written".to_string()),
                audit_log: Some(format!("Wrote file: {} ({})", path_buf.display(), mode)),
                pua_report: Some(tool_execution_report("write_file", Some("file_written"))),
            })
        })
    }
}

// ============================================================================
// SearchFilesTool
// ============================================================================

pub struct SearchFilesTool;
impl Tool for SearchFilesTool {
    fn name(&self) -> &'static str {
        "search_files"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let pattern = input.payload["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_pattern")))?;
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let root = sanitize_path(input, directory)?;
        let matcher = Pattern::new(pattern)?;
        let mut files = Vec::new();
        crate::orchestration::tool::file_walk::collect_matching_files(
            &root, &root, &matcher, &mut files,
        )?;

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", true);
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"files": files})),
            error: None,
            verification: Some("search_done".to_string()),
            audit_log: Some(format!(
                "Search files completed for pattern '{}' in '{}'",
                pattern,
                root.display()
            )),
            pua_report: Some(tool_execution_report("search_files", Some("search_done"))),
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let pattern = input.payload["pattern"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_pattern")))?;
            let directory = input.payload["directory"].as_str().unwrap_or(".");
            let root = sanitize_path(&input, directory)?;
            let matcher = glob::Pattern::new(pattern)?;

            let files = crate::orchestration::tool::file_walk::collect_matching_files_async(
                root.clone(),
                matcher,
            )
            .await?;

            Ok(ToolOutput {
                success: true,
                result: Some(serde_json::json!({"files": files})),
                error: None,
                verification: Some("search_done".to_string()),
                audit_log: Some(format!(
                    "Search files completed for pattern '{}' in '{}'",
                    pattern,
                    root.display()
                )),
                pua_report: Some(tool_execution_report("search_files", Some("search_done"))),
            })
        })
    }
}

// ============================================================================
// ApplyPatchTool
// ============================================================================

pub struct ApplyPatchTool;
impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let patch = input.payload["patch"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_patch")))?;
        // ── LAYER 2: runtime sandbox — bound the patch size (same cap as
        // write_file) so the model cannot pipe unbounded data to git apply.
        // (Path safety is git apply's own concern: it rejects absolute and
        // `..` paths by default.)
        if patch.len() > MAX_WRITE_PAYLOAD_BYTES {
            anyhow::bail!(
                "apply_patch BLOCKED: patch {} bytes exceeds maximum {} bytes",
                patch.len(),
                MAX_WRITE_PAYLOAD_BYTES
            );
        }
        let check_only = input.payload["check"].as_bool().unwrap_or(false);
        let current_dir = input.payload["directory"].as_str().unwrap_or(".");
        let sanitized_dir = sanitize_path(input, current_dir)?;
        let mut command = Command::new("git");
        command.arg("apply");
        if check_only {
            command.arg("--check");
        }
        // Pipe patch via stdin to avoid Windows \\?\ long-path prefix issues
        // that arise when using tempfile (git apply can't open \\?\ prefixed paths).
        command.arg("-");
        debug!(directory = %current_dir, check_only = %check_only, "tool: running git apply");
        command.current_dir(&sanitized_dir);
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let mut child = command.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(patch.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        let success = output.status.success();
        if !success {
            warn!(
                directory = %current_dir,
                check_only = %check_only,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "tool: git apply failed"
            );
        }

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", success);
        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "applied": success && !check_only,
                "checked": check_only,
                "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some(
                if check_only {
                    "patch_checked"
                } else {
                    "patch_applied"
                }
                .to_string(),
            ),
            audit_log: Some(format!("git apply executed in '{}'", current_dir)),
            pua_report: Some(tool_execution_report(
                "apply_patch",
                Some(if check_only {
                    "patch_checked"
                } else {
                    "patch_applied"
                }),
            )),
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let patch = input.payload["patch"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_patch")))?;
            // ── LAYER 2: runtime sandbox — bound the patch size (same cap as
            // the sync path / write_file).
            if patch.len() > MAX_WRITE_PAYLOAD_BYTES {
                anyhow::bail!(
                    "apply_patch BLOCKED: patch {} bytes exceeds maximum {} bytes",
                    patch.len(),
                    MAX_WRITE_PAYLOAD_BYTES
                );
            }
            let check_only = input.payload["check"].as_bool().unwrap_or(false);
            let current_dir = input.payload["directory"].as_str().unwrap_or(".");
            let sanitized_dir = sanitize_path(&input, current_dir)?;
            let mut command = tokio::process::Command::new("git");
            command.arg("apply");
            if check_only {
                command.arg("--check");
            }
            // Pipe patch via stdin to avoid Windows \\\\? long-path prefix issues
            command.arg("-");
            tracing::debug!(directory = %current_dir, check_only = %check_only, "tool: running git apply (async)");
            command.current_dir(&sanitized_dir);
            command.stdin(std::process::Stdio::piped());
            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());
            let mut child = command
                .spawn()
                .map_err(|e| anyhow::anyhow!("failed to spawn git apply: {}", e))?;
            if let Some(mut stdin) = child.stdin.take() {
                tokio::io::AsyncWriteExt::write_all(&mut stdin, patch.as_bytes()).await?;
            }
            let output = child.wait_with_output().await?;
            let success = output.status.success();
            if !success {
                tracing::warn!(
                    directory = %current_dir,
                    check_only = %check_only,
                    stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                    "tool: git apply failed"
                );
            }

            Ok(ToolOutput {
                success,
                result: Some(serde_json::json!({
                    "applied": success && !check_only,
                    "checked": check_only,
                    "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                    "exit_code": output.status.code(),
                })),
                error: (!success)
                    .then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
                verification: Some(
                    if check_only {
                        "patch_checked"
                    } else {
                        "patch_applied"
                    }
                    .to_string(),
                ),
                audit_log: Some(format!("git apply executed in '{}'", current_dir)),
                pua_report: Some(tool_execution_report(
                    "apply_patch",
                    Some(if check_only {
                        "patch_checked"
                    } else {
                        "patch_applied"
                    }),
                )),
            })
        })
    }
}

// ============================================================================
// RunTestsTool
// ============================================================================

/// Hardcoded allowlist of test commands for the `run_tests` tool.
///
/// Only commands in this list can be executed via the `run_tests` tool.
/// This prevents arbitrary command execution through the test runner.
/// To extend this list, modify `ALLOWED_TEST_COMMANDS` in this file.
const ALLOWED_TEST_COMMANDS: &[&str] = &[
    "cargo", "npm", "yarn", "pnpm", "make", "go", "python", "pytest", "mvn", "gradle", "git",
];

pub struct RunTestsTool;
impl Tool for RunTestsTool {
    fn name(&self) -> &'static str {
        "run_tests"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let command_name = input.payload["command"].as_str().unwrap_or("cargo");
        if !ALLOWED_TEST_COMMANDS.contains(&command_name) {
            let allowed = ALLOWED_TEST_COMMANDS.join(", ");
            anyhow::bail!(
                "{} — allowed commands: {}",
                tf("error.command_not_allowed", &[("command", command_name)]),
                allowed,
            );
        }
        let args = input.payload["args"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|text| text.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec!["test".to_string()]);
        // Validate arguments: only allow alphanumeric, `-`, `_`, `.`, `/`, `=`, and `--` prefixes
        for arg in &args {
            if !arg.chars().all(|c| {
                c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/' || c == '='
            }) {
                anyhow::bail!("Invalid test argument: '{}' — only alphanumeric, dashes, underscores, dots, slashes, and equals signs allowed", arg);
            }
            if arg.starts_with("--")
                && !arg[2..]
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '=')
            {
                anyhow::bail!("Invalid flag argument: '{}'", arg);
            }
        }
        let current_dir = sanitize_path(input, input.payload["directory"].as_str().unwrap_or("."))?;
        debug!(command = %command_name, args = ?args, directory = %current_dir.display(), "tool: running shell command");
        let output = Command::new(command_name)
            .args(&args)
            .current_dir(&current_dir)
            .output()?;
        let success = output.status.success();
        if !success {
            warn!(
                command = %command_name,
                exit_code = ?output.status.code(),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "tool: shell command failed"
            );
        }

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", success);
        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "command": command_name,
                "args": args,
                "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some("tests_passed".to_string()),
            audit_log: Some(format!(
                "Executed '{}' in '{}'",
                command_name,
                current_dir.display()
            )),
            pua_report: Some(tool_execution_report("run_tests", Some("tests_passed"))),
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let command_name = input.payload["command"].as_str().unwrap_or("cargo");
            if !ALLOWED_TEST_COMMANDS.contains(&command_name) {
                let allowed = ALLOWED_TEST_COMMANDS.join(", ");
                anyhow::bail!(
                    "{} — allowed commands: {}",
                    tf("error.command_not_allowed", &[("command", command_name)]),
                    allowed,
                );
            }
            let args = input.payload["args"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(|text| text.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec!["test".to_string()]);
            // Validate arguments
            for arg in &args {
                if !arg.chars().all(|c| {
                    c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/' || c == '='
                }) {
                    anyhow::bail!("Invalid test argument: '{}'", arg);
                }
                if arg.starts_with("--")
                    && !arg[2..]
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '=')
                {
                    anyhow::bail!("Invalid flag argument: '{}'", arg);
                }
            }
            let current_dir =
                sanitize_path(&input, input.payload["directory"].as_str().unwrap_or("."))?;
            let output = tokio::process::Command::new(command_name)
                .args(&args)
                .current_dir(&current_dir)
                .output()
                .await?;
            let success = output.status.success();
            if !success {
                tracing::warn!(
                    command = %command_name,
                    exit_code = ?output.status.code(),
                    stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                    "tool: shell command failed"
                );
            }
            Ok(ToolOutput {
                success,
                result: Some(serde_json::json!({
                    "command": command_name,
                    "args": args,
                    "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                    "exit_code": output.status.code(),
                })),
                error: (!success)
                    .then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
                verification: Some("tests_passed".to_string()),
                audit_log: Some(format!(
                    "Executed '{}' in '{}'",
                    command_name,
                    current_dir.display()
                )),
                pua_report: Some(tool_execution_report("run_tests", Some("tests_passed"))),
            })
        })
    }
}

// ============================================================================
// InspectGitDiffTool
// ============================================================================

pub struct InspectGitDiffTool;
impl Tool for InspectGitDiffTool {
    fn name(&self) -> &'static str {
        "inspect_git_diff"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let span = tracing::info_span!(
            "tool.run",
            tool = self.name(),
            input_size = serde_json::to_string(&input.payload)
                .unwrap_or_default()
                .len() as u64,
            latency_ms = 0u64,
            success = false,
        );
        let _guard = span.enter();
        let start = Instant::now();

        let current_dir = input.payload["directory"].as_str().unwrap_or(".");
        let sanitized_dir = sanitize_path(input, current_dir)?;
        let staged = input.payload["staged"].as_bool().unwrap_or(false);
        let files = input.payload["files"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(|text| text.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut command = Command::new("git");
        command.arg("diff").current_dir(&sanitized_dir);
        if staged {
            command.arg("--cached");
        }
        if !files.is_empty() {
            command.arg("--").args(&files);
        }
        let output = command.output()?;
        let success = output.status.success();

        let elapsed = start.elapsed().as_millis() as u64;
        span.record("latency_ms", elapsed);
        span.record("success", success);
        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "diff": String::from_utf8_lossy(&output.stdout).to_string(),
                "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                "exit_code": output.status.code(),
                "staged": staged,
                "files": files,
            })),
            error: (!success).then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
            verification: Some("diff_inspected".to_string()),
            audit_log: Some(format!("git diff inspected in '{}'", current_dir)),
            pua_report: Some(tool_execution_report(
                "inspect_git_diff",
                Some("diff_inspected"),
            )),
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            let current_dir = input.payload["directory"].as_str().unwrap_or(".");
            let sanitized_dir = sanitize_path(&input, current_dir)?;
            let staged = input.payload["staged"].as_bool().unwrap_or(false);
            let files = input.payload["files"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(|text| text.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let mut command = tokio::process::Command::new("git");
            command.arg("diff").current_dir(&sanitized_dir);
            if staged {
                command.arg("--cached");
            }
            if !files.is_empty() {
                command.arg("--").args(&files);
            }
            let output = command.output().await?;
            let success = output.status.success();

            Ok(ToolOutput {
                success,
                result: Some(serde_json::json!({
                    "diff": String::from_utf8_lossy(&output.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
                    "exit_code": output.status.code(),
                    "staged": staged,
                    "files": files,
                })),
                error: (!success)
                    .then(|| String::from_utf8_lossy(&output.stderr).trim().to_string()),
                verification: Some("diff_inspected".to_string()),
                audit_log: Some(format!("git diff inspected in '{}'", current_dir)),
                pua_report: Some(tool_execution_report(
                    "inspect_git_diff",
                    Some("diff_inspected"),
                )),
            })
        })
    }
}

// ============================================================================
// SkillListTool
// ============================================================================

/// Tool that lists all registered skills with their name, description, and score.
///
/// Requires a `SkillRegistry` to have been set via `set_skill_registry()`
/// before calling `run()`. Returns an empty list if no registry is configured.
///
/// Input payload: ignored (no arguments required).
/// Output: `{ "skills": [{ "name": "...", "description": "...", "score": 0.0 }, ...] }`
pub struct SkillListTool;

impl Tool for SkillListTool {
    fn name(&self) -> &'static str {
        "skill_list"
    }

    fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
        let skills = match SKILL_REGISTRY.get() {
            Some(registry) => match registry.read() {
                Ok(guard) => {
                    let descriptors = guard.list(false);
                    descriptors
                        .into_iter()
                        .map(|d| {
                            serde_json::json!({
                                "name": d.name,
                                "description": d.description,
                                "score": d.score,
                                "input_schema": d.input_schema,
                                "total_calls": d.total_calls,
                                "success_calls": d.success_calls,
                                "failure_calls": d.failure_calls,
                                "average_latency_ms": d.average_latency_ms,
                            })
                        })
                        .collect::<Vec<_>>()
                }
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        };

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"skills": skills})),
            error: None,
            verification: Some("skills_listed".to_string()),
            audit_log: Some(format!("Listed {} skill(s)", skills.len())),
            pua_report: Some(tool_execution_report("skill_list", Some("skills_listed"))),
        })
    }
}

// ============================================================================
// SkillExecuteTool
// ============================================================================

/// Tool that executes a registered skill by name with provided input.
///
/// Requires a `SkillRegistry` to have been set via `set_skill_registry()`.
/// Returns an error if no registry or the skill is not found.
pub struct SkillExecuteTool;

/// Shared static Arc for SkillExecuteTool — avoids allocating a new Arc on every call.
static SKILL_EXECUTE_TOOL: std::sync::OnceLock<std::sync::Arc<SkillExecuteTool>> =
    std::sync::OnceLock::new();

fn skill_execute_arc() -> std::sync::Arc<SkillExecuteTool> {
    SKILL_EXECUTE_TOOL
        .get_or_init(|| std::sync::Arc::new(SkillExecuteTool))
        .clone()
}

impl Tool for SkillExecuteTool {
    fn name(&self) -> &'static str {
        "skill_execute"
    }

    /// Async execution: look up the skill from the registry (via spawn_blocking to
    /// avoid holding the async runtime on a RwLock read), then await
    /// `skill.execute(...)` directly. This avoids violating principle #23
    /// (no block_in_place + block_on in hot paths).
    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            // ── Step 1: Extract skill name from payload ──
            let payload = &input.payload;
            let skill_name = payload
                .get("skill_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("skill_execute requires 'skill_name' argument"))?
                .to_string();
            let skill_input = payload
                .get("input")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            // ── Step 2: Look up skill in registry (async-safe via spawn_blocking) ──
            let skill = match SKILL_REGISTRY.get() {
                Some(registry) => {
                    let registry = Arc::clone(registry);
                    let skill_name = skill_name.clone();
                    let skill_input_val = skill_input.clone();
                    tokio::task::spawn_blocking(move || {
                        let guard = registry
                            .read()
                            .map_err(|e| anyhow::anyhow!("skill registry lock failed: {}", e))?;
                        // Try exact match first, then fuzzy match
                        guard
                            .get(&skill_name)
                            .or_else(|| {
                                let fuzzy =
                                    guard.best_match_with_input(&skill_name, &skill_input_val)?;
                                tracing::info!(
                                    "skill_execute: fuzzy-matched '{}' -> '{}'",
                                    skill_name,
                                    fuzzy
                                );
                                guard.get(&fuzzy)
                            })
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "skill '{}' not found in registry (no fuzzy match either). \
                                     Use 'skill_list' tool first to see available skills.",
                                    skill_name
                                )
                            })
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("skill registry lock task failed: {}", e))??
                }
                None => {
                    return Ok(ToolOutput {
                        success: false,
                        result: None,
                        error: Some(
                            "no skill registry configured — call set_skill_registry() first"
                                .to_string(),
                        ),
                        verification: None,
                        audit_log: None,
                        pua_report: None,
                    });
                }
            };

            // ── Step 3: Execute the skill (truly async) ──
            let exec_start = Instant::now();
            let result = skill.execute(&skill_input).await;
            let exec_elapsed = exec_start.elapsed();

            let outcome_success = result.is_ok();
            if let Some(registry) = SKILL_REGISTRY.get() {
                let registry = Arc::clone(registry);
                let s_name = skill_name.clone();
                tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = registry.write() {
                        guard.record_outcome(&s_name, outcome_success, exec_elapsed);
                    }
                })
                .await
                .ok();
            }

            match result {
                Ok(value) => Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "skill": skill_name,
                        "output": value,
                    })),
                    error: None,
                    verification: Some("skill_executed".to_string()),
                    audit_log: Some(format!("Executed skill '{}'", skill_name)),
                    pua_report: Some(tool_execution_report(
                        "skill_execute",
                        Some("skill_executed"),
                    )),
                }),
                Err(e) => Ok(ToolOutput {
                    success: false,
                    result: None,
                    error: Some(format!("skill '{}' execution failed: {}", skill_name, e)),
                    verification: None,
                    audit_log: None,
                    pua_report: None,
                }),
            }
        })
    }

    /// Sync fallback: bridges to `run_async` via the dedicated skill runtime.
    ///
    /// Always uses the dedicated blocking runtime to avoid `block_in_place` + `block_on`
    /// on hot paths (principle #23). Async callers should always use `run_async`
    /// directly for optimal non-blocking execution.
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let input = input.clone();
        // Run on the shared dedicated blocking runtime so we never `block_on`
        // on an async worker (see `exec_common::blocking_runtime()`). The
        // guard serializes concurrent sync `run()` calls on the shared
        // current-thread runtime.
        crate::orchestration::tool::exec_common::with_blocking_runtime(|rt| {
            rt.block_on(skill_execute_arc().run_async(input))
        })
    }
}

// ============================================================================
// SkillCreateTool
// ============================================================================

/// Tool that creates a new skill from a prompt template.
///
/// Bridges to the existing `SkillCreatorSkill` in the skill execution system.
/// Requires a `SkillRegistry` to have been set via `set_skill_registry()`.
pub struct SkillCreateTool;

impl Tool for SkillCreateTool {
    fn name(&self) -> &'static str {
        "skill_create"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let payload = &input.payload;
        let name = payload
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("skill_create requires 'name' argument"))?;
        let description = payload
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("skill_create requires 'description' argument"))?;
        let prompt_template = payload
            .get("prompt_template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("skill_create requires 'prompt_template' argument"))?;
        // Parse optional input_schema from JSON Value into HashMap<String, String>
        let input_schema: std::collections::HashMap<String, String> = payload
            .get("input_schema")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        let registry = SKILL_REGISTRY.get().ok_or_else(|| {
            anyhow::anyhow!("no skill registry configured — call set_skill_registry() first")
        })?;
        let mut guard = registry
            .write()
            .map_err(|e| anyhow::anyhow!("skill registry lock failed: {}", e))?;

        guard
            .create_skill_from_prompt(name, description, prompt_template, input_schema)
            .map_err(|e| anyhow::anyhow!("failed to create skill: {}", e))?;

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "skill": name,
                "description": description,
            })),
            error: None,
            verification: Some("skill_created".to_string()),
            audit_log: Some(format!("Created skill '{}': {}", name, description)),
            pua_report: Some(tool_execution_report("skill_create", Some("skill_created"))),
        })
    }
}

// ============================================================================
// SkillReloadTool
// ============================================================================

/// Tool that triggers an immediate reload of skills from the local skills directory.
///
/// Without this tool, AI agents would need to wait up to 60s for the background
/// refresh task.  This is the instant version.
pub struct SkillReloadTool;

impl Tool for SkillReloadTool {
    fn name(&self) -> &'static str {
        "skill_reload"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let registry = SKILL_REGISTRY.get().ok_or_else(|| {
            anyhow::anyhow!("no skill registry configured — call set_skill_registry() first")
        })?;

        let custom_dir = input.payload.get("directory").and_then(|v| v.as_str());
        let agents_skills_dir = custom_dir.map(std::path::PathBuf::from);

        let mut guard = registry
            .write()
            .map_err(|e| anyhow::anyhow!("skill registry lock failed: {}", e))?;

        let summary = guard
            .discover_and_register_local_skills(agents_skills_dir.as_deref())
            .map_err(|e| anyhow::anyhow!("skill reload failed: {}", e))?;

        let total = guard.list(false).len();

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "registered": summary.registered,
                "skipped": summary.skipped,
                "errors": summary.errors,
                "total_skills": total,
            })),
            error: None,
            verification: Some("skills_reloaded".to_string()),
            audit_log: Some(format!(
                "Skill reload: {} new, {} skipped, {} errors ({} total)",
                summary.registered,
                summary.skipped,
                summary.errors.len(),
                total
            )),
            pua_report: Some(tool_execution_report(
                "skill_reload",
                Some("skills_reloaded"),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_input(base: Option<PathBuf>) -> ToolInput {
        ToolInput {
            task_id: "test".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({}),
            allowed_base_dir: base,
        }
    }

    #[test]
    fn sanitize_path_for_write_rejects_unresolved_parent_dir_components() {
        // Regression: the canonicalize fallback rejoins raw path components,
        // and `Path::starts_with` does NOT normalize `..` — a path like
        // `<base>/../../x/y.txt` (with a missing intermediate dir) would pass
        // the base check and escape on write.
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir_all(&base).unwrap();

        let evil = base.join("..").join("..").join("escaped").join("x.txt");
        let err =
            sanitize_path_for_write(&write_input(Some(base.clone())), &evil.to_string_lossy())
                .unwrap_err();
        assert!(
            err.to_string().contains("denied") || err.to_string().contains(".."),
            ".. traversal must be rejected, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sanitize_path_for_write_rejects_dangling_symlink_leaf() {
        // Regression: writing through a dangling symlink would land outside
        // the base dir; the leaf must be rejected.
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir_all(&base).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink("/tmp/definitely-not-a-target-xyz", &link).unwrap();

        let err =
            sanitize_path_for_write(&write_input(Some(base.clone())), &link.to_string_lossy())
                .unwrap_err();
        assert!(
            err.to_string().contains("symbolic") || err.to_string().contains("denied"),
            "dangling symlink leaf must be rejected, got: {err}"
        );
    }

    #[test]
    fn sanitize_path_for_write_allows_existing_paths_inside_base() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("sub").join("f.txt"), "x").unwrap();

        let target = base.join("sub").join("f.txt");
        let ok =
            sanitize_path_for_write(&write_input(Some(base.clone())), &target.to_string_lossy())
                .unwrap();
        assert!(ok.starts_with(&base));
    }
}
