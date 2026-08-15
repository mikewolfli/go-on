//! HTTP request tool with runtime sandboxing (LAYER 2 + LAYER 3) — fully async.
//!
//! Security architecture:
//!   LAYER 1 (Principles): Review gate only confirms user intent, not safety.
//!   LAYER 2 (Runtime):   This file — tool-level sandboxing, enforced at runtime.
//!   LAYER 3 (Config):     URL allow/block policies from `UrlPolicyConfig`.
//!
//! Runtime sandbox rules (all enforced at execution time, not by LLM):
//!   1. Only http:// and https:// schemes allowed.
//!   2. Private/internal IPs blocked by default (10.x, 192.168.x, 172.16-31.x, 127.x, ::1).
//!   3. Response body size limited (default 10MB, configurable via UrlPolicyConfig).
//!   4. Request timeout (default 15s, configurable).
//!   5. Full audit logging of every request (URL, method, status, size).
//!   6. URL allow/block patterns from config (`allowed_patterns` / `blocked_patterns`).
//!
//! The `UrlPolicyConfig` is loaded at server startup and stored in a global OnceLock.
//! Call `init_url_policy(config)` once during server initialization.

use std::sync::{Arc, LazyLock, OnceLock};

use crate::config::UrlPolicyConfig;
use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tracing::{debug, warn};

// ── Global URL policy config (LAYER 3) ───────────────────────────────
// Initialized once at server startup from AppConfig::SecurityConfig::url_policy.
// Falls back to defaults if not initialized.
static URL_POLICY_OVERRIDE: OnceLock<UrlPolicyConfig> = OnceLock::new();
static URL_POLICY_DEFAULT: LazyLock<UrlPolicyConfig> = LazyLock::new(|| UrlPolicyConfig {
    max_response_bytes: 10 * 1024 * 1024,
    block_private_ips: true,
    restrict_to_allowed: false,
    allowed_patterns: Vec::new(),
    blocked_patterns: Vec::new(),
});

/// Initialize the global URL policy config from AppConfig.
/// Must be called once during server startup.
pub fn init_url_policy(config: UrlPolicyConfig) {
    let _ = URL_POLICY_OVERRIDE.set(config);
    if let Some(p) = URL_POLICY_OVERRIDE.get() {
        tracing::info!(
            "http_request: URL policy initialized (max_response_bytes={}, block_private_ips={}, restrict_to_allowed={}, allowed_patterns={}, blocked_patterns={})",
            p.max_response_bytes,
            p.block_private_ips,
            p.restrict_to_allowed,
            p.allowed_patterns.len(),
            p.blocked_patterns.len(),
        );
    }
}

/// Get the effective URL policy.
fn url_policy() -> &'static UrlPolicyConfig {
    URL_POLICY_OVERRIDE
        .get()
        .unwrap_or_else(|| &URL_POLICY_DEFAULT)
}

/// Read a blocking response body with the policy size cap enforced DURING
/// the read (content-length pre-check + `Read::take`), so a huge response is
/// rejected without ever being fully buffered. Shared by http_request's sync
/// path, web_scrape and rss_read.
pub(crate) fn read_blocking_body_capped(
    response: &mut reqwest::blocking::Response,
    url: &str,
) -> Result<Vec<u8>> {
    use std::io::Read;
    let max_bytes = url_policy().max_response_bytes;
    if let Some(declared) = response.content_length() {
        if declared > max_bytes as u64 {
            warn!(
                "http_request BLOCKED (response too large): declared {} bytes > {} max — url={}",
                declared, max_bytes, url
            );
            anyhow::bail!(
                "{}",
                tf(
                    "error.http_response_too_large",
                    &[
                        ("size", &declared.to_string()),
                        ("max", &max_bytes.to_string())
                    ]
                )
            );
        }
    }
    let mut body_bytes: Vec<u8> = Vec::new();
    let read = response
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut body_bytes)
        .with_context(|| format!("failed to read response body from {url}"))?;
    if read > max_bytes {
        warn!(
            "http_request BLOCKED (response too large): {} bytes > {} max — url={}",
            read, max_bytes, url
        );
        anyhow::bail!(
            "{}",
            tf(
                "error.http_response_too_large",
                &[("size", &read.to_string()), ("max", &max_bytes.to_string())]
            )
        );
    }
    Ok(body_bytes)
}

/// Extract the first HTTP/HTTPS URL from a text string.
pub fn extract_url(text: &str) -> Option<String> {
    // The earliest occurrence wins regardless of scheme — previously an
    // `https://` later in the text shadowed an earlier `http://`, so the
    // wrong URL was fetched/validated.
    let start = text
        .find("https://")
        .into_iter()
        .chain(text.find("http://"))
        .min()?;
    let remaining = &text[start..];
    let end = remaining
        .find(|c: char| {
            c.is_whitespace() || c == '\"' || c == '\'' || c == '>' || c == ')' || c == ']'
        })
        .unwrap_or(remaining.len());
    Some(remaining[..end].to_string())
}

/// Check if a hostname resolves to a private/internal IP address.
///
/// `pub(crate)` — shared with the observe-phase URL pre-fetch guard in
/// `acp/impl/chat/phases/` so both SSRF surfaces use the same definition
/// (loopback / private / link-local / multicast / unspecified / IPv6 variants).
pub(crate) fn is_private_host(host: &str) -> bool {
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

    // Hostname (not an IP literal): best-effort resolution now, blocking if
    // ANY resolved address is private (e.g. `metadata.google.internal` →
    // 169.254.169.254, or an internal DNS name → 10.x). This is a mitigation,
    // not a full DNS-rebinding defense — the later connect performs its own
    // lookup — but it closes the plain "hostname points at an internal
    // address" hole that the literal-IP check alone leaves open. Unresolvable
    // hostnames are allowed through; the connect fails with a clear error.
    use std::net::ToSocketAddrs;
    if let Ok(mut addrs) = (host, 0u16).to_socket_addrs() {
        return addrs.any(|addr| is_private_ip(addr.ip()));
    }
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
///
/// Shared by `http_request`, `web_scrape`, and `rss_read` so the SSRF /
/// private-IP protection applies to every URL-fetching tool.
pub(crate) fn validate_url(url: &str) -> Result<()> {
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

    if url_policy().block_private_ips && is_private_host(host) {
        anyhow::bail!(
            "{}",
            tf("error.url_blocked_private_host", &[("host", host)])
        );
    }

    // ── URL allow/block patterns (LAYER 3) ───────────────────────────
    let policy = url_policy();
    if policy.restrict_to_allowed && !policy.allowed_patterns.is_empty() {
        let allowed = policy.allowed_patterns.iter().any(|p| url.contains(p));
        if !allowed {
            anyhow::bail!("{}", tf("error.url_not_allowed", &[("url", url)]));
        }
    }
    if !policy.blocked_patterns.is_empty() {
        let blocked = policy.blocked_patterns.iter().any(|p| url.contains(p));
        if blocked {
            anyhow::bail!("{}", tf("error.url_blocked", &[("url", url)]));
        }
    }

    Ok(())
}

/// Timeout for the blocking hostname SSRF check (DNS resolution), so a slow
/// or offline resolver cannot stall an async worker. Shared with
/// `skill_import`'s RemoteSkill endpoint validation.
pub(crate) const SSRF_DNS_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// Async variant of [`validate_url`] for use on tokio workers.
///
/// The hostname SSRF check performs a (blocking) DNS resolution, which must
/// not run on an async worker; it is moved to the blocking pool with a
/// bounded timeout. On timeout the check is skipped (hostname treated as
/// public, matching the pre-DNS-resolution behavior) and reqwest's own
/// connect timeout applies. Note: after the timeout fires, the detached
/// blocking task keeps occupying one blocking-pool thread until the resolver
/// returns (accepted trade-off: workers are never stalled).
pub(crate) async fn validate_url_async(url: &str) -> Result<()> {
    let url_owned = url.to_string();
    let url_for_msg = url_owned.clone();
    match tokio::time::timeout(
        SSRF_DNS_CHECK_TIMEOUT,
        tokio::task::spawn_blocking(move || validate_url(&url_owned)),
    )
    .await
    {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(validation_error))) => Err(validation_error),
        Ok(Err(join_err)) => Err(anyhow::anyhow!("SSRF check task failed: {join_err}")),
        Err(_) => {
            warn!(
                "SSRF hostname check timed out for url={url_for_msg} — proceeding (DNS check skipped)"
            );
            Ok(())
        }
    }
}

/// Async variant of [`is_private_host`] for use on tokio workers.
///
/// Same blocking-DNS caveat as [`validate_url_async`]: the resolution runs on
/// the blocking pool with a bounded timeout; on timeout the host is treated
/// as public.
pub(crate) async fn is_private_host_async(host: &str) -> bool {
    let host_owned = host.to_string();
    let host_for_msg = host_owned.clone();
    match tokio::time::timeout(
        SSRF_DNS_CHECK_TIMEOUT,
        tokio::task::spawn_blocking(move || is_private_host(&host_owned)),
    )
    .await
    {
        Ok(Ok(private)) => private,
        // Check task failed unexpectedly: fail closed (treat as private).
        Ok(Err(_)) => true,
        Err(_) => {
            warn!("SSRF hostname check timed out for host={host_for_msg} — treating as public");
            false
        }
    }
}

/// Maximum redirect hops for the http_request tool (10).
const MAX_REDIRECTS: usize = 10;

/// Validate a redirect target without the blocking DNS recheck.
///
/// reqwest's redirect-policy hook is synchronous, so the resolver cannot be
/// moved to the blocking pool here. This still catches literal private/
/// loopback IP redirects (e.g. `public.example` → `169.254.169.254`) and
/// allow/block pattern violations on every hop; hostname-resolves-to-internal
/// redirects remain a documented residual (the initial-URL `validate_url`
/// still performs the DNS recheck). Without this, the previous
/// `Policy::limited` followed every hop unvalidated, so a one-line redirect
/// chain could reach the cloud metadata service.
fn validate_redirect_url_no_dns(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid redirect URL: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        anyhow::bail!("unsupported redirect scheme: {scheme}");
    }
    let host = parsed.host_str().unwrap_or("");
    if host.is_empty() {
        anyhow::bail!("redirect URL missing host");
    }

    if url_policy().block_private_ips {
        let literal = host.trim_start_matches('[').trim_end_matches(']');
        if host.eq_ignore_ascii_case("localhost")
            || host.eq_ignore_ascii_case("127.0.0.1")
            || host.eq_ignore_ascii_case("::1")
            || host.eq_ignore_ascii_case("[::1]")
        {
            anyhow::bail!("redirect target is a private host: {host}");
        }
        if let Ok(ip) = literal.parse::<IpAddr>() {
            if is_private_ip(ip) {
                anyhow::bail!("redirect target is a private host: {host}");
            }
        }
    }

    // URL allow/block patterns (LAYER 3) apply to redirect hops too.
    let policy = url_policy();
    if policy.restrict_to_allowed && !policy.allowed_patterns.is_empty() {
        let allowed = policy.allowed_patterns.iter().any(|p| url.contains(p));
        if !allowed {
            anyhow::bail!("redirect target not allowed: {url}");
        }
    }
    if !policy.blocked_patterns.is_empty() {
        let blocked = policy.blocked_patterns.iter().any(|p| url.contains(p));
        if blocked {
            anyhow::bail!("redirect target blocked: {url}");
        }
    }
    Ok(())
}

/// Redirect policy that re-validates every hop against the URL policy
/// (scheme / literal private IP / allow-block patterns).
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let url = attempt.url().to_string();
        match validate_redirect_url_no_dns(&url) {
            Ok(()) => {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    attempt.error(std::io::Error::other("too many redirects"))
                } else {
                    attempt.follow()
                }
            }
            Err(e) => attempt.error(std::io::Error::other(e.to_string())),
        }
    })
}

/// Shared async reqwest client — built once and reused to benefit from
/// connection pooling. Per-request timeouts are applied at the request
/// builder level so the pooled client serves every timeout class.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(redirect_policy())
            .build()
            .expect("failed to build shared HTTP client")
    })
}

/// Shared blocking reqwest client for the sync fallback path. Also used by
/// `web_scrape` / `rss_read` (via `super::http::blocking_tool_client`) so
/// redirect hops are re-validated against the URL policy there too.
pub(crate) fn blocking_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .redirect(redirect_policy())
            .build()
            .expect("failed to build shared blocking HTTP client")
    })
}

pub struct HttpRequestTool;

impl Tool for HttpRequestTool {
    fn name(&self) -> &'static str {
        "http_request"
    }
    fn description(&self) -> &str {
        "Make HTTP requests (GET/POST/PUT/DELETE) to external APIs. Only http:// and https:// URLs are allowed. Private/internal IPs are blocked for security."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full HTTP/HTTPS URL to request (required)"
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"],
                    "description": "HTTP method (default: GET)"
                },
                "headers": {
                    "type": "object",
                    "description": "Optional HTTP headers as key-value pairs",
                    "additionalProperties": {"type": "string"}
                },
                "body": {
                    "type": "string",
                    "description": "Request body for POST/PUT/PATCH"
                },
                "auth": {
                    "type": "object",
                    "properties": {
                        "bearer": {
                            "type": "string",
                            "description": "Bearer token for Authorization header"
                        }
                    }
                },
                "query": {
                    "type": "object",
                    "description": "Query parameters as key-value pairs",
                    "additionalProperties": {"type": "string"}
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Request timeout in milliseconds (default: 15000)"
                }
            },
            "required": ["url"]
        })
    }

    /// Sync path: uses blocking reqwest client directly.
    /// Independent from run_async — no cross-delegation to avoid
    /// issues with spawn_blocking + block_in_place nesting.
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        Self::run_sync(input)
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
            // validate_url_async runs the (blocking) hostname DNS SSRF check
            // on the blocking pool with a bounded timeout, so a slow resolver
            // cannot stall this async worker.
            validate_url_async(&url).await.map_err(|e| {
                warn!("http_request BLOCKED (sandbox): {} — url={}", e, url);
                e
            })?;

            let method = input.payload["method"]
                .as_str()
                .unwrap_or("GET")
                .to_string();
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

            // ── 4. Max response size (from UrlPolicyConfig, LAYER 3) ──
            let max_response_bytes: usize = url_policy().max_response_bytes;

            let client = http_client();

            let mut request_builder: reqwest::RequestBuilder = match method.to_uppercase().as_str()
            {
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

            // Per-request timeout on the pooled client.
            request_builder = request_builder.timeout(Duration::from_millis(timeout_ms));

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
            let response = request_builder
                .send()
                .await
                .context("HTTP request failed")?;
            let status = response.status().as_u16();

            // ── 9. Runtime sandbox: response body size limit ──────────
            // Enforce the cap DURING the read: check the declared
            // Content-Length first (before buffering any body bytes), then
            // stream the body with a hard cap so an oversized response is
            // rejected without ever being fully resident in memory.
            // (Previously `response.text()` buffered the whole body before
            // the limit was checked, so the 10MB default didn't protect
            // memory at all.)
            let max_bytes = max_response_bytes;
            if let Some(declared) = response.content_length() {
                if declared > max_bytes as u64 {
                    warn!(
                        "http_request BLOCKED (response too large): declared {} bytes > {} max — url={}",
                        declared,
                        max_bytes,
                        url
                    );
                    anyhow::bail!(
                        "{}",
                        tf(
                            "error.http_response_too_large",
                            &[
                                ("size", &declared.to_string()),
                                ("max", &max_bytes.to_string())
                            ]
                        )
                    );
                }
            }
            use futures_util::StreamExt;
            let mut body_stream = response.bytes_stream();
            let mut body_bytes: Vec<u8> = Vec::new();
            let body_read_result: Result<()> = loop {
                match body_stream.next().await {
                    Some(Ok(chunk)) => {
                        body_bytes.extend_from_slice(&chunk);
                        if body_bytes.len() > max_bytes {
                            break Err(anyhow::anyhow!(
                                "{}",
                                tf(
                                    "error.http_response_too_large",
                                    &[
                                        ("size", &body_bytes.len().to_string()),
                                        ("max", &max_bytes.to_string())
                                    ]
                                )
                            ));
                        }
                    }
                    Some(Err(e)) => {
                        break Err(anyhow::anyhow!("failed to read response body: {e}"));
                    }
                    None => break Ok(()),
                }
            };
            if let Err(e) = body_read_result {
                warn!(
                    "http_request BLOCKED (response too large): {} bytes > {} max — url={}",
                    body_bytes.len(),
                    max_bytes,
                    url
                );
                return Err(e);
            }
            let response_body = String::from_utf8_lossy(&body_bytes).into_owned();

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

        let client = blocking_client();

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

        // Per-request timeout on the pooled blocking client.
        request_builder = request_builder.timeout(Duration::from_millis(timeout_ms));

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

        // ── Runtime sandbox: response body size limit ──────────────────
        // Shared capped reader (content-length pre-check + `Read::take`), so
        // a huge response is never fully buffered.
        let mut response = response;
        let body_bytes = read_blocking_body_capped(&mut response, &url)?;
        let response_body = String::from_utf8_lossy(&body_bytes).into_owned();

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
