//! Protocol negotiation, HTTP request parsing, and version handshake
//!
//! Contains low-level HTTP request parsing functions and adaptive signal
//! inference used to determine which protocol variant a client is speaking.
//! Extracted from the parent `runtime.rs` to reduce the monolithic file size.

use anyhow::Result;

/// A parsed HTTP request with its components split for routing.
pub(crate) struct ParsedHttpRequest<'a> {
    pub(crate) method: &'a str,
    pub(crate) path: &'a str,
    pub(crate) header_part: &'a str,
    pub(crate) body_initial_part: &'a str,
}

/// Parse a raw HTTP request text into its components.
pub(crate) fn parse_http_request(request_text: &str) -> Result<ParsedHttpRequest<'_>> {
    let header_end = request_text
        .find("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing header terminator"))?;

    let (header_part, body_initial_part) = request_text.split_at(header_end + 4);
    let request_line = header_part
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing request line"))?;

    let mut request_line_parts = request_line.split_whitespace();
    let method = request_line_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing method"))?;
    let path = request_line_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing path"))?;
    Ok(ParsedHttpRequest {
        method,
        path,
        header_part,
        body_initial_part,
    })
}

/// Extract the Content-Length value from HTTP headers.
pub(crate) fn extract_content_length(headers: &str) -> Option<usize> {
    let mut found: Option<usize> = None;
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-length") {
            continue;
        }
        let val: usize = value.trim().parse().ok()?;
        match found {
            None => found = Some(val),
            Some(prev) if prev == val => {} // duplicate with same value — OK
            Some(_) => return None,         // different values — reject per RFC 7230
        }
    }
    found
}

/// Extract the value of a named header from raw HTTP headers.
pub(crate) fn extract_header_value(headers: &str, header_name: &str) -> Option<String> {
    extract_header_values(headers, header_name)
        .into_iter()
        .next()
}

/// Extract all values of a named header (case-insensitive name match).
///
/// Single source of raw HTTP header parsing — used by the session auth
/// (`SessionManager::extract_user_from_request`) and the entry auth guard
/// (`extract_entry_token`), so header name casing is handled identically on
/// both ACP and MCP HTTP arms.
pub(crate) fn extract_header_values(headers: &str, header_name: &str) -> Vec<String> {
    headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case(header_name) {
                Some(value.trim().to_string())
            } else {
                None
            }
        })
        .collect()
}
