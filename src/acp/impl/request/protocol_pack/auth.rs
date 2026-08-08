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
    if !server.runtime_config.user_auth_enabled {
        // Auth disabled (local profile) — a default user-level session
        // applies; report success as before.
        return Ok(serde_json::to_value(
            crate::schema::AuthenticateResponse::new(),
        )?);
    }

    let session_manager = match server.session.session_manager.as_ref() {
        Some(sm) => sm,
        None => {
            anyhow::bail!("Session manager not initialized — cannot authenticate")
        }
    };

    let token = params
        .get("bearer_token")
        .and_then(Value::as_str)
        .or_else(|| params.get("api_key").and_then(Value::as_str))
        .filter(|t| !t.is_empty());

    match token {
        Some(token) => {
            let result = session_manager.authenticate(token);
            if result.valid {
                Ok(serde_json::to_value(
                    crate::schema::AuthenticateResponse::new(),
                )?)
            } else {
                anyhow::bail!(
                    "Authentication failed: {}",
                    result.reason.unwrap_or_else(|| "invalid token".to_string())
                )
            }
        }
        None => anyhow::bail!(
            "Authentication required: provide `bearer_token` (or `api_key`) in params"
        ),
    }
}

/// Handle `logout` — terminates the current authenticated session.
pub async fn logout_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    // B51-36: Evict tenant rate limiter state on logout if session info is present.
    #[cfg(feature = "multi-users-server")]
    {
        if let Some(session_id) = _params.get("sessionId").and_then(Value::as_str) {
            if !session_id.is_empty() {
                if let Some(ref limiter) = _server.rate_limiting.rate_limit_middleware {
                    limiter.evict_tenant(session_id);
                }
            }
        }
    }

    Ok(serde_json::to_value(&crate::schema::LogoutResponse {
        meta: None,
    })?)
}
