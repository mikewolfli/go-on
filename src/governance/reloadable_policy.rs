//! GAP-B53-29 — Policy runtime hot-reload
//!
//! Provides traits and infrastructure for hot-reloadable governance policies.
//! Policies implement [`ReloadablePolicy`], and the [`PolicyReloader`] registry
//! watches for file changes and triggers reloads automatically.

use std::path::Path;

use anyhow::Result;
use notify::{Config, Event, EventKind, RecursiveMode, Watcher};
use tracing::{debug, error, info, warn};

use crate::governance::audit::{record_audit_threadsafe, ThreadSafeAuditLog};

// ─── ReloadablePolicy trait ────────────────────────────────────────────────

/// A policy that can be hot-reloaded at runtime.
///
/// Implementors define their own reload mechanism (e.g., re-reading a TOML file,
/// fetching from a remote endpoint, or re-evaluating an expression).
pub trait ReloadablePolicy: Send + Sync {
    /// Reload the policy from its source.
    ///
    /// Returns `Ok(())` on success. On failure the policy should retain its
    /// last known good state and log the error.
    fn reload(&mut self) -> Result<()>;

    /// Returns the last known good version timestamp (milliseconds since epoch).
    fn last_reload_ms(&self) -> u64;

    /// Return an optional evaluator closure that can be registered into the
    /// runtime policy map.  The closure is invoked during evaluate(); if it
    /// returns Some(verdict) the evaluation short-circuits.
    ///
    /// The default implementation returns None, meaning the reloadable policy
    /// is tracked (checksum, reload cycle) but does not participate in
    /// runtime evaluation.  Concrete policy types override this to wire their
    /// configuration into the evaluation pipeline.
    fn as_evaluator_fn(&self) -> Option<crate::governance::harness_bus::evaluator::PolicyFn> {
        None
    }
}

// ─── PolicyReloader ────────────────────────────────────────────────────────

/// Minimum interval between reload cycles.
///
/// `PolicyEvaluator::evaluate()` invokes `reload_all()` on every request (hot
/// path), and each reload cycle performs 3 synchronous file reads + sha256
/// digests even when the files are unchanged. The background 60s task keeps
/// policies fresh; this bound turns per-request reloads into cheap timestamp
/// checks after the first request in each window.
const RELOAD_MIN_INTERVAL_MS: u64 = 2000;

/// Runtime policy registry that watches for file changes and triggers reloads.
///
/// Manages a collection of [`ReloadablePolicy`] instances and optionally
/// monitors the filesystem for changes using `notify`.
pub struct PolicyReloader {
    /// Registered hot-reloadable policies.
    policies: Vec<Box<dyn ReloadablePolicy>>,
    /// Optional filesystem watcher for automatic reloads.
    watcher: Option<notify::RecommendedWatcher>,
    /// The directory path being watched (if watcher is active).
    watch_path: Option<String>,
    /// Callback invoked when a file change is detected and reload_all completes.
    on_reload: Option<Box<dyn Fn() + Send + Sync + 'static>>,
    /// Channel sender for notifying consumers about reload events.
    notify_tx: Option<std::sync::mpsc::Sender<()>>,
    /// Optional audit log for recording reload events.
    audit_log: Option<ThreadSafeAuditLog>,
    /// Timestamp (ms) of the last completed reload cycle.
    last_reload_cycle_ms: u64,
}

impl Default for PolicyReloader {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyReloader {
    /// Create a new empty `PolicyReloader` without a watcher.
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
            watcher: None,
            watch_path: None,
            on_reload: None,
            notify_tx: None,
            audit_log: None,
            last_reload_cycle_ms: 0,
        }
    }

    /// Register a new hot-reloadable policy.
    pub fn register(&mut self, policy: Box<dyn ReloadablePolicy>) {
        self.policies.push(policy);
        info!("policy registered, total: {}", self.policies.len());
    }

    /// Reload all registered policies. Errors are logged per-policy but
    /// do not prevent other policies from reloading.
    ///
    /// If an audit log is configured, a reload event is recorded.
    ///
    /// Hot-path callers (every `PolicyEvaluator::evaluate()`) invoke this on
    /// every request; the `RELOAD_MIN_INTERVAL_MS` guard skips the actual
    /// disk reads when a cycle completed within the window.
    pub fn reload_all(&mut self) {
        let now = crate::shared::timestamps::now_ts_ms() as u64;
        if now.saturating_sub(self.last_reload_cycle_ms) < RELOAD_MIN_INTERVAL_MS {
            return;
        }
        self.last_reload_cycle_ms = now;

        let policy_count = self.policies.len();
        let mut errors: Vec<String> = Vec::new();

        for policy in self.policies.iter_mut() {
            if let Err(e) = policy.reload() {
                error!("policy reload failed after error: {e}");
                errors.push(format!("{e}"));
            } else {
                debug!("policy reloaded successfully");
            }
        }

        info!("reload_all complete for {} policies", policy_count);

        // Record reload event to audit log if configured
        if let Some(ref audit) = self.audit_log {
            let error_msg = if errors.is_empty() {
                None
            } else {
                Some(errors.join("; "))
            };
            record_audit_threadsafe(
                audit,
                "governance",
                "policy_reload",
                &format!("reloaded {} policies", policy_count),
                error_msg,
                Some(format!(
                    "reload-{}",
                    crate::shared::timestamps::now_ts_ms() as u64
                )),
            );
        }
    }

    /// Return the number of registered policies.
    pub fn len(&self) -> usize {
        self.policies.len()
    }

    /// Returns `true` if no policies are registered.
    pub fn is_empty(&self) -> bool {
        self.policies.is_empty()
    }

    /// Get a reference to all registered policies.
    pub fn policies(&self) -> &[Box<dyn ReloadablePolicy>] {
        &self.policies
    }

    /// Get a mutable reference to all registered policies.
    pub fn policies_mut(&mut self) -> &mut [Box<dyn ReloadablePolicy>] {
        &mut self.policies
    }

    /// Start watching the given directory for file changes.
    ///
    /// When a file modification event is detected, all registered policies
    /// are reloaded automatically. This is best-effort: watcher errors are
    /// logged but do not crash the process.
    pub fn start_watching(&mut self, watch_dir: impl AsRef<Path>) -> Result<()> {
        let watch_dir = watch_dir.as_ref().to_path_buf();
        let watch_path = watch_dir.to_string_lossy().to_string();

        // Clone the callback and sender for the watcher closure.
        let on_reload = self.on_reload.take();
        let notify_tx = self.notify_tx.clone();

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    if matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                    ) {
                        debug!(
                            "file change detected in {:?}: {:?}",
                            event.paths, event.kind
                        );
                        info!("policy watch event triggered: {:?}", event.kind);

                        // Invoke the reload callback.
                        if let Some(ref cb) = on_reload {
                            cb();
                        }

                        // Notify consumers via the channel.
                        if let Some(ref tx) = notify_tx {
                            let _ = tx.send(());
                        }
                    }
                }
                Err(e) => warn!("policy watcher error: {e}"),
            }
        })?;

        watcher.configure(Config::default())?;
        watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
        info!("policy watcher started on {}", watch_path);

        self.watcher = Some(watcher);
        self.watch_path = Some(watch_path);
        Ok(())
    }

    /// Stop the filesystem watcher, if one is active.
    pub fn stop_watching(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            // Dropping the watcher stops it
            drop(watcher);
            info!("policy watcher stopped");
        }
        self.watch_path = None;
    }

    /// Returns the path being watched, if any.
    pub fn watch_path(&self) -> Option<&str> {
        self.watch_path.as_deref()
    }
}

impl std::fmt::Debug for PolicyReloader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyReloader")
            .field("policy_count", &self.policies.len())
            .field("watch_path", &self.watch_path)
            .field("has_on_reload", &self.on_reload.is_some())
            .field("has_notify_tx", &self.notify_tx.is_some())
            .finish()
    }
}

// ─── Utility: time helpers ────────────────────────────────────────────────

/// Compute a SHA-256 digest of the given bytes, returning the raw 32-byte hash.
/// Used by reloadable policies for checksum validation.
pub fn sha256_digest(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

// ─── Concrete reloadable policy wrappers ────────────────────────────────────

/// A reloadable policy that reads a TOML-based red-line configuration file.
pub struct RedLinePolicy {
    path: std::path::PathBuf,
    last_reload: u64,
    /// SHA-256 checksum of the last loaded file content, used to skip
    /// redundant reloads when the file hasn't actually changed.
    checksum: Option<Vec<u8>>,
    /// Parsed TOML configuration, preserved for policy consumers.
    config: Option<serde_json::Value>,
}

impl RedLinePolicy {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            last_reload: 0,
            checksum: None,
            config: None,
        }
    }
}

impl ReloadablePolicy for RedLinePolicy {
    fn reload(&mut self) -> Result<()> {
        // Path validation: ensure the file exists and is readable
        if !self.path.exists() {
            // Auto-create with sensible defaults so the system always has
            // an explicit configuration and users can see/edit the file.
            let default_toml = r#"# RedLine policy — blocks high-risk operations
#   action:  "allow" | "deny"
#   field:   "risk_score" | "file_count"
#   threshold: trigger value (0.0–1.0 for risk_score)
action = "deny"
field = "risk_score"
threshold = 0.5
"#;
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&self.path, default_toml.as_bytes()) {
                tracing::warn!(
                    "failed to create default RedLine policy at {}: {}",
                    self.path.display(),
                    e
                );
                self.config = None;
                self.checksum = None;
                return Ok(());
            }
            tracing::info!("Created default RedLine policy at {}", self.path.display());
        }
        if !self.path.is_file() {
            anyhow::bail!("RedLine policy path is not a file: {}", self.path.display());
        }

        let content = std::fs::read_to_string(&self.path)?;

        // Checksum validation: skip reload if content hasn't changed
        let new_checksum = sha256_digest(content.as_bytes());
        if self.checksum.as_ref() == Some(&new_checksum) {
            debug!(
                "RedLine policy unchanged, skipping reload: {}",
                self.path.display()
            );
            return Ok(());
        }

        let config: serde_json::Value = toml::from_str(&content)?;
        self.last_reload = crate::shared::timestamps::now_ts_ms() as u64;
        self.checksum = Some(new_checksum);
        self.config = Some(config);
        info!("RedLine policy reloaded from {}", self.path.display());
        Ok(())
    }

    fn last_reload_ms(&self) -> u64 {
        self.last_reload
    }

    fn as_evaluator_fn(&self) -> Option<crate::governance::harness_bus::evaluator::PolicyFn> {
        let config = self.config.clone()?;
        let action = config
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("deny")
            .to_string();
        let field = config
            .get("field")
            .and_then(|v| v.as_str())
            .unwrap_or("risk_score")
            .to_string();
        let threshold = config
            .get("threshold")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5);
        Some(Box::new(
            move |ctx: &crate::governance::pua::TaskContext| {
                let value = match field.as_str() {
                    "risk_score" => Some(ctx.risk_score),
                    "file_count" => Some(ctx.file_count as f64),
                    _ => None,
                };
                if let Some(v) = value {
                    if v >= threshold {
                        return if action == "allow" {
                            Some(crate::governance::harness_bus::types::PolicyVerdict::Allow)
                        } else {
                            Some(crate::governance::harness_bus::types::PolicyVerdict::Deny(
                                crate::governance::harness_bus::types::PolicyViolation {
                                    kind: "reloadable".to_string(),
                                    detail: format!(
                                        "RedLine policy '{}' triggered: {} >= {}",
                                        field, v, threshold
                                    ),
                                },
                            ))
                        };
                    }
                }
                None
            },
        ))
    }
}

/// A reloadable policy that reads a TOML-based quality-compass configuration file.
pub struct QualityCompassPolicy {
    path: std::path::PathBuf,
    last_reload: u64,
    /// SHA-256 checksum of the last loaded file content.
    checksum: Option<Vec<u8>>,
    /// Parsed TOML configuration, preserved for policy consumers.
    config: Option<serde_json::Value>,
}

impl QualityCompassPolicy {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            last_reload: 0,
            checksum: None,
            config: None,
        }
    }
}

impl ReloadablePolicy for QualityCompassPolicy {
    fn reload(&mut self) -> Result<()> {
        // Path validation: ensure the file exists and is readable
        if !self.path.exists() {
            let default_toml = r#"# QualityCompass policy — minimum quality gate
#   minimum_quality: minimum acceptable quality score (0.0–1.0)
#   require_review:  require human review for multi-file tasks
minimum_quality = 0.7
require_review = false
"#;
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&self.path, default_toml.as_bytes()) {
                tracing::warn!(
                    "failed to create default QualityCompass policy at {}: {}",
                    self.path.display(),
                    e
                );
                self.config = None;
                self.checksum = None;
                return Ok(());
            }
            tracing::info!(
                "Created default QualityCompass policy at {}",
                self.path.display()
            );
        }
        if !self.path.is_file() {
            anyhow::bail!(
                "QualityCompass policy path is not a file: {}",
                self.path.display()
            );
        }

        let content = std::fs::read_to_string(&self.path)?;

        // Checksum validation: skip reload if content hasn't changed
        let new_checksum = sha256_digest(content.as_bytes());
        if self.checksum.as_ref() == Some(&new_checksum) {
            debug!(
                "QualityCompass policy unchanged, skipping reload: {}",
                self.path.display()
            );
            return Ok(());
        }

        let config: serde_json::Value = toml::from_str(&content)?;
        self.last_reload = crate::shared::timestamps::now_ts_ms() as u64;
        self.checksum = Some(new_checksum);
        self.config = Some(config);
        info!(
            "QualityCompass policy reloaded from {}",
            self.path.display()
        );
        Ok(())
    }

    fn last_reload_ms(&self) -> u64 {
        self.last_reload
    }

    fn as_evaluator_fn(&self) -> Option<crate::governance::harness_bus::evaluator::PolicyFn> {
        let config = self.config.clone()?;
        let required_quality = config
            .get("minimum_quality")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.7);
        let require_review = config
            .get("require_review")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Some(Box::new(
            move |ctx: &crate::governance::pua::TaskContext| {
                let effective_quality = 1.0 - ctx.risk_score.clamp(0.0, 1.0);
                if effective_quality < required_quality {
                    return Some(
                        crate::governance::harness_bus::types::PolicyVerdict::Review(
                            crate::governance::harness_bus::types::ReviewReason {
                                reason: format!(
                                    "QualityCompass: risk_score {} below quality threshold {}",
                                    ctx.risk_score, required_quality
                                ),
                            },
                        ),
                    );
                }
                if require_review && ctx.file_count > 5 {
                    return Some(
                        crate::governance::harness_bus::types::PolicyVerdict::Review(
                            crate::governance::harness_bus::types::ReviewReason {
                                reason: "QualityCompass: multi-file task requires review"
                                    .to_string(),
                            },
                        ),
                    );
                }
                None
            },
        ))
    }
}

/// A reloadable policy that reads a TOML-based sandbox configuration file.
pub struct SandboxPolicyReloadable {
    path: std::path::PathBuf,
    last_reload: u64,
    /// SHA-256 checksum of the last loaded file content.
    checksum: Option<Vec<u8>>,
    /// Parsed TOML configuration, preserved for policy consumers.
    config: Option<serde_json::Value>,
}

impl SandboxPolicyReloadable {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            last_reload: 0,
            checksum: None,
            config: None,
        }
    }
}

impl ReloadablePolicy for SandboxPolicyReloadable {
    fn reload(&mut self) -> Result<()> {
        // Path validation: ensure the file exists and is readable
        if !self.path.exists() {
            let default_toml = r#"# Sandbox policy — execution sandbox restrictions
#   max_file_writes: maximum file writes before review
#   block_commands:  block shell commands in high-risk contexts
max_file_writes = 10
block_commands = true
"#;
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&self.path, default_toml.as_bytes()) {
                tracing::warn!(
                    "failed to create default Sandbox policy at {}: {}",
                    self.path.display(),
                    e
                );
                self.config = None;
                self.checksum = None;
                return Ok(());
            }
            tracing::info!("Created default Sandbox policy at {}", self.path.display());
        }
        if !self.path.is_file() {
            anyhow::bail!("Sandbox policy path is not a file: {}", self.path.display());
        }

        let content = std::fs::read_to_string(&self.path)?;

        // Checksum validation: skip reload if content hasn't changed
        let new_checksum = sha256_digest(content.as_bytes());
        if self.checksum.as_ref() == Some(&new_checksum) {
            debug!(
                "Sandbox policy unchanged, skipping reload: {}",
                self.path.display()
            );
            return Ok(());
        }

        let config: serde_json::Value = toml::from_str(&content)?;
        self.last_reload = crate::shared::timestamps::now_ts_ms() as u64;
        self.checksum = Some(new_checksum);
        self.config = Some(config);
        info!("Sandbox policy reloaded from {}", self.path.display());
        Ok(())
    }

    fn last_reload_ms(&self) -> u64 {
        self.last_reload
    }

    fn as_evaluator_fn(&self) -> Option<crate::governance::harness_bus::evaluator::PolicyFn> {
        let config = self.config.clone()?;
        let max_file_writes = config
            .get("max_file_writes")
            .and_then(|v| v.as_u64())
            .unwrap_or(10);
        let block_commands = config
            .get("block_commands")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        Some(Box::new(
            move |ctx: &crate::governance::pua::TaskContext| {
                if block_commands && ctx.risk_score > 0.8 {
                    return Some(crate::governance::harness_bus::types::PolicyVerdict::Deny(
                        crate::governance::harness_bus::types::PolicyViolation {
                            kind: "reloadable".to_string(),
                            detail: format!(
                                "Sandbox: command execution blocked in high-risk context (risk={})",
                                ctx.risk_score
                            ),
                        },
                    ));
                }
                let required_review =
                    max_file_writes < 5 && ctx.file_count >= max_file_writes as usize;
                if required_review {
                    return Some(
                        crate::governance::harness_bus::types::PolicyVerdict::Review(
                            crate::governance::harness_bus::types::ReviewReason {
                                reason: format!(
                                    "Sandbox: file writes {} exceed threshold {}",
                                    ctx.file_count, max_file_writes
                                ),
                            },
                        ),
                    );
                }
                None
            },
        ))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPolicy {
        reload_count: u64,
        last_reload: u64,
    }

    impl ReloadablePolicy for TestPolicy {
        fn reload(&mut self) -> Result<()> {
            self.reload_count += 1;
            self.last_reload = crate::shared::timestamps::now_ts_ms() as u64;
            Ok(())
        }

        fn last_reload_ms(&self) -> u64 {
            self.last_reload
        }
    }

    #[test]
    fn test_register_and_reload() {
        let mut reloader = PolicyReloader::new();
        reloader.register(Box::new(TestPolicy {
            reload_count: 0,
            last_reload: 0,
        }));

        assert_eq!(reloader.len(), 1);
        assert!(!reloader.is_empty());

        reloader.reload_all();

        let policies = reloader.policies();
        assert_eq!(policies.len(), 1);
    }

    #[test]
    fn test_reload_updates_timestamp() {
        let policy = TestPolicy {
            reload_count: 0,
            last_reload: 0,
        };
        let ts_before = policy.last_reload_ms();

        let mut reloader = PolicyReloader::new();
        reloader.register(Box::new(policy));
        reloader.reload_all();

        let policies = reloader.policies_mut();
        let policy = &mut policies[0];
        let ts_after = policy.last_reload_ms();
        assert!(ts_after >= ts_before);
        // Reload again
        policy.reload().unwrap();
        assert!(policy.last_reload_ms() >= ts_after);
    }

    #[test]
    fn test_start_stop_watcher() {
        let mut reloader = PolicyReloader::new();
        let dir = tempfile::tempdir().expect("temp dir");

        assert!(reloader.start_watching(dir.path()).is_ok());
        assert!(reloader.watch_path().is_some());

        reloader.stop_watching();
        assert!(reloader.watch_path().is_none());
        assert!(reloader.watcher.is_none());
    }
}
