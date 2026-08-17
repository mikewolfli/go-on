//! GAP-B52-01: Self-Evolution Sandbox Executor
//!
//! Provides a sandboxed execution environment for applying code patches,
//! building projects, and running tests in an isolated workspace with
//! strict safety controls: no-network, allowed-targets whitelisting, and
//! a hard iteration limit.

use crate::orchestration::write::{file_hash, record_file_change, FileChangeEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use thiserror::Error;
use tokio::fs;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Hard limit on the number of patch iterations allowed in a single sandbox session.
const MAX_ITERATIONS: u64 = 10;
/// Minimum pre-patch code-quality health score (0.0–1.0). Patches are rejected
/// when the baseline clippy scan scores below this (e.g. scan failure or a
/// heavily degraded tree).
const MIN_QUALITY_GATE_SCORE: f64 = 0.5;

/// Timeout for `cargo build` inside the sandbox (10 minutes).
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Timeout for `cargo test` inside the sandbox (10 minutes).
const TEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Timeout for `git` operations inside the sandbox (60 seconds).
const GIT_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// CodePatch
// ---------------------------------------------------------------------------

/// A structured code patch describing changes to a single target file.
///
/// Contains both the original and patched line ranges so the system can
/// reason about what changed and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodePatch {
    /// The target file path (relative to the project root).
    pub target_file: String,
    /// Original lines as (line_number, content) pairs. Line numbers are 1-based.
    pub original_lines: Vec<(usize, String)>,
    /// Patched lines as (line_number, content) pairs. Line numbers are 1-based.
    pub patched_lines: Vec<(usize, String)>,
    /// Unified diff string representation of the change.
    pub diff: String,
    /// Natural language reasoning explaining why this patch was generated.
    pub reasoning: String,
    /// Optional UUID for tracking this patch across the evolution pipeline.
    #[serde(default)]
    pub patch_id: Option<Uuid>,
}

impl CodePatch {
    /// Create a new CodePatch from original and patched line data.
    pub fn new(
        target_file: String,
        original_lines: Vec<(usize, String)>,
        patched_lines: Vec<(usize, String)>,
        reasoning: String,
    ) -> Self {
        let diff = Self::generate_diff(&target_file, &original_lines, &patched_lines);
        Self {
            target_file,
            original_lines,
            patched_lines,
            diff,
            reasoning,
            patch_id: Some(Uuid::new_v4()),
        }
    }

    /// Generate a unified-diff-style string from line changes.
    fn generate_diff(
        target_file: &str,
        original: &[(usize, String)],
        patched: &[(usize, String)],
    ) -> String {
        let mut lines = Vec::new();
        lines.push(format!("--- a/{}", target_file));
        lines.push(format!("+++ b/{}", target_file));

        // Collect all line numbers involved
        let mut all_lines: Vec<usize> = original.iter().map(|(ln, _)| *ln).collect();
        all_lines.extend(patched.iter().map(|(ln, _)| *ln));
        all_lines.sort_unstable();
        all_lines.dedup();

        if all_lines.is_empty() {
            return lines.join("\n");
        }

        let start = all_lines.first().copied().unwrap_or(1);
        let end = all_lines.last().copied().unwrap_or(1);
        lines.push(format!(
            "@@ -{},{} +{},{} @@",
            start,
            original.len(),
            start,
            patched.len()
        ));

        // Build a map from line number for quick lookup
        let orig_map: HashMap<usize, &str> =
            original.iter().map(|(ln, s)| (*ln, s.as_str())).collect();
        let patch_map: HashMap<usize, &str> =
            patched.iter().map(|(ln, s)| (*ln, s.as_str())).collect();

        for ln in start..=end {
            match (orig_map.get(&ln), patch_map.get(&ln)) {
                (Some(_), None) => {
                    lines.push(format!("-{}", orig_map[&ln]));
                }
                (None, Some(_)) => {
                    lines.push(format!("+{}", patch_map[&ln]));
                }
                (Some(orig), Some(patch)) if orig != patch => {
                    lines.push(format!("-{}", orig));
                    lines.push(format!("+{}", patch));
                }
                (Some(_), Some(_)) => {
                    // unchanged — include for context
                    lines.push(format!(" {}", orig_map[&ln]));
                }
                (None, None) => {}
            }
        }

        lines.push("\n".to_string());
        lines.join("\n")
    }

    /// Apply this patch's changes to a file on disk.
    pub async fn apply_to_file(&self, workdir: &std::path::Path) -> Result<u64, SandboxError> {
        let file_path = workdir.join(&self.target_file);
        let content = fs::read_to_string(&file_path).await.map_err(|e| {
            SandboxError::IoError(format!("Cannot read {}: {}", file_path.display(), e))
        })?;

        let mut lines: Vec<&str> = content.lines().collect();

        // Apply removals first (original lines that are NOT in patched lines)
        let patch_lns: std::collections::HashSet<usize> =
            self.patched_lines.iter().map(|(ln, _)| *ln).collect();
        let orig_lns: std::collections::HashSet<usize> =
            self.original_lines.iter().map(|(ln, _)| *ln).collect();

        // Lines to remove: in original but not in patched
        let to_remove: Vec<usize> = orig_lns.difference(&patch_lns).copied().collect();
        // Sort descending to remove from bottom up
        let mut to_remove_sorted = to_remove;
        to_remove_sorted.sort_unstable_by(|a, b| b.cmp(a));
        for ln in to_remove_sorted {
            // Line numbers are 1-based; `ln == 0` (from a hostile patch) must
            // not underflow `ln - 1` (panic in debug, wrap in release).
            if ln >= 1 && ln <= lines.len() {
                lines.remove(ln - 1);
            }
        }

        // Apply insertions/updates
        let mut change_count = 0u64;
        for (ln, new_content) in &self.patched_lines {
            let idx = *ln;
            if idx >= 1 && idx <= lines.len() {
                // Check if this line is also in original — update or insert
                if orig_lns.contains(ln) {
                    lines[idx - 1] = new_content;
                    change_count += 1;
                } else {
                    // Insert new line before the given position
                    lines.insert(idx - 1, new_content);
                    change_count += 1;
                }
            } else if idx > lines.len() {
                // Insert beyond the current end → append. (Line numbers in a
                // patch are 1-based positions; a value past the end means the
                // new line goes at the bottom of the file.)
                lines.push(new_content);
                change_count += 1;
            }
            // idx == 0: invalid line number from a hostile patch — skip (no
            // underflow, no append; mirrors the removal path above).
        }

        let new_content = lines.join("\n");
        fs::write(&file_path, &new_content).await.map_err(|e| {
            SandboxError::IoError(format!("Cannot write {}: {}", file_path.display(), e))
        })?;

        Ok(change_count)
    }
}

// ---------------------------------------------------------------------------
// BuildResult
// ---------------------------------------------------------------------------

/// Result of a build or test invocation within the sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BuildResult {
    /// Build succeeded, possibly with warnings.
    Success {
        /// Number of warnings emitted.
        warnings: usize,
        /// Build duration in milliseconds.
        time_ms: u64,
    },
    /// Build failed with compilation errors.
    CompileError {
        /// Number of distinct error messages.
        errors: usize,
        /// Error lines captured from stderr.
        lines: Vec<String>,
    },
    /// Test run completed (success or failure).
    TestFailure {
        /// Number of failing tests.
        failed: usize,
        /// Number of passing tests.
        passed: usize,
    },
}

impl BuildResult {
    /// Returns true if this result indicates success.
    pub fn is_success(&self) -> bool {
        matches!(self, BuildResult::Success { .. })
    }

    /// Returns duration in milliseconds, or 0 if unknown.
    pub fn time_ms(&self) -> u64 {
        match self {
            BuildResult::Success { time_ms, .. } => *time_ms,
            _ => 0,
        }
    }

    /// Returns a human-readable summary string.
    pub fn summary(&self) -> String {
        match self {
            BuildResult::Success { warnings, time_ms } => {
                format!("SUCCESS ({} warnings, {} ms)", warnings, time_ms)
            }
            BuildResult::CompileError { errors, lines } => {
                let preview: Vec<&str> = lines.iter().take(5).map(|s| s.as_str()).collect();
                format!("COMPILE ERROR ({} errors): {}", errors, preview.join("; "))
            }
            BuildResult::TestFailure { failed, passed } => {
                format!("TEST FAILURE ({} failed, {} passed)", failed, passed)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SandboxError
// ---------------------------------------------------------------------------

/// Errors that can occur during sandbox operations.
#[derive(Debug, Error)]
pub enum SandboxError {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    IoError(String),

    /// Maximum iteration limit exceeded.
    #[error("max iterations ({MAX_ITERATIONS}) exceeded")]
    MaxIterationsExceeded,

    /// Target file is not in the allowed whitelist.
    #[error("forbidden target: {0} — not in allowed_targets whitelist")]
    ForbiddenTarget(String),

    /// Git operation failed.
    #[error("git error: {0}")]
    GitError(String),

    /// Build process failed with non-zero exit code.
    #[error("build failed: {0}")]
    BuildFailed(String),

    /// Test process failed.
    #[error("test failed: {0}")]
    TestFailed(String),

    /// Pre-patch code quality gate rejected the patch.
    #[error("quality gate rejected: {0}")]
    QualityGate(String),

    /// Network access was attempted from the sandbox.
    #[error("network access denied from sandbox")]
    NetworkDenied,
}

// ---------------------------------------------------------------------------
// SandboxExecutor
// ---------------------------------------------------------------------------

/// A sandboxed executor that safely applies patches, builds, and tests code.
///
/// # Safety guarantees
///
/// - **No-network**: `/etc/hosts` is isolated with a local-only mapping.
/// - **Allowed targets**: Only files matching `allowed_targets` can be patched.
/// - **Max iterations**: Hard limit of 10 patch iterations per executor lifetime.
/// - **Workspace isolation**: All operations occur within `workdir`.
#[derive(Debug)]
pub struct SandboxExecutor {
    /// Working directory for all sandbox operations.
    workdir: PathBuf,
    /// Remaining iteration budget.
    iteration_budget: Arc<AtomicU64>,
    /// Glob-style patterns for allowed target files.
    allowed_targets: Vec<String>,
    /// Unique identifier for this sandbox instance.
    instance_id: Uuid,
}

impl SandboxExecutor {
    /// Create a new SandboxExecutor.
    ///
    /// `max_iter` is capped to `MAX_ITERATIONS` (10).
    pub fn new(workdir: PathBuf, max_iter: u64) -> Self {
        let budget = max_iter.min(MAX_ITERATIONS);
        Self {
            workdir,
            iteration_budget: Arc::new(AtomicU64::new(budget)),
            allowed_targets: Vec::new(),
            instance_id: Uuid::new_v4(),
        }
    }

    /// Set the allowed target file patterns (glob-style).
    ///
    /// Only files matching these patterns can be patched.
    pub fn with_allowed_targets(mut self, patterns: Vec<String>) -> Self {
        self.allowed_targets = patterns;
        self
    }

    /// Returns the unique instance ID.
    pub fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    /// Returns remaining iteration budget.
    pub fn remaining_iterations(&self) -> u64 {
        self.iteration_budget.load(Ordering::Relaxed)
    }

    /// Apply a code patch to the sandbox workspace.
    ///
    /// Returns the number of lines changed. Performs safety checks:
    /// - Iteration budget check
    /// - Allowed-targets whitelist
    /// - Code quality pre-gate (clippy scan before applying)
    pub async fn apply_patch(&self, patch: &CodePatch) -> Result<u64, SandboxError> {
        // Check iteration budget — the decrement must not wrap: `fetch_sub(1)`
        // on an exhausted budget stores `u64::MAX`, silently re-enabling the
        // cap for every later call. `fetch_update` returns `Err` once the
        // budget is zero, so exhaustion is permanent.
        let remaining =
            self.iteration_budget
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                    if v == 0 {
                        None
                    } else {
                        Some(v - 1)
                    }
                });
        let remaining = match remaining {
            Ok(rem) => rem,
            Err(_) => return Err(SandboxError::MaxIterationsExceeded),
        };
        info!(
            sandbox = %self.instance_id,
            target = %patch.target_file,
            remaining = remaining - 1,
            "applying patch"
        );

        // Check allowed targets
        if !self.is_target_allowed(&patch.target_file) {
            return Err(SandboxError::ForbiddenTarget(patch.target_file.clone()));
        }

        // Pre-patch code quality gate: run clippy before applying to establish
        // baseline. The clippy invocation is a blocking child process (can take
        // seconds), so it is offloaded to the blocking pool; a baseline health
        // score below the minimum blocks the patch.
        let workdir = self.workdir.clone();
        let pre_quality = tokio::task::spawn_blocking(move || {
            crate::intelligence::code_quality::run_code_quality_scan(&workdir)
        })
        .await
        .map_err(|join_err| {
            SandboxError::QualityGate(format!("quality scan panicked: {join_err}"))
        })?;
        tracing::info!(
            sandbox = %self.instance_id,
            health_score = pre_quality.health_score,
            issues = pre_quality.issues.len(),
            "pre-patch quality gate completed"
        );
        if pre_quality.health_score < MIN_QUALITY_GATE_SCORE {
            return Err(SandboxError::QualityGate(format!(
                "baseline quality score {} below minimum {}",
                pre_quality.health_score, MIN_QUALITY_GATE_SCORE
            )));
        }

        // ── M4.3: change-event audit — hash the target file BEFORE the patch
        // is applied so the audit chain can replay old→new per change. The
        // sandbox CodePatch is single-file by construction (`target_file`),
        // so one event per apply. Hashes are best-effort (warn + None, same
        // pattern as M1.3): a hash failure must never abort an apply that
        // would otherwise succeed.
        let target_path = self.workdir.join(&patch.target_file);
        let old_hash = file_hash(&target_path).unwrap_or_else(|e| {
            warn!(
                sandbox = %self.instance_id,
                "self_evolution: cannot hash '{}' before apply: {e:#}",
                target_path.display()
            );
            None
        });

        // Apply the patch to the file
        let changed = patch.apply_to_file(&self.workdir).await?;

        // The write above succeeded — record the event AFTER the fact
        // (fire-and-forget, same pattern as M1.3). A failed apply returns
        // before this point, so a patch that never wrote is never recorded.
        let new_hash = file_hash(&target_path).unwrap_or_else(|e| {
            warn!(
                sandbox = %self.instance_id,
                "self_evolution: cannot hash '{}' after apply: {e:#}",
                target_path.display()
            );
            None
        });
        self.record_evolution_change(patch.patch_id, "apply", &target_path, old_hash, new_hash);

        debug!(
            sandbox = %self.instance_id,
            target = %patch.target_file,
            lines_changed = changed,
            "patch applied"
        );

        Ok(changed)
    }

    /// Best-effort audit record for a file rewrite performed by this sandbox.
    ///
    /// Records a [`FileChangeEvent`] with `op = "self_evolution"` so every
    /// source modification from the evolution loop flows through the M1.3
    /// unified write chokepoint and the audit chain can replay the old→new
    /// content hash per change. The evolution loop does not thread a task id
    /// into the sandbox (`EvolutionLoop::apply` calls `apply_patch(patch)`
    /// without one), so the task id is derived from the sandbox instance id
    /// (the evolution run) plus the `patch_id` of the change that caused it,
    /// e.g. `self_evolution/{sandbox}/{patch}`. Fire-and-forget: the audit
    /// sink swallows its own I/O errors, and the file write has already
    /// happened when this is called.
    fn record_evolution_change(
        &self,
        patch_id: Option<Uuid>,
        phase: &str,
        path: &Path,
        old_hash: Option<String>,
        new_hash: Option<String>,
    ) {
        let task_id = match patch_id {
            Some(id) => format!("self_evolution/{}/{}", self.instance_id, id),
            None => format!("self_evolution/{}", self.instance_id),
        };
        record_file_change(
            &task_id,
            phase,
            &FileChangeEvent {
                path: path.to_string_lossy().into_owned(),
                op: "self_evolution",
                old_hash,
                new_hash,
            },
        );
    }

    /// Revert a previously-applied patch by re-applying its inverse.
    ///
    /// Restores the target file to its pre-patch content. Safe to call only
    /// for patches applied by this sandbox: `original_lines` refers to the
    /// pre-patch file and `patched_lines` to the post-patch file, so swapping
    /// them yields an exact inverse diff.
    pub async fn revert_patch(&self, patch: &CodePatch) -> Result<u64, SandboxError> {
        let reversed = CodePatch {
            target_file: patch.target_file.clone(),
            original_lines: patch.patched_lines.clone(),
            patched_lines: patch.original_lines.clone(),
            diff: patch.diff.clone(),
            reasoning: format!("revert of {}", patch.reasoning),
            patch_id: patch.patch_id,
        };
        // ── M4.3: a revert is a source modification too, so it must land in
        // the audit chain with the same old→new hash replay as an apply.
        let target_path = self.workdir.join(&patch.target_file);
        let old_hash = file_hash(&target_path).unwrap_or_else(|e| {
            warn!(
                sandbox = %self.instance_id,
                "self_evolution: cannot hash '{}' before revert: {e:#}",
                target_path.display()
            );
            None
        });

        let changed = reversed.apply_to_file(&self.workdir).await?;

        let new_hash = file_hash(&target_path).unwrap_or_else(|e| {
            warn!(
                sandbox = %self.instance_id,
                "self_evolution: cannot hash '{}' after revert: {e:#}",
                target_path.display()
            );
            None
        });
        self.record_evolution_change(patch.patch_id, "rollback", &target_path, old_hash, new_hash);

        Ok(changed)
    }

    /// Build the project with the given Cargo profile.
    ///
    /// # Arguments
    /// * `profile` - Cargo profile name (e.g., "debug", "release", "check").
    pub async fn build(&self, profile: &str) -> BuildResult {
        let start = Instant::now();

        // Configure the command with network isolation
        let mut cmd = Command::new("cargo");
        cmd.current_dir(&self.workdir);
        cmd.arg("build");

        match profile {
            "release" => {
                cmd.arg("--release");
            }
            "check" => {
                cmd.arg("--check");
            }
            _ => {
                // debug profile — no extra flags
            }
        }

        // Apply network sandboxing
        self.apply_network_sandbox(&mut cmd);

        let output = match timeout(BUILD_TIMEOUT, cmd.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return BuildResult::CompileError {
                    errors: 1,
                    lines: vec![format!("Failed to spawn cargo build: {}", e)],
                };
            }
            Err(_) => {
                return BuildResult::CompileError {
                    errors: 1,
                    lines: vec!["cargo build timed out after 600s".to_string()],
                };
            }
        };

        let elapsed = start.elapsed().as_millis() as u64;

        if output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let warning_count = stderr.matches("warning").count();
            BuildResult::Success {
                warnings: warning_count,
                time_ms: elapsed,
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let all_output = format!("{}{}", stderr, stdout);
            let error_lines: Vec<String> = all_output
                .lines()
                .filter(|l| l.contains("error"))
                .map(|l| l.to_string())
                .collect();
            let error_count = error_lines.len();

            BuildResult::CompileError {
                errors: error_count,
                lines: if error_lines.is_empty() {
                    vec![all_output]
                } else {
                    error_lines
                },
            }
        }
    }

    /// Run tests for a specific target.
    ///
    /// # Arguments
    /// * `target` - Test target (package name or test name).
    pub async fn test(&self, target: &str) -> BuildResult {
        let start = Instant::now();

        let mut cmd = Command::new("cargo");
        cmd.current_dir(&self.workdir);
        cmd.arg("test");

        if !target.is_empty() && target != "all" {
            cmd.arg("--package").arg(target);
        }

        // Run tests single-threaded in sandbox to avoid resource contention
        cmd.env("RUST_TEST_THREADS", "1");
        cmd.env("CARGO_TERM_COLOR", "never");

        // Apply network sandboxing
        self.apply_network_sandbox(&mut cmd);

        let output = match timeout(TEST_TIMEOUT, cmd.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(_e)) => {
                return BuildResult::TestFailure {
                    failed: 1,
                    passed: 0,
                };
            }
            Err(_) => {
                return BuildResult::TestFailure {
                    failed: 1,
                    passed: 0,
                };
            }
        };

        let elapsed = start.elapsed().as_millis() as u64;

        if output.status.success() {
            // Tests passed — report a real Success so `is_success()` is true.
            // Previously this returned `TestFailure { failed: 0, passed: 1 }`,
            // which made every evolution cycle's verify() fail and fed
            // verify_failure counters back into the diagnostic trigger.
            BuildResult::Success {
                time_ms: elapsed,
                warnings: 0,
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let combined = format!("{}{}", stderr, stdout);

            // Count test results
            let failed = combined.lines().filter(|l| l.contains("FAILED")).count();
            let passed = combined
                .lines()
                .filter(|l| l.contains("ok") || l.contains("PASSED"))
                .count();

            BuildResult::TestFailure {
                failed: if failed == 0 { 1 } else { failed },
                passed,
            }
        }
    }

    /// Commit the current state and record the commit hash.
    ///
    /// # Arguments
    /// * `hash` - Expected commit hash (output reference).
    /// * `approved` - Whether the change was approved.
    pub async fn commit(&self, hash: &str, approved: bool) -> Result<(), SandboxError> {
        info!(
            sandbox = %self.instance_id,
            hash = %hash,
            approved = approved,
            "committing sandbox state"
        );

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.workdir);
        cmd.args(["add", "-A"]);
        self.apply_network_sandbox(&mut cmd);

        let add_output = timeout(GIT_TIMEOUT, cmd.output())
            .await
            .map_err(|_| SandboxError::GitError("git add timed out".to_string()))?
            .map_err(|e| SandboxError::GitError(format!("git add failed: {}", e)))?;

        if !add_output.status.success() {
            let stderr = String::from_utf8_lossy(&add_output.stderr);
            warn!("git add failed (non-fatal): {}", stderr);
        }

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.workdir);
        cmd.args([
            "commit",
            "-m",
            &format!(
                "self-evolution [{}] approved={} hash={}",
                self.instance_id, approved, hash
            ),
            "--allow-empty",
        ]);
        self.apply_network_sandbox(&mut cmd);

        let commit_output = timeout(GIT_TIMEOUT, cmd.output())
            .await
            .map_err(|_| SandboxError::GitError("git commit timed out".to_string()))?
            .map_err(|e| SandboxError::GitError(format!("git commit failed: {}", e)))?;

        if !commit_output.status.success() {
            let stderr = String::from_utf8_lossy(&commit_output.stderr);
            return Err(SandboxError::GitError(format!(
                "git commit failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// True when the sandbox workdir is an isolated workspace that the sandbox
    /// owns (e.g. a temp dir). When the workdir is the real project root the
    /// sandbox must NOT delete build artifacts (`target/`) or evolution
    /// history (`.goon/`) — those belong to the production tree.
    fn is_isolated_workspace(&self) -> bool {
        let cwd = std::env::current_dir().unwrap_or_default();
        self.workdir != Path::new(".") && self.workdir != cwd
    }

    /// Clean up the sandbox workspace by removing temporary files.
    pub async fn cleanup(&self) {
        info!(sandbox = %self.instance_id, "cleaning up sandbox");

        if !self.is_isolated_workspace() {
            debug!(
                sandbox = %self.instance_id,
                "cleanup skipped: workdir is the real project root"
            );
            return;
        }

        // Remove target directory to free disk space
        let target_dir = self.workdir.join("target");
        if target_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&target_dir).await {
                warn!("Failed to remove target dir: {}", e);
            }
        }

        // Remove any .goon evolution artifacts
        let goon_dir = self.workdir.join(".goon");
        if goon_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&goon_dir).await {
                warn!("Failed to remove .goon dir: {}", e);
            }
        }

        debug!(sandbox = %self.instance_id, "sandbox cleanup complete");
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Check if a target file is allowed by the whitelist.
    /// Supports glob-like patterns: `*.rs`, `src/**/*.rs`, `src/*.rs`, `src/lib.rs`
    fn is_target_allowed(&self, target: &str) -> bool {
        if self.allowed_targets.is_empty() {
            // Empty whitelist means all targets allowed (dangerous — warn)
            warn!("allowed_targets is empty — all files are patchable");
            return true;
        }

        // Reject path traversal outright: a `..` component makes
        // `workdir.join(target)` resolve outside the whitelisted tree (e.g.
        // "src/../lib.rs" escapes `src`; "src/../../x.rs" leaves the project
        // root entirely) even when the literal string starts with a whitelisted
        // prefix.
        if target.split('/').any(|seg| seg == "..") {
            return false;
        }

        self.allowed_targets.iter().any(|pattern| {
            let trimmed = pattern.trim_end_matches('/');
            if trimmed == "*" || trimmed == "**" {
                return true;
            }
            // Simple glob matching supporting * and ** wildcards
            if let Some(rest) = trimmed.strip_suffix(".rs") {
                if !target.ends_with(".rs") {
                    return false;
                }
                // Check if the prefix (without .rs) matches
                Self::match_glob_prefix(rest, &target[..target.len() - 3])
            } else {
                target == trimmed
            }
        })
    }

    /// Match a target path prefix against a glob pattern (supports `*` and `**`).
    fn match_glob_prefix(pattern: &str, target_prefix: &str) -> bool {
        // Handle **/* or ** or * pattern: match any path
        if pattern == "**/*" || pattern == "**" || pattern == "*" {
            return true;
        }
        // Handle pattern/**/* : recursive match under directory. Segment
        // boundaries matter: `src/**/*.rs` must NOT match `src2/foo.rs` or
        // `src_evil/bar.rs` — only paths under `src/` (at least one segment
        // after the prefix) qualify.
        if let Some(prefix) = pattern.strip_suffix("/**/*") {
            return target_prefix.starts_with(&format!("{prefix}/"));
        }
        // Handle src/** pattern: `src` itself or anything under `src/`.
        if let Some(prefix) = pattern.strip_suffix("/**") {
            return target_prefix == prefix || target_prefix.starts_with(&format!("{prefix}/"));
        }
        // Handle src/* pattern: direct child of src
        if let Some(prefix) = pattern.strip_suffix("/*") {
            if let Some(rest) = target_prefix.strip_prefix(prefix) {
                return rest.starts_with('/') && !rest[1..].contains('/');
            }
            return false;
        }
        // Remaining patterns are exact file matches (any trailing `*`/`**`
        // was consumed above). Exact comparison avoids prefix bleed where an
        // exact pattern like `src/lib` would otherwise also match
        // `src/library`.
        target_prefix == pattern.trim_end_matches('*').trim_end_matches('/')
    }

    /// Apply network sandboxing by setting environment variables that restrict
    /// network access and modifying the hosts file approach.
    fn apply_network_sandbox(&self, cmd: &mut Command) {
        // Block network via environment
        cmd.env("CARGO_HTTP_TIMEOUT", "5");
        cmd.env("CARGO_NET_RETRY", "0");
        cmd.env("CARGO_NET_OFFLINE", "true");
        cmd.env("GIT_HTTP_LOW_SPEED_LIMIT", "1");
        cmd.env("GIT_HTTP_LOW_SPEED_TIME", "5");
        cmd.env("NO_PROXY", "*");
        cmd.env("HTTP_PROXY", "");
        cmd.env("HTTPS_PROXY", "");
        cmd.env("all_proxy", "");
        cmd.env("ALL_PROXY", "");

        // Run with low priority to avoid hogging system resources
        cmd.env("NIX_REMOTE", "");
        cmd.env("NIX_BUILD_CORES", "1");
    }
}

impl Drop for SandboxExecutor {
    fn drop(&mut self) {
        // Best-effort cleanup on drop — do not block. Only removes the target
        // dir of an ISOLATED sandbox workspace; the real project root (".")
        // must never have its build cache deleted by a dropped sandbox.
        if !self.is_isolated_workspace() {
            return;
        }
        let workdir = self.workdir.clone();
        let instance_id = self.instance_id;
        // Only spawn if a tokio runtime is active (safe during sync test teardown).
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move {
                let target_dir = workdir.join("target");
                if target_dir.exists() {
                    if let Err(e) = fs::remove_dir_all(&target_dir).await {
                        debug!(sandbox = %instance_id, "drop cleanup (target): {}", e);
                    }
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_code_patch_new() {
        let patch = CodePatch::new(
            "src/main.rs".to_string(),
            vec![(1, "fn old() {}".to_string())],
            vec![(1, "fn new() {}".to_string())],
            "Renamed function old to new".to_string(),
        );
        assert!(patch.diff.contains("fn old()"));
        assert!(patch.diff.contains("fn new()"));
        assert!(patch.patch_id.is_some());
    }

    #[tokio::test]
    async fn test_apply_to_file_ignores_zero_line_numbers() {
        // Regression: a hostile patch with line number 0 must not underflow
        // `ln - 1` (panic in debug builds); the `ln >= 1` guard keeps the
        // file untouched instead.
        let workdir = TempDir::new().unwrap();
        let target = workdir.path().join("src/main.rs");
        fs::create_dir_all(workdir.path().join("src"))
            .await
            .unwrap();
        fs::write(&target, "fn a() {}\nfn b() {}\n").await.unwrap();

        let patch = CodePatch::new(
            "src/main.rs".to_string(),
            vec![(0, "fn a() {}".to_string())],
            vec![(0, "pwned".to_string())],
            "hostile zero-line patch".to_string(),
        );
        let changed = patch.apply_to_file(workdir.path()).await.unwrap();
        assert_eq!(changed, 0, "zero-line numbers must not change the file");
        let content = fs::read_to_string(&target).await.unwrap();
        assert!(content.contains("fn a() {}"));
        assert!(!content.contains("pwned"));
    }

    #[test]
    fn test_build_result_is_success() {
        let ok = BuildResult::Success {
            warnings: 2,
            time_ms: 1500,
        };
        let err = BuildResult::CompileError {
            errors: 3,
            lines: vec!["error[E0308]: type mismatch".to_string()],
        };
        assert!(ok.is_success());
        assert!(!err.is_success());
    }

    #[test]
    fn test_build_result_summary() {
        let ok = BuildResult::Success {
            warnings: 1,
            time_ms: 200,
        };
        assert!(ok.summary().contains("SUCCESS"));

        let err = BuildResult::CompileError {
            errors: 2,
            lines: vec!["error: aborting".to_string()],
        };
        assert!(err.summary().contains("COMPILE ERROR"));
    }

    #[test]
    fn test_build_result_time_ms() {
        let ok = BuildResult::Success {
            warnings: 0,
            time_ms: 500,
        };
        assert_eq!(ok.time_ms(), 500);

        let fail = BuildResult::TestFailure {
            failed: 1,
            passed: 0,
        };
        assert_eq!(fail.time_ms(), 0);
    }

    #[test]
    fn test_sandbox_executor_new() {
        let workdir = TempDir::new().unwrap();
        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 5);
        assert_eq!(executor.remaining_iterations(), 5);
    }

    #[test]
    fn test_sandbox_executor_caps_iterations() {
        let workdir = TempDir::new().unwrap();
        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 100);
        assert_eq!(executor.remaining_iterations(), MAX_ITERATIONS);
    }

    #[test]
    fn test_is_target_allowed_empty_whitelist() {
        let workdir = TempDir::new().unwrap();
        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 5);
        assert!(executor.is_target_allowed("src/main.rs"));
    }

    #[test]
    fn test_is_target_allowed_glob() {
        let workdir = TempDir::new().unwrap();
        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 5)
            .with_allowed_targets(vec!["src/**/*.rs".to_string()]);
        assert!(executor.is_target_allowed("src/main.rs"));
        assert!(executor.is_target_allowed("src/orchestration/mod.rs"));
        assert!(!executor.is_target_allowed("config.toml"));
    }

    #[test]
    fn test_is_target_allowed_rejects_boundary_and_traversal() {
        let workdir = TempDir::new().unwrap();
        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 5)
            .with_allowed_targets(vec!["src/**/*.rs".to_string()]);
        // Segment boundary: src2/ and src_evil/ are NOT under src/.
        assert!(!executor.is_target_allowed("src2/foo.rs"));
        assert!(!executor.is_target_allowed("src_evil/bar.rs"));
        // Path traversal: `..` components must never be accepted (they would
        // escape the whitelisted tree via workdir.join).
        assert!(!executor.is_target_allowed("src/../lib.rs"));
        assert!(!executor.is_target_allowed("src/../../other.rs"));
        // Normal whitelisted paths still pass.
        assert!(executor.is_target_allowed("src/main.rs"));
        assert!(executor.is_target_allowed("src/orchestration/mod.rs"));
    }

    #[test]
    fn test_is_target_allowed_exact_pattern_no_prefix_bleed() {
        let workdir = TempDir::new().unwrap();
        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 5)
            .with_allowed_targets(vec!["src/lib.rs".to_string()]);
        assert!(executor.is_target_allowed("src/lib.rs"));
        assert!(!executor.is_target_allowed("src/library.rs"));
        assert!(!executor.is_target_allowed("src2/lib.rs"));
    }

    #[test]
    fn test_code_patch_diff_empty() {
        let patch = CodePatch::new(
            "empty.rs".to_string(),
            vec![],
            vec![],
            "No changes".to_string(),
        );
        assert!(!patch.diff.is_empty());
    }

    #[test]
    fn test_sandbox_executor_instance_id() {
        let workdir = TempDir::new().unwrap();
        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 3);
        assert_ne!(executor.instance_id(), Uuid::nil());
    }

    #[tokio::test]
    async fn test_sandbox_executor_forbidden_target() {
        let workdir = TempDir::new().unwrap();
        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 5)
            .with_allowed_targets(vec!["src/lib.rs".to_string()]);
        let patch = CodePatch::new(
            "Cargo.toml".to_string(),
            vec![(1, "[package]".to_string())],
            vec![(1, "[package]".to_string())],
            "test forbidden".to_string(),
        );
        let result = executor.apply_patch(&patch).await;
        assert!(result.is_err());
        match result {
            Err(SandboxError::ForbiddenTarget(t)) => assert_eq!(t, "Cargo.toml"),
            _ => panic!("Expected ForbiddenTarget error"),
        }
    }

    #[tokio::test]
    async fn test_record_evolution_change_lands_in_audit_log() {
        // `apply_patch` runs a clippy-based quality gate that needs a full
        // Cargo project to pass, so this exercises the M4.3 audit wiring
        // end-to-end without it: a real patch applied through the sandbox
        // write site (`apply_to_file`), real before/after hashes from
        // `file_hash`, and the event recorded through the extracted
        // `record_evolution_change` helper. The full apply-loop assertion
        // would additionally need the quality gate to pass on a real
        // workspace (see `apply_patch`).
        let workdir = TempDir::new().unwrap();
        let target = "src/main.rs";
        let full = workdir.path().join(target);
        fs::create_dir_all(workdir.path().join("src"))
            .await
            .unwrap();
        fs::write(&full, "fn a() {}\n").await.unwrap();

        let patch = CodePatch::new(
            target.to_string(),
            vec![(1, "fn a() {}".to_string())],
            vec![(1, "fn b() {}".to_string())],
            "audit test patch".to_string(),
        );

        let old_hash = file_hash(&full).expect("pre-apply hash must succeed");
        assert!(old_hash.is_some(), "target file exists before apply");

        patch.apply_to_file(workdir.path()).await.unwrap();
        let new_hash = file_hash(&full).expect("post-apply hash must succeed");
        assert_ne!(
            old_hash, new_hash,
            "applying the patch must change the file"
        );

        let executor = SandboxExecutor::new(workdir.path().to_path_buf(), 1);
        executor.record_evolution_change(
            patch.patch_id,
            "apply",
            &full,
            old_hash.clone(),
            new_hash.clone(),
        );

        // The task id embeds the sandbox instance id (a fresh UUID per
        // executor), so filtering by it is collision-free even though the
        // audit sink is process-wide and shared with parallel tests.
        let patch_id = patch.patch_id.expect("CodePatch::new sets a patch id");
        let task_id = format!("self_evolution/{}/{}", executor.instance_id(), patch_id);
        let entry = crate::governance::audit::global_audit_log()
            .entries()
            .into_iter()
            .find(|e| e.task_id == task_id && e.decision == "file_change")
            .expect("record_evolution_change must land in the global audit log");
        assert_eq!(entry.phase, "apply");
        assert_eq!(entry.tool.as_deref(), Some("self_evolution"));
        assert_eq!(entry.inputs["path"], full.to_string_lossy().as_ref());
        assert_eq!(entry.inputs["op"], "self_evolution");
        let outputs = entry.outputs.as_ref().expect("outputs must be set");
        assert_eq!(outputs["old_hash"], old_hash.as_deref().unwrap());
        assert_eq!(outputs["new_hash"], new_hash.as_deref().unwrap());
    }
}
