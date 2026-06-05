//! CORS processing for the ACP HTTP runtime
//!
//! This module contains CORS (Cross-Origin Resource Sharing) processing
//! functions extracted from `runtime.rs`. It handles:
//!
//! - Extracting header values from raw HTTP request text
//! - Computing CORS response headers for actual requests
//! - Handling CORS preflight (OPTIONS) requests
//!
//! The pure CORS config types and header-building logic remain in
//! `crate::acp::r#impl::cors`; this module bridges those helpers with
//! the runtime's HTTP connection handling.

use anyhow::Result;
use tokio::net::TcpStream;

use crate::acp::r#impl::cors::{
    build_cors_headers, build_preflight_response_headers, is_origin_allowed,
};
use crate::acp::server::AcpServer;

/// Extract a header value from raw HTTP header text (case-insensitive key).
pub(super) fn extract_header_value(headers: &str, header_name: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case(header_name) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// Compute CORS response headers for an actual (non-preflight) HTTP request.
///
/// Extracts the `Origin` header from the request, checks it against the
/// server's CORS configuration, and returns a formatted string of CORS
/// headers (each ending with `\r\n`).  Returns an empty string when CORS
/// is disabled or the origin is not allowed.
pub(super) fn compute_cors_response_headers(headers: &str, server: &AcpServer) -> String {
    let config = match server.runtime_config.cors_config() {
        Some(c) => c,
        None => return String::new(),
    };
    let origin = extract_header_value(headers, "origin");
    let cors_headers = build_cors_headers(origin.as_deref(), &config);
    if cors_headers.is_empty() {
        return String::new();
    }
    cors_headers
        .iter()
        .map(|(k, v)| format!("{}: {}\r\n", k, v))
        .collect()
}

#[allow(dead_code)] // TODO-BLUE64: Reserved for ACP runtime CORS preflight handling
/// Handle an OPTIONS (CORS preflight) request.
pub(super) async fn handle_cors_preflight(
    socket: &mut TcpStream,
    headers: &str,
    server: &AcpServer,
) -> Result<()> {
    let config = match server.runtime_config.cors_config() {
        Some(c) => c,
        None => {
            super::write_http_json_response(
                socket,
                405,
                serde_json::json!({"error": "Method Not Allowed"}),
                "",
            )
            .await?;
            return Ok(());
        }
    };
    let origin = extract_header_value(headers, "origin");
    let allow_origin = origin.as_deref().filter(|o| is_origin_allowed(o, &config));

    if allow_origin.is_none() && !config.allowed_origins.contains(&"*".to_string()) {
        super::write_http_json_response(
            socket,
            403,
            serde_json::json!({"error": "Origin not allowed"}),
            "",
        )
        .await?;
        return Ok(());
    }

    let rh = extract_header_value(headers, "access-control-request-headers");
    let preflight_headers = build_preflight_response_headers(rh.as_deref(), &config);
    let origin_val = allow_origin.unwrap_or("*").to_string();

    let mut cors_str = format!("Access-Control-Allow-Origin: {}\r\n", origin_val);
    for (k, v) in &preflight_headers {
        cors_str.push_str(&format!("{}: {}\r\n", k, v));
    }
    cors_str.push_str("Access-Control-Max-Age: ");
    cors_str.push_str(&config.max_age_seconds.to_string());
    cors_str.push_str("\r\n");

    super::write_http_json_response(socket, 200, serde_json::json!({"ok": true}), &cors_str)
        .await?;
    Ok(())
}
