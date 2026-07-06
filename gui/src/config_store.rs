use crate::config::AppConfig;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};

/// Manages application configuration loading, saving, and shared access.
/// Provides fingerprint-based change detection to avoid unnecessary syncs.
///
/// All access to the mutable configuration goes through `Arc<RwLock<>>` to
/// prevent data races between the UI thread (which mutates config) and async
/// tasks (which read the shared snapshot).
pub struct ConfigStore {
    /// Mutable application configuration, synchronized via RwLock.
    inner: Arc<RwLock<AppConfig>>,
    /// Immutable snapshot shared across threads for use in async tasks
    config_shared: Arc<AppConfig>,
    /// Generation counter incremented on each write; used to detect changes
    config_generation: Arc<AtomicU64>,
    /// Generation at which config_shared was last synced
    config_shared_generation: u64,
}

impl ConfigStore {
    pub fn new(config: AppConfig) -> Self {
        let config_shared = Arc::new(config.clone());
        Self {
            inner: Arc::new(RwLock::new(config)),
            config_shared,
            config_generation: Arc::new(AtomicU64::new(0)),
            config_shared_generation: 0,
        }
    }

    /// Acquire a read lock on the inner config.
    /// Returns a `RwLockReadGuard` that derefs to `AppConfig`.
    /// Recovers from a poisoned lock by logging a warning.
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, AppConfig> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Acquire a write lock on the inner config.
    /// Returns a `RwLockWriteGuard` that derefs to `AppConfig`.
    /// Recovers from a poisoned lock by logging a warning.
    /// Increments the generation counter so `sync_shared_if_needed` detects the change.
    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, AppConfig> {
        self.config_generation.fetch_add(1, Ordering::Release);
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sync the shared config snapshot if the mutable config has changed.
    /// Uses a generation counter (incremented on each write) instead of hashing
    /// all config fields every frame.
    pub fn sync_shared_if_needed(&mut self) {
        let current_gen = self.config_generation.load(Ordering::Acquire);
        if current_gen != self.config_shared_generation {
            let config = self
                .inner
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            self.config_shared = Arc::new(AppConfig::clone(&config));
            self.config_shared_generation = current_gen;
        }
    }

    /// Get the current language code from config.
    pub fn current_lang_code(&self) -> &str {
        &self.config_shared.language
    }

    /// Get a reference to the shared config.
    pub fn shared(&self) -> &Arc<AppConfig> {
        &self.config_shared
    }
}
