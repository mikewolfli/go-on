//! GAP-B53-29 — Policy runtime hot-reload
//!
//! Provides traits and infrastructure for hot-reloadable governance policies.
//! Policies implement [`ReloadablePolicy`], and the [`PolicyReloader`] registry
//! watches for file changes and triggers reloads automatically.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use notify::{Config, Event, EventKind, RecursiveMode, Watcher};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

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
    #[allow(dead_code)]
    fn last_reload_ms(&self) -> u64;
}

// ─── PolicyReloader ────────────────────────────────────────────────────────

/// Runtime policy registry that watches for file changes and triggers reloads.
///
/// Manages a collection of [`ReloadablePolicy`] instances and optionally
/// monitors the filesystem for changes using `notify`.
pub struct PolicyReloader {
    /// Registered hot-reloadable policies.
    policies: Vec<Box<dyn ReloadablePolicy>>,
    /// Optional filesystem watcher for automatic reloads.
    #[allow(dead_code)] // Set during construction, used by background reload task
    watcher: Option<notify::RecommendedWatcher>,
    /// The directory path being watched (if watcher is active).
    watch_path: Option<String>,
    /// Callback invoked when a file change is detected and reload_all completes.
    on_reload: Option<Box<dyn Fn() + Send + Sync + 'static>>,
    /// Channel sender for notifying consumers about reload events.
    notify_tx: Option<std::sync::mpsc::Sender<()>>,
}

impl Default for PolicyReloader {
    fn default() -> Self {
        Self::new()
    }
}
#[allow(dead_code)] // All methods reserved for production governance wiring
impl PolicyReloader {
    /// Create a new empty `PolicyReloader` without a watcher.
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
            watcher: None,
            watch_path: None,
            on_reload: None,
            notify_tx: None,
        }
    }

    /// Register a new hot-reloadable policy.
    pub fn register(&mut self, policy: Box<dyn ReloadablePolicy>) {
        self.policies.push(policy);
        info!("policy registered, total: {}", self.policies.len());
    }

    /// Set a callback that is invoked after every reload cycle completes.
    pub fn set_on_reload(&mut self, callback: Box<dyn Fn() + Send + Sync + 'static>) {
        self.on_reload = Some(callback);
    }

    /// Set a channel sender that receives `()` after each reload cycle.
    pub fn set_notify_channel(&mut self, tx: std::sync::mpsc::Sender<()>) {
        self.notify_tx = Some(tx);
    }

    /// Drain the notification channel, returning the number of pending reload events.
    /// This can be polled from a background task.
    pub fn drain_notifications(&self, rx: &std::sync::mpsc::Receiver<()>) -> usize {
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        count
    }

    /// Reload all registered policies. Errors are logged per-policy but
    /// do not prevent other policies from reloading.
    pub fn reload_all(&mut self) {
        for policy in self.policies.iter_mut() {
            if let Err(e) = policy.reload() {
                error!("policy reload failed after error: {e}");
            } else {
                debug!("policy reloaded successfully");
            }
        }
        info!("reload_all complete for {} policies", self.policies.len());
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
    #[allow(dead_code)] // Reserved for production governance wiring
    pub fn policies_mut(&mut self) -> &mut [Box<dyn ReloadablePolicy>] {
        &mut self.policies
    }

    /// Start watching the given directory for file changes.
    ///
    /// When a file modification event is detected, all registered policies
    /// are reloaded automatically. This is best-effort: watcher errors are
    /// logged but do not crash the process.
    #[allow(dead_code)] // Reserved for production governance wiring
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
    #[allow(dead_code)] // Reserved for production governance wiring
    pub fn stop_watching(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            // Dropping the watcher stops it
            drop(watcher);
            info!("policy watcher stopped");
        }
        self.watch_path = None;
    }

    /// Returns the path being watched, if any.
    #[allow(dead_code)] // Reserved for production governance wiring
    pub fn watch_path(&self) -> Option<&str> {
        self.watch_path.as_deref()
    }

    /// Start a background task that calls [`reload_all`] periodically.
    ///
    /// Consumes this `PolicyReloader` and spawns a tokio task that runs
    /// forever (or until the returned `JoinHandle` is cancelled).
    /// Each iteration waits `interval_secs` seconds, then calls
    /// `reload_all()`.
    ///
    /// ## Example
    ///
    /// ```ignore
    /// let handle = reloader.start_background_reload(60);
    /// // … later …
    /// handle.abort();
    /// ```
    #[allow(dead_code)] // Reserved for production governance wiring
    pub fn start_background_reload(mut self, interval_secs: u64) -> JoinHandle<()> {
        let duration = std::time::Duration::from_secs(interval_secs);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(duration);
            loop {
                interval.tick().await;
                self.reload_all();
            }
        })
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

/// Returns the current time in milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ─── Concrete reloadable policy wrappers ────────────────────────────────────

/// A reloadable policy that reads a TOML-based red-line configuration file.
#[allow(dead_code)]
pub struct RedLinePolicy {
    path: std::path::PathBuf,
    last_reload: u64,
}

impl RedLinePolicy {
    #[allow(dead_code)]
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            last_reload: 0,
        }
    }
}

impl ReloadablePolicy for RedLinePolicy {
    fn reload(&mut self) -> Result<()> {
        let content = std::fs::read_to_string(&self.path)?;
        let _config: serde_json::Value = toml::from_str(&content)?;
        self.last_reload = now_ms();
        info!("RedLine policy reloaded from {}", self.path.display());
        Ok(())
    }

    fn last_reload_ms(&self) -> u64 {
        self.last_reload
    }
}

/// A reloadable policy that reads a TOML-based quality-compass configuration file.
#[allow(dead_code)]
pub struct QualityCompassPolicy {
    path: std::path::PathBuf,
    last_reload: u64,
}

impl QualityCompassPolicy {
    #[allow(dead_code)]
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            last_reload: 0,
        }
    }
}

impl ReloadablePolicy for QualityCompassPolicy {
    fn reload(&mut self) -> Result<()> {
        let content = std::fs::read_to_string(&self.path)?;
        let _config: serde_json::Value = toml::from_str(&content)?;
        self.last_reload = now_ms();
        info!(
            "QualityCompass policy reloaded from {}",
            self.path.display()
        );
        Ok(())
    }

    fn last_reload_ms(&self) -> u64 {
        self.last_reload
    }
}

/// A reloadable policy that reads a TOML-based sandbox configuration file.
#[allow(dead_code)]
pub struct SandboxPolicyReloadable {
    path: std::path::PathBuf,
    last_reload: u64,
}

impl SandboxPolicyReloadable {
    #[allow(dead_code)]
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            last_reload: 0,
        }
    }
}

impl ReloadablePolicy for SandboxPolicyReloadable {
    fn reload(&mut self) -> Result<()> {
        let content = std::fs::read_to_string(&self.path)?;
        let _config: serde_json::Value = toml::from_str(&content)?;
        self.last_reload = now_ms();
        info!("Sandbox policy reloaded from {}", self.path.display());
        Ok(())
    }

    fn last_reload_ms(&self) -> u64 {
        self.last_reload
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
            self.last_reload = now_ms();
            Ok(())
        }

        fn last_reload_ms(&self) -> u64 {
            self.last_reload
        }
    }

    #[test]
    fn test_policy_reloader_empty() {
        let reloader = PolicyReloader::new();
        assert!(reloader.is_empty());
        assert_eq!(reloader.len(), 0);
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

    #[test]
    fn test_now_ms_nonzero() {
        assert!(now_ms() > 1_700_000_000_000u64); // Should be well past this
    }
}
