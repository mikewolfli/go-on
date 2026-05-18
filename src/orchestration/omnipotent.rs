//! Omnipotent mode runtime — F-GAP-09 (FUTURE3.M1 / BLUE38 §6.6).
//!
//! Omnipotent mode is a special execution mode where the agent has unrestricted
//! access to all system capabilities (all tools, all skills, all data). Access
//! is gated by short-lived escalation tokens that must be validated before
//! entering the mode. Every operation performed while in omnipotent mode is
//! recorded in an audit log.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::i18n::runtime::tf;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Verdict returned when validating an escalation token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OmnipotentVerdict {
    /// Token is valid and omnipotent mode may be entered.
    Allowed,
    /// Token is invalid or expired; the accompanying message explains why.
    Denied(String),
    /// No token was provided and one is required to enter omnipotent mode.
    RequiresToken,
}

/// An escalation token that acts as a capability ticket for entering
/// omnipotent mode. Tokens are short-lived and can be revoked at any time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationToken {
    /// Unique token identifier.
    pub token_id: String,
    /// The identity (user or service) this token was issued to.
    pub issued_to: String,
    /// Unix timestamp (milliseconds) when the token was issued.
    pub issued_at: u64,
    /// Unix timestamp (milliseconds) when the token expires.
    pub expires_at: u64,
    /// Human-readable reason for the escalation.
    pub reason: String,
    /// Whether this token has been explicitly revoked.
    pub is_revoked: bool,
}

/// A single auditable action performed while in omnipotent mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmnipotentAction {
    /// Unix timestamp (milliseconds) when the action occurred.
    pub timestamp_ms: u64,
    /// The actor (user or service) that performed the action.
    pub actor: String,
    /// Description of the action (e.g. "invoke_tool", "read_file", "execute_command").
    pub action: String,
    /// The resource the action was performed on (e.g. file path, tool name, skill ID).
    pub resource: String,
    /// Outcome of the action (e.g. "success", "failure: <reason>").
    pub outcome: String,
    /// The token ID that was used to authenticate this session.
    pub token_id: String,
}

/// Snapshot of omnipotent mode runtime metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmnipotentProfile {
    /// Whether omnipotent mode is currently enabled.
    pub enabled: bool,
    /// Number of currently active omnipotent sessions.
    pub active_sessions: u32,
    /// Total number of escalation tokens ever issued.
    pub total_escalations: u64,
    /// Total number of omnipotent actions recorded.
    pub total_actions: u64,
    /// Total number of tokens that have been revoked.
    pub revoked_tokens: u64,
}

/// A session guard that is returned when omnipotent mode is entered.
/// The session count is automatically decremented when this guard is dropped.
#[derive(Debug)]
pub struct OmnipotentSession {
    active_sessions: Arc<AtomicU32>,
    /// The token ID that was used to enter this session.
    pub token_id: String,
}

impl Drop for OmnipotentSession {
    fn drop(&mut self) {
        self.active_sessions
            .fetch_update(Ordering::Release, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            })
            .ok();
    }
}

/// Omnipotent mode runtime — F-GAP-09.
///
/// Manages escalation tokens, active sessions, audit logging, and profile
/// metrics for the omnipotent execution mode.
#[derive(Debug)]
pub struct OmnipotentMode {
    /// Whether omnipotent mode is enabled.
    enabled: Arc<AtomicBool>,
    /// Registered escalation tokens (who can enter omnipotent mode).
    escalation_tokens: Arc<RwLock<HashMap<String, EscalationToken>>>,
    /// Audit log of all omnipotent operations.
    audit_log: Arc<Mutex<Vec<OmnipotentAction>>>,
    /// Maximum concurrent omnipotent sessions.
    max_concurrent: u32,
    /// Active session count.
    active_sessions: Arc<AtomicU32>,
    /// Profile metrics.
    profile: Arc<Mutex<OmnipotentProfile>>,
}

impl OmnipotentMode {
    /// Create a new `OmnipotentMode` runtime with default settings.
    ///
    /// The runtime starts with omnipotent mode disabled, no tokens, and an
    /// empty audit log. A maximum of 10 concurrent sessions is allowed by
    /// default.
    pub fn new() -> Self {
        let profile = OmnipotentProfile {
            enabled: false,
            active_sessions: 0,
            total_escalations: 0,
            total_actions: 0,
            revoked_tokens: 0,
        };
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            escalation_tokens: Arc::new(RwLock::new(HashMap::new())),
            audit_log: Arc::new(Mutex::new(Vec::new())),
            max_concurrent: 10,
            active_sessions: Arc::new(AtomicU32::new(0)),
            profile: Arc::new(Mutex::new(profile)),
        }
    }

    /// Issue a new escalation token.
    ///
    /// `issued_to` identifies the user or service that will receive the token.
    /// `reason` is a human-readable justification for the escalation.
    /// `ttl_secs` controls how many seconds the token remains valid.
    ///
    /// Returns the new `token_id` on success.
    pub fn issue_token(&self, issued_to: &str, reason: &str, ttl_secs: u64) -> Result<String> {
        let now_ms = now_epoch_ms();
        let token_id = format!("omni-{}-{}", issued_to, hex_encode_timestamp(now_ms));

        let token = EscalationToken {
            token_id: token_id.clone(),
            issued_to: issued_to.to_string(),
            issued_at: now_ms,
            expires_at: now_ms + ttl_secs * 1000,
            reason: reason.to_string(),
            is_revoked: false,
        };

        {
            let mut tokens = self
                .escalation_tokens
                .write()
                .map_err(|e| anyhow!("Failed to acquire write lock on escalation tokens: {}", e))?;
            tokens.insert(token_id.clone(), token);
        }

        // Enable omnipotent mode when the first token is issued.
        self.enabled.store(true, Ordering::Release);

        {
            let mut profile = self
                .profile
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on profile: {}", e))?;
            profile.total_escalations += 1;
            profile.enabled = self.enabled.load(Ordering::Acquire);
        }

        Ok(token_id)
    }

    /// Revoke an escalation token so it can no longer be used.
    ///
    /// This is idempotent — revoking an already-revoked or non-existent token
    /// is a no-op (aside from incrementing the revoked counter for unknown
    /// tokens to track unexpected revocation attempts).
    pub fn revoke_token(&self, token_id: &str) {
        let mut tokens = match self.escalation_tokens.write() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("omnipotent escalation_tokens RwLock poisoned, recovering");
                poisoned.into_inner()
            }
        };

        if let Some(token) = tokens.get_mut(token_id) {
            if !token.is_revoked {
                token.is_revoked = true;
                if let Ok(mut profile) = self.profile.lock() {
                    profile.revoked_tokens += 1;
                }
            }
        } else {
            // Token does not exist — still count it as a revocation attempt
            // for observability.
            if let Ok(mut profile) = self.profile.lock() {
                profile.revoked_tokens += 1;
            }
        }

        // Disable omnipotent mode if there are no valid tokens remaining.
        if count_valid_tokens(&tokens, now_epoch_ms()) == 0 {
            self.enabled.store(false, Ordering::Release);
        }

        // Sync profile enabled state.
        if let Ok(mut profile) = self.profile.lock() {
            profile.enabled = self.enabled.load(Ordering::Acquire);
        }
    }

    /// Validate an escalation token and return the appropriate verdict.
    ///
    /// A token is considered valid if:
    /// - It exists in the registry.
    /// - It has not been revoked.
    /// - It has not expired.
    pub fn validate_token(&self, token_id: &str) -> OmnipotentVerdict {
        if token_id.is_empty() {
            return OmnipotentVerdict::RequiresToken;
        }

        let tokens = match self.escalation_tokens.read() {
            Ok(guard) => guard,
            Err(_) => {
                return OmnipotentVerdict::Denied(
                    "Failed to acquire read lock on escalation tokens".to_string(),
                )
            }
        };

        let token = match tokens.get(token_id) {
            Some(t) => t,
            None => {
                return OmnipotentVerdict::Denied(tf(
                    "error.token_not_found",
                    &[("token_id", token_id)],
                ))
            }
        };

        if token.is_revoked {
            return OmnipotentVerdict::Denied(tf("error.token_revoked", &[("token_id", token_id)]));
        }

        let now_ms = now_epoch_ms();
        if now_ms > token.expires_at {
            return OmnipotentVerdict::Denied(tf(
                "error.token_expired",
                &[
                    ("token_id", token_id),
                    ("expires_at", &token.expires_at.to_string()),
                    ("now", &now_ms.to_string()),
                ],
            ));
        }

        OmnipotentVerdict::Allowed
    }

    /// Enter omnipotent mode using a validated token.
    ///
    /// Returns an `OmnipotentSession` guard if the token is valid and the
    /// maximum number of concurrent sessions has not been exceeded. The
    /// session count is automatically decremented when the returned guard
    /// is dropped.
    pub fn enter_omnipotent(&self, token_id: &str) -> Result<OmnipotentSession> {
        // Validate the token first.
        match self.validate_token(token_id) {
            OmnipotentVerdict::Allowed => { /* proceed */ }
            OmnipotentVerdict::Denied(reason) => {
                return Err(anyhow!("Access denied: {}", reason));
            }
            OmnipotentVerdict::RequiresToken => {
                return Err(anyhow!("A valid escalation token is required"));
            }
        }

        // Check concurrent session limit.
        let current = self.active_sessions.load(Ordering::Acquire);
        if current >= self.max_concurrent {
            return Err(anyhow!(
                "Maximum concurrent omnipotent sessions ({}) reached",
                self.max_concurrent
            ));
        }

        self.active_sessions.fetch_add(1, Ordering::Release);

        // Update profile.
        if let Ok(mut profile) = self.profile.lock() {
            profile.active_sessions = self.active_sessions.load(Ordering::Acquire);
        }

        Ok(OmnipotentSession {
            active_sessions: Arc::clone(&self.active_sessions),
            token_id: token_id.to_string(),
        })
    }

    /// Record an omnipotent action in the audit log.
    ///
    /// Every action performed while in omnipotent mode should be recorded for
    /// accountability and auditability.
    pub fn record_action(
        &self,
        actor: &str,
        action: &str,
        resource: &str,
        outcome: &str,
        token_id: &str,
    ) {
        let entry = OmnipotentAction {
            timestamp_ms: now_epoch_ms(),
            actor: actor.to_string(),
            action: action.to_string(),
            resource: resource.to_string(),
            outcome: outcome.to_string(),
            token_id: token_id.to_string(),
        };

        if let Ok(mut log) = self.audit_log.lock() {
            log.push(entry);
        }

        if let Ok(mut profile) = self.profile.lock() {
            profile.total_actions += 1;
        }
    }

    /// Check whether anyone is currently in omnipotent mode.
    pub fn is_omnipotent(&self) -> bool {
        self.enabled.load(Ordering::Acquire) && self.active_sessions.load(Ordering::Acquire) > 0
    }

    /// Return a snapshot of the current omnipotent mode profile metrics.
    pub fn profile(&self) -> OmnipotentProfile {
        self.profile
            .lock()
            .map(|p| p.clone())
            .unwrap_or(OmnipotentProfile {
                enabled: self.enabled.load(Ordering::Acquire),
                active_sessions: self.active_sessions.load(Ordering::Acquire),
                total_escalations: 0,
                total_actions: 0,
                revoked_tokens: 0,
            })
    }
}

impl Default for OmnipotentMode {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Private helpers ─────────────────────────────────────────────────────────

/// Return the current Unix time in milliseconds.
fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Count the number of tokens in `tokens` that are not revoked and not expired
/// (relative to `now_ms`).
fn count_valid_tokens(tokens: &HashMap<String, EscalationToken>, now_ms: u64) -> usize {
    tokens
        .values()
        .filter(|t| !t.is_revoked && now_ms <= t.expires_at)
        .count()
}

/// Produce a simple hex string from the given timestamp for use in token IDs.
///
/// This is deliberately deterministic and uses only standard library facilities
/// so no external dependencies (like `uuid` or `rand`) are required.
fn hex_encode_timestamp(ts_ms: u64) -> String {
    let bytes = ts_ms.to_le_bytes();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in &bytes {
        out.push_str(&format!("{:02x}", b));
    }
    // Append a few bytes of process-scoped randomness to reduce collision risk.
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    let extra = hasher.finish() & 0xFFFF;
    out.push_str(&format!("{:04x}", extra));
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_new_omnipotent_mode_is_disabled() {
        let om = OmnipotentMode::new();
        assert!(!om.enabled.load(Ordering::Acquire));
        assert_eq!(om.active_sessions.load(Ordering::Acquire), 0);
        let profile = om.profile();
        assert!(!profile.enabled);
        assert_eq!(profile.active_sessions, 0);
        assert_eq!(profile.total_escalations, 0);
        assert_eq!(profile.total_actions, 0);
        assert_eq!(profile.revoked_tokens, 0);
    }

    #[test]
    fn test_validate_empty_token_requires_token() {
        let om = OmnipotentMode::new();
        assert!(matches!(
            om.validate_token(""),
            OmnipotentVerdict::RequiresToken
        ));
    }

    #[test]
    fn test_validate_unknown_token_denied() {
        let om = OmnipotentMode::new();
        let verdict = om.validate_token("nonexistent");
        assert!(
            matches!(verdict, OmnipotentVerdict::Denied(ref s) if s.contains("error.token_not_found") || s.contains("not found"))
        );
    }

    #[test]
    fn test_issue_and_validate_token() {
        let om = OmnipotentMode::new();
        let token_id = om
            .issue_token("alice", "Emergency maintenance", 60)
            .expect("Should issue token");

        assert!(om.enabled.load(Ordering::Acquire));
        assert!(matches!(
            om.validate_token(&token_id),
            OmnipotentVerdict::Allowed
        ));
    }

    #[test]
    fn test_issue_token_updates_profile() {
        let om = OmnipotentMode::new();
        let _ = om
            .issue_token("bob", "Debug production issue", 120)
            .unwrap();
        let profile = om.profile();
        assert!(profile.enabled);
        assert_eq!(profile.total_escalations, 1);
    }

    #[test]
    fn test_revoke_token() {
        let om = OmnipotentMode::new();
        let token_id = om.issue_token("carol", "Security audit", 3600).unwrap();

        om.revoke_token(&token_id);

        let verdict = om.validate_token(&token_id);
        assert!(matches!(verdict, OmnipotentVerdict::Denied(ref s) if s.contains("revoked")));
    }

    #[test]
    fn test_revoke_token_updates_profile() {
        let om = OmnipotentMode::new();
        let token_id = om.issue_token("dave", "Testing", 60).unwrap();
        om.revoke_token(&token_id);
        let profile = om.profile();
        assert_eq!(profile.revoked_tokens, 1);
    }

    #[test]
    fn test_revoke_unknown_token_is_noop() {
        let om = OmnipotentMode::new();
        om.revoke_token("does-not-exist");
        let profile = om.profile();
        // Unknown revocation attempts are still counted for observability.
        assert_eq!(profile.revoked_tokens, 1);
    }

    #[test]
    fn test_expired_token_is_denied() {
        let om = OmnipotentMode::new();
        // Issue a token with 0 TTL so it expires immediately.
        let token_id = om.issue_token("eve", "Quick test", 0).unwrap();

        // Small sleep to ensure expiry has passed.
        std::thread::sleep(Duration::from_millis(10));

        let verdict = om.validate_token(&token_id);
        assert!(matches!(verdict, OmnipotentVerdict::Denied(ref s) if s.contains("expired")));
    }

    #[test]
    fn test_enter_omnipotent_with_valid_token() {
        let om = OmnipotentMode::new();
        let token_id = om.issue_token("frank", "System upgrade", 300).unwrap();

        let session = om
            .enter_omnipotent(&token_id)
            .expect("Should enter omnipotent mode");
        assert!(om.is_omnipotent());
        assert_eq!(session.token_id, token_id);

        // Session count should be 1.
        assert_eq!(om.active_sessions.load(Ordering::Acquire), 1);
    }

    #[test]
    fn test_enter_omnipotent_with_invalid_token_fails() {
        let om = OmnipotentMode::new();
        let _ = om.issue_token("grace", "Testing", 300).unwrap();

        let result = om.enter_omnipotent("bad-token");
        assert!(result.is_err());
        assert!(!om.is_omnipotent());
    }

    #[test]
    fn test_enter_omnipotent_with_expired_token_fails() {
        let om = OmnipotentMode::new();
        let token_id = om.issue_token("heidi", "Zero TTL", 0).unwrap();
        std::thread::sleep(Duration::from_millis(10));

        let result = om.enter_omnipotent(&token_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_drop_session_decrements_count() {
        let om = OmnipotentMode::new();
        let token_id = om.issue_token("ivan", "Maintenance", 300).unwrap();

        {
            let _session = om.enter_omnipotent(&token_id).unwrap();
            assert_eq!(om.active_sessions.load(Ordering::Acquire), 1);
        }
        // Session was dropped, count should be back to 0.
        assert_eq!(om.active_sessions.load(Ordering::Acquire), 0);
        assert!(!om.is_omnipotent());
    }

    #[test]
    fn test_max_concurrent_sessions() {
        let om = OmnipotentMode::new();
        let token_id = om.issue_token("jane", "Multiple sessions", 300).unwrap();

        // Enter max_concurrent sessions.
        let mut sessions = Vec::new();
        for _ in 0..om.max_concurrent {
            let session = om.enter_omnipotent(&token_id).unwrap();
            sessions.push(session);
        }

        // The next attempt should fail.
        let result = om.enter_omnipotent(&token_id);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Maximum concurrent"));

        assert_eq!(
            om.active_sessions.load(Ordering::Acquire),
            om.max_concurrent
        );

        // Drop one session, then we should be able to enter again.
        sessions.pop();
        assert_eq!(
            om.active_sessions.load(Ordering::Acquire),
            om.max_concurrent - 1
        );

        let _new_session = om.enter_omnipotent(&token_id).unwrap();
        assert_eq!(
            om.active_sessions.load(Ordering::Acquire),
            om.max_concurrent
        );
    }

    #[test]
    fn test_record_action() {
        let om = OmnipotentMode::new();
        om.record_action("admin", "invoke_tool", "file_read", "success", "token-001");

        let profile = om.profile();
        assert_eq!(profile.total_actions, 1);

        // Verify the entry is in the audit log.
        let log = om.audit_log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].actor, "admin");
        assert_eq!(log[0].action, "invoke_tool");
        assert_eq!(log[0].resource, "file_read");
        assert_eq!(log[0].outcome, "success");
        assert_eq!(log[0].token_id, "token-001");
    }

    #[test]
    fn test_record_multiple_actions() {
        let om = OmnipotentMode::new();
        om.record_action("admin", "read", "/etc/config", "success", "tok-1");
        om.record_action("admin", "write", "/tmp/output", "success", "tok-1");
        om.record_action(
            "user",
            "execute",
            "diagnostic.sh",
            "failure: timeout",
            "tok-2",
        );

        let profile = om.profile();
        assert_eq!(profile.total_actions, 3);

        let log = om.audit_log.lock().unwrap();
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn test_profile_after_actions_and_escalations() {
        let om = OmnipotentMode::new();
        let t1 = om.issue_token("kate", "Task A", 60).unwrap();
        let t2 = om.issue_token("leo", "Task B", 60).unwrap();

        om.record_action("kate", "read", "db", "success", &t1);
        om.record_action("leo", "write", "fs", "success", &t2);
        om.revoke_token(&t1);

        let profile = om.profile();
        assert!(profile.enabled);
        assert_eq!(profile.total_escalations, 2);
        assert_eq!(profile.total_actions, 2);
        assert_eq!(profile.revoked_tokens, 1);
    }

    #[test]
    fn test_disable_when_last_token_revoked() {
        let om = OmnipotentMode::new();
        let t1 = om.issue_token("mallory", "Quick", 60).unwrap();
        let t2 = om.issue_token("nancy", "Quick too", 60).unwrap();

        om.revoke_token(&t1);
        // Still one valid token left -> enabled.
        assert!(om.enabled.load(Ordering::Acquire));

        om.revoke_token(&t2);
        // No valid tokens left -> disabled.
        assert!(!om.enabled.load(Ordering::Acquire));
    }

    #[test]
    fn test_default() {
        let om = OmnipotentMode::default();
        assert!(!om.enabled.load(Ordering::Acquire));
        assert_eq!(om.max_concurrent, 10);
    }

    #[test]
    fn test_omnipotent_session_debug() {
        let om = OmnipotentMode::new();
        let token_id = om.issue_token("oscar", "Debug", 60).unwrap();
        let session = om.enter_omnipotent(&token_id).unwrap();
        let debug_str = format!("{:?}", session);
        assert!(debug_str.contains(&token_id));
    }
}
