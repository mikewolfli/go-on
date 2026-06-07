//! Security: mTLS, TLS configuration, entry authentication, RBAC authorization
//!
//! Contains entry guard functions (token-based auth, rate limiting), RBAC
//! authorization checks, and related security infrastructure.
//! Extracted from the parent `runtime.rs` to reduce the monolithic file size.

use std::net::SocketAddr;

use anyhow::Result;
use tokio::net::TcpStream;
use tracing::warn;

use crate::acp::r#impl::session::UserSession;
use crate::acp::server::AcpServer;
use crate::governance::rbac::{AccessDecision, Permission, Principal};

use super::http::write_http_json_response;
use super::protocol::extract_header_value;

/// Apply entry guards and return `true` if the request was rejected (response already written).
pub(crate) async fn http_entry_guard(
    socket: &mut TcpStream,
    server: &AcpServer,
    header_part: &str,
    method: &str,
    path: &str,
    peer_addr: SocketAddr,
    cors_headers: &str,
) -> Result<bool> {
    apply_entry_guards(
        socket,
        server,
        header_part,
        method,
        path,
        peer_addr,
        cors_headers,
    )
    .await
}

/// Check HTTP authorization (RBAC) for an incoming request.
/// Returns `true` if the request was rejected (response already written).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn check_http_authorization(
    socket: &mut TcpStream,
    server: &AcpServer,
    user_session: Option<&UserSession>,
    method: &str,
    path: &str,
    cors_headers: &str,
) -> Result<bool> {
    // If user auth is disabled, allow everything
    if !server.runtime_config.user_auth_enabled {
        return Ok(false);
    }

    // If no session, reject with 401
    let session = match user_session {
        Some(s) => s,
        None => {
            write_http_json_response(
                socket,
                401,
                serde_json::json!({"error": "Authentication required", "code": "AUTH_REQUIRED"}),
                cors_headers,
            )
            .await?;
            return Ok(true);
        }
    };

    // Exempt paths (health, root capabilities — GET only for root)
    if path == "/health" || (path == "/" && method == "GET") {
        return Ok(false);
    }

    // Map HTTP method + path to required permission
    let required_perm = match (method, path) {
        ("POST", "/rpc") => Permission::Execute,
        ("GET", _) => Permission::Read,
        ("POST", "/chat" | "/chat/stream") => Permission::Execute,
        ("POST", "/chat/completions" | "/v1/chat/completions") => Permission::Execute,
        ("POST", "/v1/responses") => Permission::Execute,
        _ => Permission::Read,
    };

    // Create principal from session
    let principal = Principal::new(
        &session.user_id,
        session.roles.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        session.tenant_id.as_deref(),
    );

    // Resolve permissions from roles
    let access_decision = server
        .governance_deps
        .rbac_enforcer
        .as_ref()
        .map(|enforcer| {
            let guard = enforcer.read().unwrap_or_else(|e| e.into_inner());
            let mut p = principal.clone();
            guard.resolve_permissions(&mut p);
            guard.check_access(&p, &required_perm)
        });

    if let Some(decision) = access_decision {
        match decision {
            AccessDecision::Allow => {
                return Ok(false);
            }
            AccessDecision::Deny { reason } => {
                write_http_json_response(
                    socket,
                    403,
                    serde_json::json!({
                        "error": "Forbidden",
                        "code": "ACCESS_DENIED",
                        "reason": reason
                    }),
                    cors_headers,
                )
                .await?;
                return Ok(true);
            }
            AccessDecision::Escalate { required_role } => {
                write_http_json_response(
                    socket,
                    403,
                    serde_json::json!({
                        "error": "Insufficient privileges",
                        "code": "PRIVILEGE_ESCALATION_REQUIRED",
                        "required_role": required_role
                    }),
                    cors_headers,
                )
                .await?;
                return Ok(true);
            }
        }
    }

    // No RBAC enforcer configured — allow (backward compat)
    Ok(false)
}

/// Check if a path is exempt from entry guards.
fn entry_guard_exempt_path(path: &str) -> bool {
    matches!(path, "/" | "/health")
}

/// Write a structured entry rejection response.
#[allow(clippy::too_many_arguments)]
async fn write_entry_rejection(
    socket: &mut TcpStream,
    status: u16,
    code: &str,
    kind: &str,
    message: String,
    source: &str,
    path: &str,
    policy: &str,
    cors_headers: &str,
) -> Result<()> {
    let trace_id = format!("entry-{}", crate::acp::prelude::now_ts_ms());
    write_http_json_response(
        socket,
        status,
        serde_json::json!({
            "ok": false,
            "error": {
                "code": code,
                "kind": kind,
                "message": message,
                "source": source,
                "path": path,
                "policy": policy,
                "trace_id": trace_id,
            }
        }),
        cors_headers,
    )
    .await
}

/// Extract the entry authentication token from request headers.
fn extract_entry_token(headers: &str) -> Option<String> {
    if let Some(auth) = extract_header_value(headers, "authorization") {
        let lower = auth.to_ascii_lowercase();
        if lower.starts_with("bearer ") {
            return Some(auth[7..].trim().to_string());
        }
    }

    extract_header_value(headers, "x-api-key")
        .or_else(|| extract_header_value(headers, "x-go-on-key"))
        .filter(|value| !value.trim().is_empty())
}

/// Apply entry guards — token auth and rate limiting.
async fn apply_entry_guards(
    socket: &mut TcpStream,
    server: &AcpServer,
    headers: &str,
    method: &str,
    path: &str,
    peer_addr: SocketAddr,
    cors_headers: &str,
) -> Result<bool> {
    if entry_guard_exempt_path(path) {
        return Ok(false);
    }

    let source = peer_addr.ip().to_string();

    if server.runtime_config.entry_auth_enabled {
        let env_name = server.runtime_config.entry_auth_api_key_env.trim();
        let expected_key = crate::shared::secret_override::get_secret(env_name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        if expected_key.is_none() {
            warn!(
                "entry auth enabled but env is missing/empty; denying {} {} from {}",
                method, path, source
            );
            write_entry_rejection(
                socket,
                503,
                "ENTRY_AUTH_MISCONFIGURED",
                "service_unavailable",
                format!(
                    "entry auth is enabled but env '{}' is missing or empty",
                    env_name
                ),
                &source,
                path,
                "entry_auth",
                cors_headers,
            )
            .await?;
            return Ok(true);
        }

        let provided = extract_entry_token(headers)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if provided != expected_key {
            warn!(
                "entry auth rejected {} {} from {} (missing or invalid key)",
                method, path, source
            );
            write_entry_rejection(
                socket,
                401,
                "ENTRY_AUTH_REQUIRED",
                "unauthorized",
                "missing or invalid entry API key".to_string(),
                &source,
                path,
                "entry_auth",
                cors_headers,
            )
            .await?;
            return Ok(true);
        }
    }

    let key = format!("entry:{}", source);
    let rpm_limit = server.runtime_config.entry_rate_limit_rpm.max(1);
    let burst = server.runtime_config.entry_rate_limit_burst.max(1);
    let allowed = server
        .phase_rate_limiter
        .lock()
        .map(|guard| guard.allow(&key, rpm_limit, Some(burst)))
        .unwrap_or(true);

    if !allowed {
        warn!(
            "entry rate limit rejected {} {} from {} (rpm={}, burst={})",
            method, path, source, rpm_limit, burst
        );
        write_entry_rejection(
            socket,
            429,
            "ENTRY_RATE_LIMITED",
            "rate_limited",
            "entry rate limit exceeded".to_string(),
            &source,
            path,
            "entry_rate_limit",
            cors_headers,
        )
        .await?;
        return Ok(true);
    }

    Ok(false)
}
