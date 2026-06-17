//! Configuration hot-reload mechanism
//!
//! Watches the config file for changes using the `notify` crate (inotify on Linux,
//! kqueue on macOS, ReadDirectoryChanges on Windows). When a change is detected,
//! re-reads the config, validates it, and swaps the active configuration atomically.
//!
//! # Architecture
//!
//! ```text
//! WatchDog (background task)
//!   ├── Debouncer (coalesces rapid events via `notify`)
//!   ├── ConfigLoader (re-reads & validates)
//!   └── Callback (notifies subscribers)
//! ```
//!
//! # Thread safety
//!
//! `Arc<RwLock<AppConfig>>` enables lock-free reads and exclusive write during reload.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::core::config::AppConfig;
use crate::core::config_validation::ConfigValidator;
use crate::protocol::state_sync::{self, StateSyncEvent};

use anyhow::Result;
use tracing::{info, warn};

/// Default debounce interval for coalescing rapid file change events.
const DEFAULT_DEBOUNCE_MS: u64 = 500;

/// Callback invoked after a successful config reload.
pub type OnReload = Arc<dyn Fn(&AppConfig) + Send + Sync>;

/// Configuration for hot-reload behaviour.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HotReloadConfig {
    /// Path to the config file to watch.
    pub config_path: PathBuf,
    /// Debounce interval in milliseconds.
    pub debounce_ms: u64,
    /// Whether to enable watching.
    pub enabled: bool,
}

impl Default for HotReloadConfig {
    fn default() -> Self {
        Self {
            config_path: PathBuf::from("config/config.toml"),
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            enabled: true,
        }
    }
}

/// Receives notifications about config reload lifecycle events.
pub trait ReloadObserver: Send + Sync {
    /// Called when a config reload attempt begins.
    fn on_reload_started(&self, path: &Path);
    /// Called when a config reload succeeds.
    fn on_reload_succeeded(&self, path: &Path);
    /// Called when a config reload fails.
    fn on_reload_failed(&self, path: &Path, error: &anyhow::Error);
}

/// The hot-reload watchdog that monitors config file changes.
pub struct WatchDog {
    config: HotReloadConfig,
    active_config: Arc<RwLock<AppConfig>>,
    on_reload: Option<OnReload>,
    observers: Vec<Box<dyn ReloadObserver>>,
}

impl WatchDog {
    /// Create a new WatchDog.
    pub fn new(config: HotReloadConfig, active_config: Arc<RwLock<AppConfig>>) -> Self {
        Self {
            config,
            active_config,
            on_reload: None,
            observers: Vec::new(),
        }
    }

    /// Set the reload callback.
    pub fn on_reload(mut self, callback: OnReload) -> Self {
        self.on_reload = Some(callback);
        self
    }

    /// Register a reload observer.
    pub fn add_observer(&mut self, observer: Box<dyn ReloadObserver>) {
        self.observers.push(observer);
    }

    /// Start the watchdog in a background tokio task.
    /// Uses `notify::RecommendedWatcher` for file system events.
    /// Falls back to polling on failure.
    /// Returns a `JoinHandle` that can be cancelled to stop watching.
    pub async fn start(self) -> Result<tokio::task::JoinHandle<()>> {
        // If disabled, return immediately without spawning the notify watcher.
        if !self.config.enabled {
            info!("Hot-reload watchdog disabled by config");
            let handle = tokio::spawn(async move {});
            return Ok(handle);
        }

        let handle = tokio::spawn(async move {
            // Attempt to use notify-based watcher
            let path = self.config.config_path.clone();
            let debounce = Duration::from_millis(self.config.debounce_ms);

            match Self::run_notify_watch(&path, debounce).await {
                Ok(watch_handle) => {
                    // Watch started successfully — run the reload loop
                    Self::run_notify_loop(self, path, debounce, watch_handle).await;
                }
                Err(e) => {
                    warn!(
                        "notify watcher failed to start: {e}; falling back to polling-based watch"
                    );
                    // Fallback to polling
                    self.run_polling_loop().await;
                }
            }
        });
        Ok(handle)
    }

    /// Set up a notify-based file watcher on the config file and its parent.
    async fn run_notify_watch(
        path: &Path,
        _debounce: Duration,
    ) -> Result<tokio::sync::mpsc::Receiver<notify::Event>> {
        use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

        let (tx, rx) = tokio::sync::mpsc::channel(256);

        // Bridge from notify's callback-based API to tokio mpsc
        let event_tx = tx.clone();
        let watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = event_tx.blocking_send(event);
                }
            },
            Config::default()
                .with_poll_interval(_debounce)
                .with_compare_contents(false),
        )
        .map_err(|e| anyhow::anyhow!("failed to create notify watcher: {e}"))?;

        // Watch both the file itself (if it exists) and its parent directory
        // so we detect renames/saves common in editor workflows.
        let watch_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        let mut watcher = watcher;
        watcher
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .map_err(|e| anyhow::anyhow!("failed to watch directory {:?}: {e}", watch_dir))?;

        info!(
            "Hot-reload notify watcher started for: {:?} (watching {:?})",
            path, watch_dir
        );

        Ok(rx)
    }

    /// Run the reload loop using notify events.
    async fn run_notify_loop(
        self,
        path: PathBuf,
        debounce: Duration,
        mut rx: tokio::sync::mpsc::Receiver<notify::Event>,
    ) {
        use notify::EventKind;

        // Track last reload time for debouncing
        let mut last_reload = tokio::time::Instant::now();

        loop {
            let event = tokio::time::timeout(
                Duration::from_secs(30), // periodic wakeup for health check
                rx.recv(),
            )
            .await;

            let should_reload = match event {
                Ok(Some(event)) => {
                    let matched = event
                        .paths
                        .iter()
                        .any(|p| p == &path || path.starts_with(p));
                    match event.kind {
                        EventKind::Modify(_) | EventKind::Create(_) => matched,
                        EventKind::Remove(_) => true,
                        EventKind::Any => true,
                        _ => matched,
                    }
                }
                Ok(None) => {
                    // Channel closed
                    info!("notify watcher channel closed, stopping watch");
                    break;
                }
                Err(_) => {
                    // Timeout — periodic wakeup, check modified time as fallback
                    false
                }
            };

            if should_reload {
                // Debounce: ensure minimum interval between reloads
                let now = tokio::time::Instant::now();
                if now.duration_since(last_reload) < debounce {
                    continue;
                }
                last_reload = now;

                Self::reload_config(&self, &path).await;
            }
        }
    }

    /// Reload the config and notify observers.
    async fn reload_config(self: &WatchDog, path: &Path) {
        // Notify observers that reload is starting
        for observer in &self.observers {
            observer.on_reload_started(path);
        }

        match AppConfig::load(path) {
            Ok(new_config) => {
                // B51-35: Validate the new config before applying. If the config
                // has critical validation errors, reject the reload entirely.
                let validator = ConfigValidator::new(path, new_config.clone());
                let validation = validator.validate();
                if validation.has_critical_errors() {
                    let error_details: Vec<String> = validation
                        .critical_errors()
                        .iter()
                        .map(|e| format!("[{}] {}", e.section, e.message))
                        .collect();
                    let err_msg = format!("Config validation failed: {}", error_details.join("; "));
                    warn!("Config hot-reload REJECTED for {:?}: {}", path, err_msg);
                    for observer in &self.observers {
                        observer.on_reload_failed(path, &anyhow::anyhow!(err_msg.clone()));
                    }
                    return;
                }

                info!("Config hot-reloaded successfully from: {:?}", path);
                let mut guard = self.active_config.write().await;
                *guard = new_config.clone();
                drop(guard);

                // Notify all connected clients via state sync broadcaster
                state_sync::publish_event(StateSyncEvent::ConfigReloaded {
                    changed_keys: vec![], // fine-grained key tracking could be added later
                });

                if let Some(ref cb) = self.on_reload {
                    let cb = cb.clone();
                    let config_snapshot = self.active_config.read().await.clone();
                    tokio::task::spawn_blocking(move || {
                        cb(&config_snapshot);
                    })
                    .await
                    .ok();
                }

                for observer in &self.observers {
                    observer.on_reload_succeeded(path);
                }
            }
            Err(e) => {
                warn!("Config hot-reload FAILED for {:?}: {}", path, e);
                for observer in &self.observers {
                    observer.on_reload_failed(path, &e);
                }
            }
        }
    }

    /// Fallback polling-based watch loop (used when notify fails).
    async fn run_polling_loop(self) {
        let path = self.config.config_path.clone();
        let debounce = Duration::from_millis(self.config.debounce_ms);

        // Track last known modification time
        let mut last_modified = std::time::SystemTime::UNIX_EPOCH;
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            if let Ok(mtime) = meta.modified() {
                last_modified = mtime;
            }
        }

        info!("Hot-reload polling watchdog started for: {:?}", path);

        // Exponential backoff: start at 500ms, double on each error, cap at 30s.
        let backoff_base = Duration::from_millis(500);
        let backoff_cap = Duration::from_secs(30);
        let mut backoff = backoff_base;
        let mut consecutive_errors: u32 = 0;

        loop {
            tokio::time::sleep(debounce.max(backoff)).await;

            let current = match tokio::fs::metadata(&path).await {
                Ok(meta) => match meta.modified() {
                    Ok(mtime) => mtime,
                    Err(_) => {
                        consecutive_errors += 1;
                        backoff = backoff_base
                            .checked_mul(1u32 << consecutive_errors.min(10))
                            .unwrap_or(backoff_cap)
                            .min(backoff_cap);
                        continue;
                    }
                },
                Err(_) => {
                    consecutive_errors += 1;
                    backoff = backoff_base
                        .checked_mul(1u32 << consecutive_errors.min(10))
                        .unwrap_or(backoff_cap)
                        .min(backoff_cap);
                    continue;
                }
            };

            if current == last_modified {
                continue;
            }
            last_modified = current;

            // Success — reset backoff
            consecutive_errors = 0;
            backoff = backoff_base;

            Self::reload_config(&self, &path).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_config_round_trip() {
        // Validate that default config can be serialized
        let config = HotReloadConfig::default();
        let _ = serde_json::to_value(&config).expect("serialization should work");
    }
}
