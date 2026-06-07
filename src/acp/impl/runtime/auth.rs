//! Authentication and authorization for the ACP HTTP runtime
//!
//! This module contains auth-related functions extracted from `runtime.rs`.
//! It handles:
//!
//! - Extracting bearer/API-key tokens from HTTP headers
//! - RBAC-based authorization checks for incoming requests
//!
//! Entry guard token validation remains in `runtime.rs` alongside the
//! connection handling pipeline.

use anyhow::Result;
use tokio::net::TcpStream;

use crate::acp::server::AcpServer;
use crate::governance::rbac::{AccessDecision, Permission, Principal};

use super::http::write_http_json_response;
use super::protocol::extract_header_value;

/// Extract an authentication/API key token from HTTP headers.
///
/// Checks, in order: the `Authorization: Bearer <token>` header,
/// the `X-Api-Key` header, and the `X-Go-On-Key` header.
#[allow(dead_code)] // TODO-BLUE64: Reserved for ACP runtime auth pipeline
pub(super) fn extract_entry_token(headers: &str) -> Option<String> {
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

/// Check if the user session is authorized for the given request path and method.
/// Returns `Ok(true)` if a response has been written (request is handled/denied),
/// or `Ok(false)` if the request should proceed.
#[allow(dead_code)] // TODO-BLUE64: Reserved for ACP runtime auth pipeline
pub(super) async fn check_http_authorization(
    socket: &mut TcpStream,
    server: &AcpServer,
    user_session: Option<&crate::acp::r#impl::session::UserSession>,
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

    let principal = Principal::new(
        &session.user_id,
        session.roles.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        session.tenant_id.as_deref(),
    );

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

    Ok(false)
}
