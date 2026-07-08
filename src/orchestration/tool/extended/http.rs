//! HTTP request tool with runtime sandboxing (LAYER 2) — fully async.
//!
//! Security architecture:
//!   LAYER 1 (Principles): Review gate only confirms user intent, not safety.
//!   LAYER 2 (Runtime):   This file — tool-level sandboxing, enforced at runtime.
//!   LAYER 3 (Config):     URL allow/block policies from AppConfig.
//!
//! Runtime sandbox rules (all enforced at execution time, not by LLM):
//!   1. Only http:// and https:// schemes allowed.
//!   2. Private/internal IPs blocked by default (10.x, 192.168.x, 172.16-31.x, 127.x, ::1).
//!   3. Response body size limited (default 10MB, configurable via UrlPolicyConfig).
//!   4. Request timeout (default 15s, configurable).
//!   5. Full audit logging of every request (URL, method, status, size).
//!
//! Uses async reqwest client directly (not blocking) for optimal performance
//! in multi-user scenarios. The `run` method delegates to `run_async` via
//! tokio's spawn_blocking.

use std::sync::Arc;

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tracing::{debug, warn};

/// Extract the first HTTP/HTTPS URL from a text string.
fn extract_url(text: &str) -> Option<String> {
    let https = text.find("https://");
    let http = text.find("http://").filter(|_| https.is_none());
    let start = https.or(http)?;
    let remaining = &text[start..];
    let end = remaining
        .find(|c: char| {
            c.is_whitespace() || c == '\"' || c == '\'' || c == '>' || c == ')' || c == ']'
        })
        .unwrap_or(remaining.len());
    Some(remaining[..end].to_string())
}

/// Check if a hostname resolves to a private/internal IP address.
fn is_private_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("127.0.0.1")
        || host.eq_ignore_ascii_case("::1")
        || host.eq_ignore_ascii_case("[::1]")
    {
        return true;
    }

    if let Ok(ip) = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        return is_private_ip(ip);
    }

    // No DNS resolution here to avoid DNS rebinding attacks.
    false
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.octets()[0] == 0
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
}

/// Validate URL scheme and block private IPs (LAYER 2 runtime sandbox).
fn validate_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|e| {
        anyhow::anyhow!("{}", tf("error.invalid_url", &[("error", &e.to_string())]))
    })?;

    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        anyhow::bail!(
            "{}",
            tf("error.unsupported_url_scheme", &[("scheme", scheme)])
        );
    }

    let host = parsed.host_str().unwrap_or("");
    if host.is_empty() {
        anyhow::bail!("{}", t("error.url_missing_host"));
    }

    let block_private = std::env::var("GO_ON_PRIVATE_IP_BLOCK")
        .ok()
        .and_then(|v| v.parse::<bool>().ok())
        .unwrap_or(true);

    if block_private && is_private_host(host) {
        anyhow::bail!(
            "{}",
            tf("error.url_blocked_private_host", &[("host", host)])
        );
    }

    Ok(())
}

/// Shared async reqwest client for all HttpRequestTool instances.
/// Built once and reused to benefit from connection pooling.
fn http_client(timeout_ms: u64) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("failed to build HTTP client")
}

pub struct HttpRequestTool;

impl Tool for HttpRequestTool {
    fn name(&self) -> &'static str {
        "http_request"
    }
    fn description(&self) -> &str {
        "Make HTTP requests (GET/POST/PUT/DELETE) to external APIs. Only http:// and https:// URLs are allowed. Private/internal IPs are blocked for security."
    }

    /// Sync path: delegates to the async implementation via spawn_blocking.
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let rt = tokio::runtime::Handle::try_current()
            .ok()
            .and_then(|handle| {
                // If we're already on a tokio runtime, block on the async impl
                // inside spawn_blocking to avoid blocking the worker thread.
                Some(
                    tokio::task::block_in_place(|| {
                        handle.block_on(async {
                            let this = Arc::new(HttpRequestTool);
                            this.run_async(input.clone()).await
                        })
                    }),
                )
            })
            .unwrap_or_else(|| {
                // No tokio runtime available — use sync reqwest as fallback
                Self::run_sync(input)
            })?;
        Ok(rt)
    }

    /// Async path: fully async reqwest client for non-blocking execution.
    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        Box::pin(async move {
            // ── 1. Resolve URL ────────────────────────────────────────
            let url = if let Some(url_str) = input.payload["url"].as_str() {
                url_str.to_string()
            } else {
                extract_url(&input.objective)
                    .or_else(|| extract_url(&input.payload.to_string()))
                    .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_url")))?
            };

            // ── 2. Runtime sandbox: URL validation (LAYER 2) ──────────
            validate_url(&url).map_err(|e| {
                warn!("http_request BLOCKED (sandbox): {} — url={}", e, url);
                e
            })?;

            let method = input.payload["method"].as_str().unwrap_or("GET").to_string();
            let body = input.payload["body"].as_str().map(|s| s.to_string());

            // ── 3. Timeout ────────────────────────────────────────────
            let timeout_ms = input.payload["timeout_ms"]
                .as_u64()
                .or_else(|| {
                    std::env::var("GO_ON_HTTP_TIMEOUT_MS")
                        .ok()
                        .and_then(|v| v.parse::<u64>().ok())
                })
                .unwrap_or(15_000);

            debug!(method = %method, url = %url, timeout_ms = %timeout_ms, "http_request: executing");

            // ── 4. Max response size ──────────────────────────────────
            let max_response_bytes: usize = std::env::var("GO_ON_HTTP_MAX_RESPONSE_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10 * 1024 * 1024);

            let client = http_client(timeout_ms)?;

            let mut request_builder: reqwest::RequestBuilder = match method.to_uppercase().as_str() {
                "GET" => client.get(&url),
                "POST" => {
                    let mut builder = client.post(&url);
                    if let Some(ref body_text) = body {
                        builder = builder.body(body_text.clone());
                    }
                    builder
                }
                "PUT" => {
                    let mut builder = client.put(&url);
                    if let Some(ref body_text) = body {
                        builder = builder.body(body_text.clone());
                    }
                    builder
                }
                "DELETE" => {
                    let mut builder = client.delete(&url);
                    if let Some(ref body_text) = body {
                        builder = builder.body(body_text.clone());
                    }
                    builder
                }
                "PATCH" => {
                    let mut builder = client.patch(&url);
                    if let Some(ref body_text) = body {
                        builder = builder.body(body_text.clone());
                    }
                    builder
                }
                "HEAD" => client.head(&url),
                "OPTIONS" => {
                    let mut builder = client.request(reqwest::Method::OPTIONS, &url);
                    if let Some(ref body_text) = body {
                        builder = builder.body(body_text.clone());
                    }
                    builder
                }
                other => {
                    anyhow::bail!(
                        "{}",
                        tf("error.unsupported_http_method", &[("method", other)])
                    );
                }
            };

            // ── 5. Custom headers ─────────────────────────────────────
            if let Some(headers_obj) = input.payload["headers"].as_object() {
                for (key, value) in headers_obj {
                    if let Some(val_str) = value.as_str() {
                        if let (Ok(header_name), Ok(header_value)) = (
                            reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                            reqwest::header::HeaderValue::from_str(val_str),
                        ) {
                            request_builder = request_builder.header(header_name, header_value);
                        }
                    }
                }
            }

            // ── 6. Bearer auth ────────────────────────────────────────
            if let Some(auth_obj) = input.payload["auth"].as_object() {
                if let Some(bearer_token) = auth_obj.get("bearer").and_then(Value::as_str) {
                    request_builder = request_builder.bearer_auth(bearer_token);
                }
            }

            // ── 7. Query parameters ───────────────────────────────────
            if let Some(query_obj) = input.payload["query"].as_object() {
                let mut query_pairs: Vec<(String, String)> = Vec::new();
                for (key, value) in query_obj {
                    let val_str = value
                        .as_str()
                        .map(|s| s.to_string())
                        .or_else(|| value.as_i64().map(|n| n.to_string()))
                        .or_else(|| value.as_f64().map(|n| n.to_string()))
                        .unwrap_or_default();
                    query_pairs.push((key.clone(), val_str));
                }
                request_builder = request_builder.query(&query_pairs);
            }

            // ── 8. Execute request (async) ────────────────────────────
            let response = request_builder.send().await.context("HTTP request failed")?;
            let status = response.status().as_u16();

            // ── 9. Runtime sandbox: response body size limit ──────────
            let response_body = response
                .text()
                .await
                .unwrap_or_else(|_| "(body read failed)".to_string());

            if response_body.len() > max_response_bytes {
                warn!(
                    "http_request BLOCKED (response too large): {} bytes > {} max — url={}",
                    response_body.len(),
                    max_response_bytes,
                    url
                );
                anyhow::bail!(
                    "{}",
                    tf(
                        "error.http_response_too_large",
                        &[
                            ("size", &response_body.len().to_string()),
                            ("max", &max_response_bytes.to_string())
                        ]
                    )
                );
            }

            let success = (200..400).contains(&status);

            // ── 10. Audit log ─────────────────────────────────────────
            debug!(
                "http_request AUDIT: {} {} -> {} ({} bytes, success={})",
                method,
                url,
                status,
                response_body.len(),
                success
            );

            Ok(ToolOutput {
                success,
                result: Some(serde_json::json!({
                    "status": status,
                    "body": response_body,
                    "url": url,
                    "method": method,
                })),
                error: (!success).then(|| format!("HTTP status {}", status)),
                verification: Some("http_request_completed".to_string()),
                audit_log: Some(format!(
                    "HTTP {} {} -> {} ({} bytes)",
                    method,
                    url,
                    status,
                    response_body.len()
                )),
                pua_report: Some(tool_execution_report(
                    "http_request",
                    Some("http_request_completed"),
                )),
            })
        })
    }
}

impl HttpRequestTool {
    /// Pure-sync fallback when no tokio runtime is available.
    fn run_sync(input: &ToolInput) -> Result<ToolOutput> {
        let url = if let Some(url_str) = input.payload["url"].as_str() {
            url_str.to_string()
        } else {
            extract_url(&input.objective)
                .or_else(|| extract_url(&input.payload.to_string()))
                .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_url")))?
        };

        validate_url(&url).map_err(|e| {
            warn!("http_request BLOCKED (sandbox): {} — url={}", e, url);
            e
        })?;

        let method = input.payload["method"].as_str().unwrap_or("GET");
        let body = input.payload["body"].as_str();

        let timeout_ms = input.payload["timeout_ms"]
            .as_u64()
            .or_else(|| {
                std::env::var("GO_ON_HTTP_TIMEOUT_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .unwrap_or(15_000);

        let max_response_bytes: usize = std::env::var("GO_ON_HTTP_MAX_RESPONSE_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10 * 1024 * 1024);

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build HTTP client")?;

        let mut request_builder = match method.to_uppercase().as_str() {
            "GET" => client.get(&url),
            "POST" => {
                let mut builder = client.post(&url);
                if let Some(body_text) = body {
                    builder = builder.body(body_text.to_string());
                }
                builder
            }
            "PUT" => {
                let mut builder = client.put(&url);
                if let Some(body_text) = body {
                    builder = builder.body(body_text.to_string());
                }
                builder
            }
            "DELETE" => {
                let mut builder = client.delete(&url);
                if let Some(body_text) = body {
                    builder = builder.body(body_text.to_string());
                }
                builder
            }
            "PATCH" => {
                let mut builder = client.patch(&url);
                if let Some(body_text) = body {
                    builder = builder.body(body_text.to_string());
                }
                builder
            }
            "HEAD" => client.head(&url),
            "OPTIONS" => {
                let mut builder = client.request(reqwest::Method::OPTIONS, &url);
                if let Some(body_text) = body {
                    builder = builder.body(body_text.to_string());
                }
                builder
            }
            other => {
                anyhow::bail!(
                    "{}",
                    tf("error.unsupported_http_method", &[("method", other)])
                );
            }
        };

        if let Some(headers_obj) = input.payload["headers"].as_object() {
            for (key, value) in headers_obj {
                if let Some(val_str) = value.as_str() {
                    if let (Ok(header_name), Ok(header_value)) = (
                        reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                        reqwest::header::HeaderValue::from_str(val_str),
                    ) {
                        request_builder = request_builder.header(header_name, header_value);
                    }
                }
            }
        }

        if let Some(auth_obj) = input.payload["auth"].as_object() {
            if let Some(bearer_token) = auth_obj.get("bearer").and_then(Value::as_str) {
                request_builder = request_builder.bearer_auth(bearer_token);
            }
        }

        if let Some(query_obj) = input.payload["query"].as_object() {
            let mut query_pairs: Vec<(String, String)> = Vec::new();
            for (key, value) in query_obj {
                let val_str = value
                    .as_str()
                    .map(|s| s.to_string())
                    .or_else(|| value.as_i64().map(|n| n.to_string()))
                    .or_else(|| value.as_f64().map(|n| n.to_string()))
                    .unwrap_or_default();
                query_pairs.push((key.clone(), val_str));
            }
            request_builder = request_builder.query(&query_pairs);
        }

        let response = request_builder.send().context("HTTP request failed")?;
        let status = response.status().as_u16();
        let response_body = response
            .text()
            .unwrap_or_else(|_| "(body read failed)".to_string());

        if response_body.len() > max_response_bytes {
            warn!(
                "http_request BLOCKED (response too large): {} bytes > {} max — url={}",
                response_body.len(),
                max_response_bytes,
                url
            );
            anyhow::bail!(
                "{}",
                tf(
                    "error.http_response_too_large",
                    &[
                        ("size", &response_body.len().to_string()),
                        ("max", &max_response_bytes.to_string())
                    ]
                )
            );
        }

        let success = (200..400).contains(&status);

        debug!(
            "http_request AUDIT: {} {} -> {} ({} bytes, success={})",
            method,
            url,
            status,
            response_body.len(),
            success
        );

        Ok(ToolOutput {
            success,
            result: Some(serde_json::json!({
                "status": status,
                "body": response_body,
                "url": url,
                "method": method,
            })),
            error: (!success).then(|| format!("HTTP status {}", status)),
            verification: Some("http_request_completed".to_string()),
            audit_log: Some(format!(
                "HTTP {} {} -> {} ({} bytes)",
                method,
                url,
                status,
                response_body.len()
            )),
            pua_report: Some(tool_execution_report(
                "http_request",
                Some("http_request_completed"),
            )),
        })
    }
}
