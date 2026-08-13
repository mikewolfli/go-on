use super::*;

// ── Standard ACP authentication handlers ──────────────────────────────────

/// Handle `authenticate` — authenticates the client.
///
/// Performs the real credential handshake: the `bearer_token` (or `api_key`)
/// from the request params is validated against the session manager, matching
/// the per-request auth gate (JsonRpcAuthProvider). Previously this was a
/// no-op that always returned success while the pre-dispatch auth gate made
/// the method unreachable whenever `user_auth_enabled` was set.
pub async fn authenticate_payload(server: &AcpServer, params: Value) -> Result<Value> {
    // Delegate to the unified auth middleware (JsonRpcAuthProvider) so the
    // `authenticate` method and the pre-dispatch auth gate share one
    // credential-extraction + validation chain (previously duplicated here).
    use crate::acp::r#impl::request::auth_middleware::{AuthProvider as _, JsonRpcAuthProvider};

    let provider = JsonRpcAuthProvider {
        params: &Some(params),
    };
    match provider.authenticate(server) {
        Ok(_session) => {
            // Auth disabled (returns Ok(None)) or a valid session (Ok(Some)):
            // both report success. Auth failure surfaces as Err below.
            Ok(serde_json::to_value(
                crate::schema::AuthenticateResponse::new(),
            )?)
        }
        Err(reason) => anyhow::bail!("Authentication failed: {reason}"),
    }
}

/// Handle `logout` — terminates the current authenticated session.
pub async fn logout_payload(server: &AcpServer, params: Value) -> Result<Value> {
    // P1: revoke the bearer token presented at login so `logout` actually
    // terminates the session. The param key matches the auth middleware's
    // `bearer_token` (auth_middleware.rs); previously logout only evicted
    // tenant rate-limiter state and left the token valid. When auth is
    // disabled no session manager exists, so this is a no-op.
    if let Some(token) = params.get("bearer_token").and_then(Value::as_str) {
        if !token.is_empty() {
            if let Some(sm) = server.session.session_manager.as_ref() {
                let revoked = sm.revoke_session(token);
                warn!(
                    revoked = revoked,
                    "logout: bearer token revoked (session terminated)"
                );
            }
        }
    }

    // B51-36: Evict tenant rate limiter state on logout if session info is present.
    #[cfg(feature = "multi-users-server")]
    {
        if let Some(session_id) = params.get("sessionId").and_then(Value::as_str) {
            if !session_id.is_empty() {
                if let Some(ref limiter) = server.rate_limiting.rate_limit_middleware {
                    limiter.evict_tenant(session_id);
                }
            }
        }
    }

    Ok(serde_json::to_value(&crate::schema::LogoutResponse {
        meta: None,
    })?)
}
