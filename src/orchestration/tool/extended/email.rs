//! Email (.eml) document tools
//!
//! Provides `EmailParseTool` for parsing MIME email (.eml) files using
//! only standard Rust string parsing. No external IMAP/SMTP crates required.
//! Only compiled when `feature = "document-email"` is enabled.

#[cfg(feature = "document-email")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "document-email")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "document-email")]
use anyhow::{Context, Result};
#[cfg(feature = "document-email")]
use std::fs;
#[cfg(feature = "document-email")]
use tracing::info;

// ── EmailParseTool ──────────────────────────────────────────────────────────

#[cfg(feature = "document-email")]
pub struct EmailParseTool;

#[cfg(feature = "document-email")]
impl Tool for EmailParseTool {
    fn name(&self) -> &'static str {
        "email_parse"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;

        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read EML: {}", validated.display()))?;

        info!(path = %validated.display(), bytes = content.len(), "parsing EML file");

        let (headers, body) = parse_eml(&content);

        let from = headers.get("from").cloned().unwrap_or_default();
        let to = headers.get("to").cloned().unwrap_or_default();
        let subject = headers.get("subject").cloned().unwrap_or_default();
        let date = headers.get("date").cloned().unwrap_or_default();
        let message_id = headers.get("message-id").cloned().unwrap_or_default();
        let content_type = headers.get("content-type").cloned().unwrap_or_default();

        // Collect all headers as a JSON object for completeness
        let all_headers: serde_json::Map<String, serde_json::Value> = headers
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect();

        let report = tool_execution_report("email_parse", Some("eml_parsed"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "headers": all_headers,
                "from": from,
                "to": to,
                "subject": subject,
                "date": date,
                "message_id": message_id,
                "content_type": content_type,
                "body": body,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: Some("eml_parsed".to_string()),
            audit_log: Some(format!(
                "Parsed EML '{}': subject='{}', from='{}'",
                validated.display(),
                subject,
                from,
            )),
            pua_report: Some(report),
        })
    }
}

/// Parse a raw EML string into (headers, body_text).
///
/// Splits the message at the first blank line (headers vs body).
/// Decodes header values by removing RFC 2047 encoded-word sequences
/// (`=?charset?encoding?text?=`) and unfolding folded whitespace.
/// For the body, if the content is multipart, only the first
/// `text/plain` part's content is extracted (naive boundary search).
#[cfg(feature = "document-email")]
fn parse_eml(raw: &str) -> (std::collections::HashMap<String, String>, String) {
    let mut headers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut body = String::new();

    // Find the header/body separator (first blank line)
    let separator_idx = if let Some(idx) = raw.find("\r\n\r\n") {
        idx + 4
    } else if let Some(idx) = raw.find("\n\n") {
        idx + 2
    } else {
        // No body
        let _ = raw.len();
        return (headers, body);
    };

    let header_section = &raw[..separator_idx];
    let body_section = &raw[separator_idx..];

    // Parse headers
    let mut current_header_name: Option<String> = None;
    let mut current_header_value = String::new();

    for line in header_section.lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            // Continuation of previous header (folded whitespace)
            current_header_value.push(' ');
            current_header_value.push_str(line.trim());
        } else if let Some((name, value)) = line.split_once(':') {
            // Save previous header
            if let Some(ref hname) = current_header_name {
                let decoded = decode_mime_header(&current_header_value);
                headers
                    .entry(hname.to_lowercase())
                    .or_insert_with(String::new)
                    .push_str(&decoded);
            }
            current_header_name = Some(name.trim().to_string());
            current_header_value = value.trim().to_string();
        }
    }
    // Save last header
    if let Some(ref hname) = current_header_name {
        let decoded = decode_mime_header(&current_header_value);
        headers
            .entry(hname.to_lowercase())
            .or_insert_with(String::new)
            .push_str(&decoded);
    }

    // Extract plain text body
    body = extract_plain_text_body(body_section);

    (headers, body)
}

/// Decode a single MIME encoded-word (`=?charset?encoding?text?=`) to plain text.
/// Supports both `?B?` (base64) and `?Q?` (quoted-printable) encodings.
/// Non-encoded text is returned as-is.
#[cfg(feature = "document-email")]
fn decode_mime_header(input: &str) -> String {
    let mut result = String::new();
    let mut remaining = input;

    loop {
        if let Some(start) = remaining.find("=?") {
            // Append text before the encoded word
            result.push_str(&remaining[..start]);
            let after_start = &remaining[start + 2..];

            if let Some(end) = after_start.find("?=") {
                let encoded_part = &after_start[..end + 2]; // include the trailing ?=
                let inner = &after_start[..end]; // without ?=
                remaining = &after_start[end + 2..];

                // Parse: charset?encoding?data
                let parts: Vec<&str> = inner.splitn(3, '?').collect();
                if parts.len() == 3 {
                    let _charset = parts[0];
                    let encoding = parts[1].to_uppercase();
                    let data = parts[2];

                    let decoded = match encoding.as_str() {
                        "B" => {
                            // Base64 decode
                            use base64::Engine;
                            let engine = base64::engine::general_purpose::STANDARD;
                            if let Ok(bytes) = engine.decode(data) {
                                String::from_utf8_lossy(&bytes).to_string()
                            } else {
                                encoded_part.to_string()
                            }
                        }
                        "Q" => {
                            // Quoted-printable decode
                            decode_q_encoding(data)
                        }
                        _ => encoded_part.to_string(),
                    };
                    result.push_str(&decoded);
                } else {
                    result.push_str(encoded_part);
                }
            } else {
                result.push_str(&remaining[start..]);
                break;
            }
        } else {
            result.push_str(remaining);
            break;
        }
    }

    result
}

/// Decode a quoted-printable encoded string (used in MIME `?Q?` encoding).
/// Replaces `=` followed by two hex digits with the corresponding byte,
/// and replaces `_` with space.
#[cfg(feature = "document-email")]
fn decode_q_encoding(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        if c == '=' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                } else {
                    result.push('=');
                    result.push_str(&hex);
                }
            } else {
                result.push('=');
                result.push_str(&hex);
            }
        } else if c == '_' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }

    result
}

/// Extract plain text body from a MIME message body section.
/// Handles simple text/plain bodies and multipart messages with
/// boundary markers (extracts the first text/plain part).
#[cfg(feature = "document-email")]
fn extract_plain_text_body(body_section: &str) -> String {
    // Check for multipart boundary
    if let Some(boundary) = detect_boundary(body_section) {
        // Multipart: find first text/plain part
        let parts: Vec<&str> = body_section.split(&format!("--{}", boundary)).collect();
        for part in parts {
            let part_lower = part.to_lowercase();
            if part_lower.contains("content-type: text/plain")
                || part_lower.contains("content-type:text/plain")
            {
                // Extract body of this part (skip its sub-headers)
                if let Some(body_start) = part.find("\r\n\r\n") {
                    return strip_trailing_boundary(part[body_start + 4..].trim(), &boundary);
                } else if let Some(body_start) = part.find("\n\n") {
                    return strip_trailing_boundary(part[body_start + 2..].trim(), &boundary);
                }
            }
        }
        // Fallback: return raw body trimmed
        body_section.trim().to_string()
    } else {
        // Not multipart — strip any remaining MIME headers from the body
        let trimmed = body_section.trim();
        if let Some(body_start) = trimmed.find("\r\n\r\n") {
            trimmed[body_start + 4..].trim().to_string()
        } else if let Some(body_start) = trimmed.find("\n\n") {
            trimmed[body_start + 2..].trim().to_string()
        } else {
            // No sub-headers — return as-is (but skip the initial blank line)
            trimmed
                .strip_prefix("\r\n")
                .or_else(|| trimmed.strip_prefix('\n'))
                .unwrap_or(trimmed)
                .to_string()
        }
    }
}

/// Detect the MIME boundary string from a body section.
#[cfg(feature = "document-email")]
fn detect_boundary(body_section: &str) -> Option<String> {
    // Look for `boundary="..."` in the first few lines
    for line in body_section.lines().take(20) {
        let lower = line.to_lowercase();
        if let Some(start) = lower.find("boundary=\"") {
            let after = &lower[start + 10..];
            if let Some(end) = after.find('"') {
                return Some(after[..end].to_string());
            }
        }
        if let Some(start) = lower.find("boundary=") {
            let after = &lower[start + 9..];
            // Boundary may be unquoted
            let end = after
                .find(|c: char| c.is_whitespace() || c == ';')
                .unwrap_or(after.len());
            let b = after[..end].trim_matches('"').to_string();
            if !b.is_empty() {
                return Some(b);
            }
        }
    }
    None
}

/// Strip trailing boundary marker (e.g., `--boundary--`) from a body part.
#[cfg(feature = "document-email")]
fn strip_trailing_boundary(body: &str, boundary: &str) -> String {
    let end_marker = format!("--{}--", boundary);
    let mid_marker = format!("--{}", boundary);
    let mut result = body.to_string();
    if let Some(idx) = result.find(&end_marker) {
        result.truncate(idx);
    } else if let Some(idx) = result.find(&mid_marker) {
        result.truncate(idx);
    }
    result.trim().to_string()
}
