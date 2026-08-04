//! Security: mTLS, TLS configuration, entry authentication, RBAC authorization
//!
//! Contains entry guard functions (token-based auth, rate limiting), RBAC
//! authorization checks, and related security infrastructure.
//! Extracted from the parent `runtime.rs` to reduce the monolithic file size.

use std::net::SocketAddr;

use anyhow::Result;
use tracing::warn;

use crate::acp::r#impl::session::UserSession;
use crate::acp::server::AcpServer;
use crate::core::error::ErrorCode;
use crate::governance::rbac::{AccessDecision, Permission, Principal};

use super::http::{write_http_json_response, HttpStream};
use super::protocol::extract_header_value;
use crate::i18n::runtime::t;

/// Apply entry guards and return `true` if the request was rejected (response already written).
pub(crate) async fn http_entry_guard(
    socket: &mut HttpStream,
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
    socket: &mut HttpStream,
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
                serde_json::json!({"error": t("error.auth_required"), "code": ErrorCode::Unauthorized}),
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
                        "error": t("error.auth_forbidden"),
                        "code": ErrorCode::Forbidden,
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
                        "error": t("error.auth_insufficient_privileges"),
                        "code": ErrorCode::Forbidden,
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
///
/// Uses i18n-aware messages for user-facing strings.
#[allow(clippy::too_many_arguments)]
async fn write_entry_rejection(
    socket: &mut HttpStream,
    status: u16,
    code: &str,
    kind: &str,
    message: String,
    source: &str,
    path: &str,
    policy: &str,
    cors_headers: &str,
) -> Result<()> {
    let trace_id = format!("entry-{}", crate::shared::timestamps::now_ts_ms());
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
///
/// Checks `Authorization: Bearer <token>` first, then falls back to
/// `X-Api-Key` and `X-Go-On-Key` headers. Shared by the ACP and MCP HTTP
/// servers (the MCP server's duplicate copy was removed).
pub(crate) fn extract_entry_token(headers: &str) -> Option<String> {
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

/// Outcome of evaluating the entry-auth guard for a request.
pub(crate) enum EntryAuthOutcome {
    /// Entry auth is disabled or the token verified — request may proceed.
    Pass,
    /// Reject the request with the given HTTP status / error code / message.
    Reject {
        status: u16,
        code: &'static str,
        message: String,
    },
}

/// Evaluate entry authentication for a request. Pure decision logic shared by
/// the ACP and MCP HTTP arms (the MCP server previously re-implemented this
/// with a local `constant_time_eq` and no rate limiting).
pub(crate) fn evaluate_entry_auth(server: &AcpServer, headers: &str) -> EntryAuthOutcome {
    if !server.runtime_config.entry_auth_enabled {
        return EntryAuthOutcome::Pass;
    }
    let env_name = server.runtime_config.entry_auth_api_key_env.trim();
    let expected_key = crate::shared::secret_override::get_secret(env_name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let Some(expected) = expected_key else {
        return EntryAuthOutcome::Reject {
            status: 503,
            code: "ENTRY_AUTH_MISCONFIGURED",
            message: format!(
                "entry auth is enabled but env '{}' is missing or empty",
                env_name
            ),
        };
    };

    let provided = extract_entry_token(headers)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    // Constant-time comparison so token verification does not leak timing
    // information about the key.
    let matches = match provided.as_deref() {
        Some(p) => {
            use subtle::ConstantTimeEq;
            p.as_bytes().ct_eq(expected.as_bytes()).into()
        }
        None => false,
    };
    if matches {
        EntryAuthOutcome::Pass
    } else {
        EntryAuthOutcome::Reject {
            status: 401,
            code: "ENTRY_AUTH_REQUIRED",
            message: t("error.entry_auth_required"),
        }
    }
}

/// Apply entry guards — token auth and rate limiting.
async fn apply_entry_guards(
    socket: &mut HttpStream,
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

    match evaluate_entry_auth(server, headers) {
        EntryAuthOutcome::Pass => {}
        EntryAuthOutcome::Reject {
            status,
            code,
            message,
        } => {
            warn!(
                "entry auth rejected {} {} from {} ({})",
                method, path, source, message
            );
            write_entry_rejection(
                socket,
                status,
                code,
                if status == 503 {
                    "service_unavailable"
                } else {
                    "unauthorized"
                },
                message,
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
        .resilience
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
            t("error.chat.rate_limited"),
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
