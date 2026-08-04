//! TLS/mTLS HTTP connection handling
//!
//! The former TLS-specific HTTP router (`handle_tls_http_stream` and friends)
//! was removed in the round-23 HTTP unification: the accept loop in
//! `http_server.rs` now performs the TLS handshake and wraps the stream in
//! [`HttpStream`](super::http::HttpStream), then routes through the same
//! `handle_http_connection` as plaintext. This closed the behavior forks
//! (TLS `/chat` was 501, `/v1/chat/completions` was mis-routed into the
//! ChatParams SSE path, and TLS skipped entry auth / RBAC entirely).
//!
//! The only remaining item here is the root capabilities payload, shared by
//! the plaintext and TLS routing.

/// Build the root capabilities response payload.
pub(crate) fn build_root_capabilities_response() -> serde_json::Value {
    serde_json::json!({
        "service": "go-on",
        "protocol": "acp-http",
        "health": "/health",
        "endpoints": {
            "chat": ["/chat", "/chat/stream"],
            "openai": ["/v1/models", "/v1/model", "/models", "/v1/chat/completions", "/chat/completions"],
            "responses": ["/v1/responses", "/v1/responses/{id}"],
        }
    })
}
