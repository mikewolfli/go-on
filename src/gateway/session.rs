//! Session identity and per-chat turn lease (M3.4).
//!
//! [`session_key`] derives the canonical session hash from the platform name
//! and the platform-scoped chat id. [`TurnLease`] is a per-`(platform,
//! chat_id)` lock: a webhook delivery that is already being answered for the
//! same chat is rejected instead of running two concurrent agent turns.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default lease duration: a held lease expires after 10 minutes, bounding the
/// blast radius of a wedged turn. The holder normally releases on drop, but a
/// crashed task cannot block a chat forever.
pub const DEFAULT_LEASE_DURATION: Duration = Duration::from_secs(10 * 60);

/// Canonical session hash for a `(platform, chat_id)` pair.
///
/// The `:` separator is safe in practice because platform names are
/// `&'static str` constants chosen by the adapter (never user-controlled), so
/// `"a:b" + "c"` cannot be confused with `"a" + "b:c"`.
pub fn session_key(platform: &str, chat_id: &str) -> String {
    format!("{platform}:{chat_id}")
}

/// Per-chat turn lease (M3.4): at most one in-flight agent turn per
/// `(platform, chat_id)` session.
///
/// `try_claim` succeeds only when no *unexpired* lease exists for the session;
/// an expired lease (holder crashed or the turn outlived the window) is
/// reclaimed. The returned guard releases the lease when dropped.
pub struct TurnLease {
    active: Mutex<HashMap<String, Instant>>,
    lease_duration: Duration,
}

impl TurnLease {
    /// A lease with the default 10-minute expiry.
    pub fn new() -> Self {
        Self::with_lease_duration(DEFAULT_LEASE_DURATION)
    }

    /// A lease with a custom expiry (used by tests to exercise reclamation).
    pub fn with_lease_duration(lease_duration: Duration) -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
            lease_duration,
        }
    }

    /// Try to claim the turn lease for a session.
    ///
    /// Returns `None` when another turn holds an unexpired lease for the same
    /// `(platform, chat_id)`; the caller should reject the delivery (the
    /// platform will retry, and the delivery ledger dedups the replay).
    pub fn try_claim(&self, platform: &str, chat_id: &str) -> Option<TurnLeaseGuard<'_>> {
        let key = session_key(platform, chat_id);
        let now = Instant::now();
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(claimed_at) = active.get(&key) {
            if now.duration_since(*claimed_at) < self.lease_duration {
                return None;
            }
        }
        active.insert(key.clone(), now);
        Some(TurnLeaseGuard {
            lease: self,
            key,
        })
    }

    /// Whether an unexpired lease is currently held for the session.
    pub fn is_active(&self, platform: &str, chat_id: &str) -> bool {
        let key = session_key(platform, chat_id);
        let now = Instant::now();
        let active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        active
            .get(&key)
            .is_some_and(|claimed_at| now.duration_since(*claimed_at) < self.lease_duration)
    }

    fn release(&self, key: &str) {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }
}

impl Default for TurnLease {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII handle for a held [`TurnLease`]: releases the lease on drop.
pub struct TurnLeaseGuard<'a> {
    lease: &'a TurnLease,
    key: String,
}

impl Drop for TurnLeaseGuard<'_> {
    fn drop(&mut self) {
        self.lease.release(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_combines_platform_and_chat() {
        assert_eq!(session_key("telegram", "42"), "telegram:42");
        assert_ne!(session_key("telegram", "42"), session_key("telegram", "43"));
        assert_ne!(session_key("telegram", "42"), session_key("wecom", "42"));
    }

    #[test]
    fn lease_denies_concurrent_claim_and_releases_on_drop() {
        let lease = TurnLease::new();
        assert!(!lease.is_active("telegram", "42"));

        let guard = lease.try_claim("telegram", "42").expect("first claim wins");
        assert!(lease.is_active("telegram", "42"));
        assert!(
            lease.try_claim("telegram", "42").is_none(),
            "concurrent turn for the same chat must be denied"
        );

        // A different chat is unaffected.
        let other = lease.try_claim("telegram", "43").expect("other chat claimable");
        assert!(lease.is_active("telegram", "43"));

        drop(guard);
        assert!(!lease.is_active("telegram", "42"));
        assert!(
            lease.try_claim("telegram", "42").is_some(),
            "lease is released on guard drop"
        );
        drop(other);
    }

    #[test]
    fn lease_expires_after_timeout_and_can_be_reclaimed() {
        let lease = TurnLease::with_lease_duration(Duration::from_millis(10));
        let _guard = lease.try_claim("telegram", "42").expect("claim");
        assert!(lease.try_claim("telegram", "42").is_none());

        std::thread::sleep(Duration::from_millis(50));
        assert!(
            lease.try_claim("telegram", "42").is_some(),
            "an expired lease must be reclaimable"
        );
    }
}
