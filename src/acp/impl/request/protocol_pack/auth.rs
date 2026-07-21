use super::*;

// ── Standard ACP authentication handlers ──────────────────────────────────

/// Handle `authenticate` — authenticates the client.
/// Standard ACP: client sends `methodId`, agent performs auth and returns success.
pub async fn authenticate_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Ok(serde_json::to_value(
        crate::schema::AuthenticateResponse::new(),
    )?)
}

/// Handle `logout` — terminates the current authenticated session.
pub async fn logout_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    // B51-36: Evict tenant rate limiter state on logout if session info is present.
    #[cfg(feature = "multi-users-server")]
    {
        if let Some(session_id) = _params.get("sessionId").and_then(Value::as_str) {
            if !session_id.is_empty() {
                if let Some(ref limiter) = _server.rate_limiting.rate_limit_middleware {
                    limiter.evict_tenant(session_id).await;
                }
            }
        }
    }

    Ok(serde_json::to_value(&crate::schema::LogoutResponse {
        meta: None,
    })?)
}
