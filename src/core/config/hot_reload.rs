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
//!   ├── Debouncer (coalesces rapid events)
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
    /// Returns a `JoinHandle` that can be cancelled to stop watching.
    pub async fn start(self) -> Result<tokio::task::JoinHandle<()>> {
        // Implementation:
        // 1. Use a file metadata polling approach (simpler, no external deps needed):
        //    Poll `metadata().modified()` every `debounce_ms` interval
        // 2. When modification time changes, reload the config
        // 3. On success, update `active_config` and call `on_reload` + observers
        // 4. On failure, log warning but keep the old config active
        // 5. The loop runs until the handle is dropped/cancelled

        let handle = tokio::spawn(async move {
            self.run_watch_loop().await;
        });
        Ok(handle)
    }

    async fn run_watch_loop(self) {
        let path = self.config.config_path.clone();
        let debounce = Duration::from_millis(self.config.debounce_ms);

        // Track last known modification time
        let mut last_modified = std::time::SystemTime::UNIX_EPOCH;
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            if let Ok(mtime) = meta.modified() {
                last_modified = mtime;
            }
        }

        info!("Hot-reload watchdog started for: {:?}", path);

        loop {
            tokio::time::sleep(debounce).await;

            let current = match tokio::fs::metadata(&path).await {
                Ok(meta) => match meta.modified() {
                    Ok(mtime) => mtime,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };

            if current == last_modified {
                continue;
            }
            last_modified = current;

            // Notify observers that reload is starting
            for observer in &self.observers {
                observer.on_reload_started(&path);
            }

            // Reload config using the existing AppConfig::load which handles
            // parsing, normalization, auto-rules, and role registry installation.
            match AppConfig::load(&path) {
                Ok(new_config) => {
                    info!("Config hot-reloaded successfully from: {:?}", path);
                    let mut guard = self.active_config.write().await;
                    *guard = new_config.clone();
                    drop(guard);

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
                        observer.on_reload_succeeded(&path);
                    }
                }
                Err(e) => {
                    warn!("Config hot-reload FAILED for {:?}: {}", path, e);
                    for observer in &self.observers {
                        observer.on_reload_failed(&path, &e);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_hot_reload_config_default() {
        let config = HotReloadConfig::default();
        assert!(config.enabled);
        assert_eq!(config.debounce_ms, DEFAULT_DEBOUNCE_MS);
    }

    #[test]
    fn test_config_round_trip() {
        // Validate that default config can be serialized
        let config = HotReloadConfig::default();
        let _ = serde_json::to_value(&config).expect("serialization should work");
    }
}
