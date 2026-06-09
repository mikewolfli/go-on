use std::time::Instant;

/// Tracks backend crash history and manages auto-restart recovery logic.
/// Provides rate-limited restart with exponential backoff.
pub struct CrashRecovery {
    /// Track when the backend crashed to enable auto-restart with rate limiting
    pub backend_crash_time: Option<Instant>,
    /// Count of consecutive backend crashes for rate limiting
    pub backend_crash_count: u8,
    /// Non-blocking restart cooldown timestamp.
    /// Set after killing the old backend; when elapsed, the new backend is spawned.
    pub restart_cooldown_until: Option<Instant>,
}

impl CrashRecovery {
    pub fn new() -> Self {
        Self {
            backend_crash_time: None,
            backend_crash_count: 0,
            restart_cooldown_until: None,
        }
    }

    /// Record a backend crash event.
    /// F-GAP-60: Reserved for future backend crash monitoring
    #[allow(dead_code)]
    pub fn record_crash(&mut self) {
        self.backend_crash_time = Some(Instant::now());
        self.backend_crash_count = self.backend_crash_count.saturating_add(1);
    }

    /// Reset crash state (called on successful health check or manual reset).
    pub fn reset(&mut self) {
        self.backend_crash_time = None;
        self.backend_crash_count = 0;
        self.restart_cooldown_until = None;
    }

    /// Whether auto-restart should be suppressed (too many crashes).
    pub fn should_give_up(&self) -> bool {
        self.backend_crash_count >= 10
    }

    /// Compute the backoff duration in seconds for the current crash count.
    pub fn backoff_secs(&self) -> u64 {
        3u64 * (1u64 << self.backend_crash_count.min(5))
    }
}
