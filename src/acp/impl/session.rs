//! User session management module for multi-user ACP server.
//!
//! Provides token-based authentication, session lifecycle management,
//! and automatic cleanup of expired sessions.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Represents an authenticated user session.
#[derive(Clone, Debug)]
pub struct UserSession {
    /// Unique user identifier (from token subject).
    pub user_id: String,
    /// Assigned roles (e.g., `["admin"]`, `["user"]`).
    pub roles: Vec<String>,
    /// Multi-tenant isolation – `None` means single-tenant.
    pub tenant_id: Option<String>,
    /// Resolved permission strings.
    pub permissions: Vec<String>,
    /// Token issuance timestamp (epoch milliseconds).
    pub issued_at: i64,
    /// Token expiration timestamp (epoch milliseconds).
    pub expires_at: i64,
    /// Token type: `"bearer"` or `"pat"` (personal access token).
    pub token_type: String,
}

/// Outcome of a token introspection (validation) call.
#[derive(Clone, Debug)]
pub struct TokenIntrospectResult {
    /// Whether the token is valid.
    pub valid: bool,
    /// The user session if the token is valid.
    pub session: Option<UserSession>,
    /// Human-readable denial reason when the token is not valid.
    pub reason: Option<String>,
}

/// Mutable inner state of the session manager.
#[derive(Clone, Debug)]
pub struct SessionManagerInner {
    /// Active sessions keyed by token string.
    pub sessions: HashMap<String, UserSession>,
    /// HMAC secret used for token signing and verification.
    pub token_secret: String,
}

/// Auth-related configuration fields extracted from [`RuntimeConfig`].
#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// Master switch: when `false`, all requests are treated as admin.
    pub user_auth_enabled: bool,
    /// HMAC secret for signing tokens.
    pub user_auth_token_secret: String,
    /// Token TTL in seconds (default: 86400 = 24 h).
    pub user_auth_token_ttl_seconds: u64,
    /// Auto-create sessions on valid token (default: `true`).
    pub user_auth_auto_provision: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            user_auth_enabled: false,
            user_auth_token_secret: String::new(),
            user_auth_token_ttl_seconds: 86_400,
            user_auth_auto_provision: true,
        }
    }
}

impl From<&RuntimeConfig> for AuthConfig {
    fn from(cfg: &RuntimeConfig) -> Self {
        // Resolve the token secret: env var override > config file > default.
        let secret = crate::shared::secret_override::get_secret(&cfg.user_auth_token_secret_env)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| cfg.user_auth_token_secret.clone());

        Self {
            user_auth_enabled: cfg.user_auth_enabled,
            user_auth_token_secret: secret,
            user_auth_token_ttl_seconds: cfg.user_auth_token_ttl_seconds,
            user_auth_auto_provision: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Session manager
// ---------------------------------------------------------------------------

/// Manages user sessions: token issuance, validation, revocation, and
/// periodic cleanup of expired entries.
pub struct SessionManager {
    inner: Arc<RwLock<SessionManagerInner>>,
    auth_cfg: AuthConfig,
}

impl SessionManager {
    /// Create a new [`SessionManager`] with the given HMAC secret.
    ///
    /// The secret is used for both signing newly issued tokens and verifying
    /// incoming tokens.
    pub fn new(token_secret: String) -> Self {
        let inner = SessionManagerInner {
            sessions: HashMap::new(),
            token_secret,
        };
        Self {
            inner: Arc::new(RwLock::new(inner)),
            auth_cfg: AuthConfig::default(),
        }
    }

    /// Create a new [`SessionManager`] from an [`AuthConfig`].
    pub fn with_auth_config(auth_cfg: AuthConfig) -> Self {
        let secret = auth_cfg.user_auth_token_secret.clone();
        let inner = SessionManagerInner {
            sessions: HashMap::new(),
            token_secret: secret,
        };
        Self {
            inner: Arc::new(RwLock::new(inner)),
            auth_cfg,
        }
    }

    // ------------------------------------------------------------------
    // Core operations
    // ------------------------------------------------------------------

    /// Authenticate a bearer token and return a [`TokenIntrospectResult`].
    ///
    /// When `user_auth_enabled` is `false`, a default admin session is
    /// returned without any token verification.  This maintains backward
    /// compatibility for single-user deployments.
    pub fn authenticate(&self, token: &str) -> TokenIntrospectResult {
        if !self.auth_cfg.user_auth_enabled {
            let admin_session = UserSession {
                user_id: "admin".into(),
                roles: vec!["admin".into()],
                tenant_id: None,
                permissions: vec!["*".into()],
                issued_at: now_ms(),
                expires_at: now_ms() + 86_400_000,
                token_type: "bearer".into(),
            };
            return TokenIntrospectResult {
                valid: true,
                session: Some(admin_session),
                reason: None,
            };
        }

        // First try the in-memory cache.
        {
            let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
            if let Some(session) = inner.sessions.get(token) {
                if session.expires_at > now_ms() {
                    return TokenIntrospectResult {
                        valid: true,
                        session: Some(session.clone()),
                        reason: None,
                    };
                }
            }
        }

        // Fall back to validating the token from scratch.
        self.validate_token(token)
    }

    /// Issue a new token for the given user and register the session.
    pub fn issue_token(
        &self,
        user_id: &str,
        roles: &[&str],
        tenant_id: Option<&str>,
        ttl_seconds: u64,
    ) -> Result<String, String> {
        let issued_at = now_ms();
        let expires_at = issued_at + (ttl_seconds as i64 * 1000);
        let token = self.build_token(user_id, expires_at)?;

        let session = UserSession {
            user_id: user_id.to_string(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            tenant_id: tenant_id.map(|s| s.to_string()),
            permissions: Vec::new(),
            issued_at,
            expires_at,
            token_type: "bearer".into(),
        };

        {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            inner.sessions.insert(token.clone(), session);
        }

        Ok(token)
    }

    /// Revoke **all** sessions belonging to a user.
    ///
    /// Returns `true` if at least one session was removed.
    pub fn revoke_token(&self, user_id: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let before = inner.sessions.len();
        inner.sessions.retain(|_, s| s.user_id != user_id);
        inner.sessions.len() < before
    }

    /// Revoke a single session identified by its token string.
    ///
    /// Returns `true` if the session existed and was removed.
    pub fn revoke_session(&self, token: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.sessions.remove(token).is_some()
    }

    /// Validate a token by parsing its components, verifying the HMAC
    /// signature, and checking expiration.
    pub fn validate_token(&self, token: &str) -> TokenIntrospectResult {
        // Parse token: user_id:base64_hmac:expires_at_ms
        let parts: Vec<&str> = token.split(':').collect();
        if parts.len() != 3 {
            return TokenIntrospectResult {
                valid: false,
                session: None,
                reason: Some("malformed token: expected 3 colon-delimited parts".into()),
            };
        }

        let user_id = parts[0];
        let provided_sig = parts[1];
        let expires_at_str = parts[2];

        let expires_at: i64 = match expires_at_str.parse() {
            Ok(ts) => ts,
            Err(_) => {
                return TokenIntrospectResult {
                    valid: false,
                    session: None,
                    reason: Some("invalid expiration timestamp".into()),
                };
            }
        };

        // Check expiration.
        if expires_at <= now_ms() {
            return TokenIntrospectResult {
                valid: false,
                session: None,
                reason: Some("token expired".into()),
            };
        }

        // Verify HMAC signature.
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let secret = &inner.token_secret;
        let payload = format!("{}:{}", user_id, expires_at_str);
        let expected_sig = hmac_sha256_b64(secret.as_bytes(), payload.as_bytes());

        if provided_sig != expected_sig {
            return TokenIntrospectResult {
                valid: false,
                session: None,
                reason: Some("invalid token signature".into()),
            };
        }

        // Look up the session – if not found and auto-provision is enabled,
        // fabricate a minimal session.
        let session = inner.sessions.get(token).cloned().or_else(|| {
            if self.auth_cfg.user_auth_auto_provision {
                Some(UserSession {
                    user_id: user_id.to_string(),
                    roles: vec!["user".into()],
                    tenant_id: None,
                    permissions: Vec::new(),
                    issued_at: expires_at
                        - (self.auth_cfg.user_auth_token_ttl_seconds as i64 * 1000),
                    expires_at,
                    token_type: "bearer".into(),
                })
            } else {
                None
            }
        });

        let valid = session.is_some();
        let reason = if !valid {
            Some("session not found and auto-provision is disabled".into())
        } else {
            None
        };

        TokenIntrospectResult {
            valid,
            session,
            reason,
        }
    }

    /// Attempt to extract a Bearer token from the `Authorization` header and
    /// authenticate it.
    pub fn extract_user_from_request(&self, headers: &str) -> Option<UserSession> {
        // Look for "Authorization: Bearer <token>" – we do a simple scan.
        for line in headers.lines() {
            let trimmed = line.trim();
            if let Some(token) = trimmed
                .strip_prefix("Authorization:")
                .or_else(|| trimmed.strip_prefix("authorization:"))
            {
                let token = token.trim();
                if let Some(bearer_token) = token
                    .strip_prefix("Bearer ")
                    .or_else(|| token.strip_prefix("bearer "))
                {
                    let result = self.authenticate(bearer_token);
                    if result.valid {
                        return result.session;
                    }
                }
            }
        }
        None
    }

    /// Remove all sessions that have expired.
    pub fn cleanup_expired(&self) -> usize {
        let now = now_ms();
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let before = inner.sessions.len();
        inner.sessions.retain(|_, s| s.expires_at > now);
        before - inner.sessions.len()
    }

    /// Return a reference-counted handle to the inner state (useful for
    /// sharing across tasks).
    pub fn inner(&self) -> Arc<RwLock<SessionManagerInner>> {
        self.inner.clone()
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Build a signed token string: `user_id:base64_hmac:expires_at_ms`.
    fn build_token(&self, user_id: &str, expires_at: i64) -> Result<String, String> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let payload = format!("{}:{}", user_id, expires_at);
        let sig = hmac_sha256_b64(inner.token_secret.as_bytes(), payload.as_bytes());
        Ok(format!("{}:{}:{}", user_id, sig, expires_at))
    }
}

// ---------------------------------------------------------------------------
// HMAC-SHA256 helper (no external `hmac` crate dependency)
// ---------------------------------------------------------------------------

/// Compute an HMAC-SHA256 digest over `data` with the given `key` and return
/// the result as a base64-encoded string.
fn hmac_sha256_b64(key: &[u8], data: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine;

    let hmac_bytes = hmac_sha256(key, data);
    BASE64_STANDARD.encode(&hmac_bytes)
}

/// Standard HMAC-SHA256 construction using the `sha2` crate.
///
/// Implements RFC 2104:
///   HMAC(K, m) = H((K' ⊕ opad) || H((K' ⊕ ipad) || m))
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    const BLOCK_SIZE: usize = 64;

    let mut key = key.to_vec();

    // Hash key if it is longer than the block size.
    if key.len() > BLOCK_SIZE {
        key = Sha256::digest(&key).to_vec();
    }

    // Pad key to block size.
    key.resize(BLOCK_SIZE, 0);

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];

    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key[i];
        opad[i] ^= key[i];
    }

    let inner_hash = Sha256::digest([&ipad[..], data].concat());
    let result = Sha256::digest([&opad[..], &inner_hash[..]].concat());

    result.to_vec()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the current timestamp in milliseconds since the Unix epoch.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn test_manager() -> SessionManager {
        let auth_cfg = AuthConfig {
            user_auth_enabled: true,
            user_auth_token_secret: "test-secret-key".into(),
            ..Default::default()
        };
        SessionManager::with_auth_config(auth_cfg)
    }

    // ------------------------------------------------------------------
    // Token issuance and validation
    // ------------------------------------------------------------------

    #[test]
    fn test_issue_and_validate_token() {
        let mgr = test_manager();
        let token = mgr
            .issue_token("alice", &["user", "editor"], None, 3600)
            .expect("should issue token");

        let result = mgr.validate_token(&token);
        assert!(result.valid, "token should be valid");
        let session = result.session.expect("should have session");
        assert_eq!(session.user_id, "alice");
        assert_eq!(session.roles, vec!["user", "editor"]);
        assert!(session.expires_at > now_ms());
    }

    // ------------------------------------------------------------------
    // Token expiration
    // ------------------------------------------------------------------

    #[test]
    fn test_token_expiration() {
        let mgr = test_manager();
        // Token with 1-second TTL.
        let token = mgr
            .issue_token("bob", &["user"], None, 1)
            .expect("should issue token");

        // Should be valid immediately.
        let result = mgr.validate_token(&token);
        assert!(result.valid, "token should be valid right after issuance");

        // Wait for it to expire.
        thread::sleep(Duration::from_millis(1100));

        let result = mgr.validate_token(&token);
        assert!(!result.valid, "token should be expired");
        assert_eq!(result.reason.unwrap_or_default(), "token expired");
    }

    // ------------------------------------------------------------------
    // Invalid signature rejection
    // ------------------------------------------------------------------

    #[test]
    fn test_invalid_signature_rejection() {
        let mgr = test_manager();
        let token = mgr
            .issue_token("charlie", &["user"], None, 3600)
            .expect("should issue token");

        // Tamper with the signature portion.
        // Token format is user_id:base64_sig:expires_at_ms
        // Split into parts and replace the sig with garbage.
        let parts: Vec<&str> = token.splitn(3, ':').collect();
        assert_eq!(parts.len(), 3, "expected 3-part token");
        let tampered = format!("{}:tampered-sig:{}", parts[0], parts[2]);

        let result = mgr.validate_token(&tampered);
        assert!(!result.valid, "tampered token should be rejected");
        assert_eq!(result.reason.unwrap_or_default(), "invalid token signature");
    }

    // ------------------------------------------------------------------
    // Default admin session when auth disabled
    // ------------------------------------------------------------------

    #[test]
    fn test_default_admin_session_when_auth_disabled() {
        let auth_cfg = AuthConfig {
            user_auth_enabled: false,
            ..Default::default()
        };
        let mgr = SessionManager::with_auth_config(auth_cfg);

        // Any token (even garbage) should yield an admin session.
        let result = mgr.authenticate("some-random-token");
        assert!(result.valid, "should be valid when auth disabled");
        let session = result.session.expect("should have a session");
        assert_eq!(session.user_id, "admin");
        assert_eq!(session.roles, vec!["admin"]);
    }

    // ------------------------------------------------------------------
    // User role assignment
    // ------------------------------------------------------------------

    #[test]
    fn test_user_role_assignment() {
        let mgr = test_manager();

        // Issue token with custom roles.
        let token = mgr
            .issue_token("dave", &["admin", "superuser"], Some("tenant-42"), 3600)
            .expect("should issue token");

        let result = mgr.validate_token(&token);
        assert!(result.valid);
        let session = result.session.unwrap();
        assert_eq!(session.user_id, "dave");
        assert_eq!(session.roles, vec!["admin", "superuser"]);
        assert_eq!(session.tenant_id, Some("tenant-42".into()));
    }

    // ------------------------------------------------------------------
    // Token revocation (all sessions for a user)
    // ------------------------------------------------------------------

    #[test]
    fn test_revoke_token_for_user() {
        let mgr = test_manager();

        let t1 = mgr.issue_token("eve", &["user"], None, 3600).unwrap();
        let t2 = mgr.issue_token("eve", &["admin"], None, 3600).unwrap();
        let t3 = mgr.issue_token("frank", &["user"], None, 3600).unwrap();

        // Revoke all of eve's sessions.
        assert!(
            mgr.revoke_token("eve"),
            "should remove at least one session"
        );

        // Eve's tokens should no longer be present in the store.
        // (validate_token auto-provisions a fresh session, so the cached entry is gone.)
        let inner = mgr.inner.read().unwrap();
        assert!(!inner.sessions.contains_key(&t1));
        assert!(!inner.sessions.contains_key(&t2));
        assert!(inner.sessions.contains_key(&t3));
    }

    // ------------------------------------------------------------------
    // Single session revocation
    // ------------------------------------------------------------------

    #[test]
    fn test_revoke_single_session() {
        let mgr = test_manager();

        let token = mgr.issue_token("grace", &["user"], None, 3600).unwrap();
        assert!(mgr.revoke_session(&token), "should remove the session");
        assert!(
            !mgr.revoke_session(&token),
            "second removal should return false"
        );

        let inner = mgr.inner.read().unwrap();
        assert!(!inner.sessions.contains_key(&token));
    }

    // ------------------------------------------------------------------
    // Extract user from request headers
    // ------------------------------------------------------------------

    #[test]
    fn test_extract_user_from_request() {
        let mgr = test_manager();
        let token = mgr.issue_token("hank", &["user"], None, 3600).unwrap();

        let headers = format!(
            "Authorization: Bearer {}\r\nContent-Type: application/json",
            token
        );
        let session = mgr.extract_user_from_request(&headers);
        assert!(session.is_some());
        assert_eq!(session.unwrap().user_id, "hank");
    }

    #[test]
    fn test_extract_user_missing_header() {
        let mgr = test_manager();
        let headers = "Content-Type: application/json\r\nX-Custom: value";
        let session = mgr.extract_user_from_request(headers);
        assert!(session.is_none());
    }

    #[test]
    fn test_extract_user_wrong_auth_scheme() {
        let mgr = test_manager();
        let headers = "Authorization: Basic dXNlcjpwYXNz\r\n";
        let session = mgr.extract_user_from_request(headers);
        assert!(session.is_none());
    }

    // ------------------------------------------------------------------
    // Cleanup expired sessions
    // ------------------------------------------------------------------

    #[test]
    fn test_cleanup_expired_sessions() {
        let mgr = test_manager();
        let _alice = mgr.issue_token("alice", &["user"], None, 3600).unwrap();
        let _bob = mgr.issue_token("bob", &["user"], None, 1).unwrap();

        // Bob's token will expire after ~1 s.
        thread::sleep(Duration::from_millis(1100));

        let removed = mgr.cleanup_expired();
        assert_eq!(removed, 1, "should remove bob's expired session");
    }

    // ------------------------------------------------------------------
    // Malformed tokens
    // ------------------------------------------------------------------

    #[test]
    fn test_malformed_token() {
        let mgr = test_manager();
        let result = mgr.validate_token("too-few-parts");
        assert!(!result.valid);
        assert!(result.reason.unwrap().contains("malformed"));

        let result2 = mgr.validate_token("a:b:c:d:extra");
        assert!(!result2.valid);
        assert!(result2.reason.unwrap().contains("malformed"));
    }

    #[test]
    fn test_token_with_invalid_expiry() {
        let mgr = test_manager();
        let result = mgr.validate_token("alice:somesig:notanumber");
        assert!(!result.valid);
    }

    // ------------------------------------------------------------------
    // Auth config from RuntimeConfig
    // ------------------------------------------------------------------

    #[test]
    fn test_auth_config_from_runtime_config() {
        let cfg = RuntimeConfig {
            user_auth_enabled: true,
            user_auth_token_secret: "my-secret".into(),
            user_auth_token_ttl_seconds: 7200,
            ..Default::default()
        };

        let auth_cfg = AuthConfig::from(&cfg);
        assert!(auth_cfg.user_auth_enabled);
        assert_eq!(auth_cfg.user_auth_token_secret, "my-secret");
        assert_eq!(auth_cfg.user_auth_token_ttl_seconds, 7200);
    }
}
