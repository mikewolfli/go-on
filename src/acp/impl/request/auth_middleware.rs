//! Unified authentication middleware (B51-29).
//!
//! Provides an `AuthProvider` trait that abstracts over JSON-RPC param-based
//! authentication (used by stdio/JSON-RPC) and HTTP header-based authentication
//! (used by the HTTP server).  The `AuthMiddleware` struct wraps the common
//! "check if auth is enabled → delegate to provider → enforce" logic so that
//! both auth paths converge on a single implementation.

use crate::acp::r#impl::session::UserSession;
use crate::acp::server::AcpServer;
use serde_json::Value;

// ── AuthProvider trait ────────────────────────────────────────────────────

/// Pluggable authentication strategy.
///
/// Implementations extract credentials from the transport-specific source and
/// return an authenticated [`UserSession`] (or a rejection).
pub trait AuthProvider: Send + Sync {
    /// Try to authenticate and return a session.
    ///
    /// Returns `Ok(Some(session))` on success, `Ok(None)` when auth is disabled
    /// (no session needed), or `Err(reason)` on authentication failure.
    fn authenticate(&self, server: &AcpServer) -> Result<Option<UserSession>, String>;
}

// ── Concrete providers ────────────────────────────────────────────────────

/// Authenticates via JSON-RPC request params (`bearer_token` or `api_key` fields).
pub struct JsonRpcAuthProvider<'a> {
    pub params: &'a Option<Value>,
}

impl AuthProvider for JsonRpcAuthProvider<'_> {
    fn authenticate(&self, server: &AcpServer) -> Result<Option<UserSession>, String> {
        // If user auth is disabled, allow everything (backward compatible)
        if !server.runtime_config.user_auth_enabled {
            return Ok(None);
        }

        let session_manager = match server.session_manager.as_ref() {
            Some(sm) => sm,
            None => {
                return Err("Session manager not initialized".into());
            }
        };

        if let Some(params) = self.params {
            if let Some(token) = params.get("bearer_token").and_then(|v| v.as_str()) {
                let result = session_manager.authenticate(token);
                if result.valid {
                    return Ok(result.session);
                }
                return Err(result.reason.unwrap_or_else(|| "Invalid token".into()));
            }
            if let Some(api_key) = params.get("api_key").and_then(|v| v.as_str()) {
                let result = session_manager.authenticate(api_key);
                if result.valid {
                    return Ok(result.session);
                }
                return Err(result.reason.unwrap_or_else(|| "Invalid API key".into()));
            }
        }

        Err("Authentication required".into())
    }
}

/// Authenticates via HTTP headers (Bearer token, X-API-Key, or session cookie).
// activated, formerly F-GAP-51 — HTTP path now passes headers to authenticate_request
pub struct HttpAuthProvider<'a> {
    pub headers: &'a str,
}

impl AuthProvider for HttpAuthProvider<'_> {
    fn authenticate(&self, server: &AcpServer) -> Result<Option<UserSession>, String> {
        // If user auth is disabled, allow everything
        if !server.runtime_config.user_auth_enabled {
            return Ok(None);
        }

        let session_manager = match server.session_manager.as_ref() {
            Some(sm) => sm,
            None => {
                return Err("Session manager not initialized".into());
            }
        };

        // Delegate to SessionManager's header parsing
        match session_manager.extract_user_from_request(self.headers) {
            Some(session) => Ok(Some(session)),
            None => Err("Authentication required".into()),
        }
    }
}

// ── AuthMiddleware ────────────────────────────────────────────────────────

/// Middleware that applies the auth gate: check provider → enforce or allow.
pub struct AuthMiddleware;

impl AuthMiddleware {
    /// Run the auth gate.
    ///
    /// Returns `Ok(Some(session))` when authenticated, `Ok(None)` when auth is
    /// disabled, or `Err(reason)` when authentication failed.
    pub fn authenticate(
        provider: &dyn AuthProvider,
        server: &AcpServer,
    ) -> Result<Option<UserSession>, String> {
        provider.authenticate(server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::r#impl::session::{AuthConfig, SessionManager};
    use std::sync::Arc;

    fn test_server_with_auth(enabled: bool) -> AcpServer {
        // Minimal server with auth config
        let builder = crate::acp::server::ServerBuilder::new();
        let mut server = builder.build().expect("test server");
        server.runtime_config.user_auth_enabled = enabled;
        if enabled {
            let auth_cfg = AuthConfig::from(&server.runtime_config);
            server.session_manager = Some(Arc::new(SessionManager::with_auth_config(auth_cfg)));
        }
        server
    }

    #[test]
    fn test_json_rpc_auth_disabled() {
        let server = test_server_with_auth(false);
        let provider = JsonRpcAuthProvider { params: &None };
        let result = AuthMiddleware::authenticate(&provider, &server);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_json_rpc_auth_missing_token() {
        let server = test_server_with_auth(true);
        let params = Some(serde_json::json!({}));
        let provider = JsonRpcAuthProvider { params: &params };
        let result = AuthMiddleware::authenticate(&provider, &server);
        assert!(result.is_err());
    }
}
