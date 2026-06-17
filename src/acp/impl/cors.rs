//! CORS (Cross-Origin Resource Sharing) support for the ACP HTTP server.
//!
//! This module provides types and functions to generate CORS headers for
//! both preflight (`OPTIONS`) and actual HTTP responses.

/// Default list of allowed origins when none are configured.
/// Default is empty (no CORS) — users must explicitly configure allowed origins.
pub const CORS_DEFAULT_ALLOWED_ORIGINS: &[&str] = &[];

/// Configuration for CORS header generation.
///
/// All fields are public so callers can construct or modify configs freely.
#[derive(Debug, Clone)]
pub struct CorsConfig {
    /// List of allowed origins.  `"*"` matches every origin.
    pub allowed_origins: Vec<String>,
    /// HTTP methods allowed by the resource (e.g. `GET`, `POST`, `OPTIONS`).
    pub allowed_methods: Vec<String>,
    /// Request headers the resource may accept.
    pub allowed_headers: Vec<String>,
    /// Headers that may be exposed to the browser / client.
    pub expose_headers: Vec<String>,
    /// Lifetime in seconds of the preflight result cache.
    pub max_age_seconds: u64,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: CORS_DEFAULT_ALLOWED_ORIGINS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            allowed_methods: vec!["GET".to_string(), "POST".to_string(), "OPTIONS".to_string()],
            allowed_headers: vec![
                "Content-Type".to_string(),
                "Authorization".to_string(),
                "X-Api-Key".to_string(),
                "X-Go-On-Key".to_string(),
            ],
            expose_headers: vec![
                "Content-Type".to_string(),
                "X-Request-Id".to_string(),
                "X-Request-Idempotency-Key".to_string(),
            ],
            max_age_seconds: 86400,
        }
    }
}

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

/// Check whether `origin` is permitted by the given `config`.
///
/// The wildcard `"*"` inside `config.allowed_origins` grants access to every
/// origin.  Origins are compared case-sensitively.
pub fn is_origin_allowed(origin: &str, config: &CorsConfig) -> bool {
    config
        .allowed_origins
        .iter()
        .any(|allowed| allowed == "*" || allowed == origin)
}

/// Build the complete set of CORS response headers for an **actual** (non-preflight)
/// request, given an optional `Origin` header value.
///
/// Returns an empty `Vec` when the origin is not allowed (or missing).
pub fn build_cors_headers(origin: Option<&str>, config: &CorsConfig) -> Vec<(String, String)> {
    let origin = match origin {
        Some(o) if !o.is_empty() => o,
        _ => return Vec::new(),
    };

    if !is_origin_allowed(origin, config) {
        return Vec::new();
    }

    let mut headers = Vec::new();

    headers.push((
        "Access-Control-Allow-Origin".to_string(),
        origin.to_string(),
    ));

    let methods = config.allowed_methods.join(", ");
    headers.push(("Access-Control-Allow-Methods".to_string(), methods));

    let req_headers = config.allowed_headers.join(", ");
    headers.push(("Access-Control-Allow-Headers".to_string(), req_headers));

    let exposed = config.expose_headers.join(", ");
    if !exposed.is_empty() {
        headers.push(("Access-Control-Expose-Headers".to_string(), exposed));
    }

    headers.push((
        "Access-Control-Max-Age".to_string(),
        config.max_age_seconds.to_string(),
    ));

    headers
}

/// Build the CORS response headers for an **`OPTIONS`** (preflight) request.
///
/// `request_headers` should contain the value of the
/// `Access-Control-Request-Headers` header sent by the client.  The returned
/// headers will reflect only those methods / headers that are actually
/// permitted by `config`.
pub fn build_preflight_response_headers(
    request_headers: Option<&str>,
    config: &CorsConfig,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::new();

    // Always include the wildcard origin for preflight when `*` is allowed,
    // or rely on the caller to supply origin matching.  Since preflight
    // responses are cached per origin we keep this simple.
    if config.allowed_origins.iter().any(|o| o == "*") {
        headers.push(("Access-Control-Allow-Origin".to_string(), "*".to_string()));
    }
    // If there is a concrete list of origins, the caller must have determined
    // which one matched so we don't add the header here unconditionally;
    // the server should set it per request.  For convenience we still include
    // the method / header negotiation below.

    let methods = config.allowed_methods.join(", ");
    headers.push(("Access-Control-Allow-Methods".to_string(), methods));

    // Reflect the allowed headers that were actually requested.
    let requested = request_headers
        .map(|h| {
            h.split(',')
                .map(|s| s.trim().to_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let permitted: Vec<&str> = config
        .allowed_headers
        .iter()
        .filter(|h| requested.is_empty() || requested.contains(&h.to_lowercase()))
        .map(|s| s.as_str())
        .collect();

    if !permitted.is_empty() {
        headers.push((
            "Access-Control-Allow-Headers".to_string(),
            permitted.join(", "),
        ));
    }

    headers.push((
        "Access-Control-Max-Age".to_string(),
        config.max_age_seconds.to_string(),
    ));

    headers
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Test helpers ------------------------------------------------------

    fn wildcard_config() -> CorsConfig {
        CorsConfig {
            allowed_origins: vec!["*".to_string()],
            ..CorsConfig::default()
        }
    }

    // -- is_origin_allowed --------------------------------------------------

    #[test]
    fn test_is_origin_allowed_wildcard() {
        let config = wildcard_config(); // allowed_origins = ["*"]
        assert!(is_origin_allowed("https://example.com", &config));
        assert!(is_origin_allowed("http://localhost:3000", &config));
        assert!(is_origin_allowed("https://app.go-on.dev", &config));
    }

    #[test]
    fn test_is_origin_allowed_exact_match() {
        let config = CorsConfig {
            allowed_origins: vec!["https://app.go-on.dev".to_string()],
            ..wildcard_config()
        };
        assert!(is_origin_allowed("https://app.go-on.dev", &config));
        assert!(!is_origin_allowed("https://evil.com", &config));
    }

    #[test]
    fn test_is_origin_allowed_multiple() {
        let config = CorsConfig {
            allowed_origins: vec!["https://a.com".to_string(), "https://b.com".to_string()],
            ..wildcard_config()
        };
        assert!(is_origin_allowed("https://a.com", &config));
        assert!(is_origin_allowed("https://b.com", &config));
        assert!(!is_origin_allowed("https://c.com", &config));
    }

    #[test]
    fn test_is_origin_allowed_case_sensitive() {
        let config = CorsConfig {
            allowed_origins: vec!["https://MyApp.COM".to_string()],
            ..wildcard_config()
        };
        // Case-sensitive – must match exactly.
        assert!(is_origin_allowed("https://MyApp.COM", &config));
        assert!(!is_origin_allowed("https://myapp.com", &config));
    }

    // -- build_cors_headers ------------------------------------------------

    #[test]
    fn test_build_cors_headers_with_matching_origin() {
        let config = wildcard_config();
        let headers = build_cors_headers(Some("https://example.com"), &config);

        assert_eq!(
            header_value(&headers, "Access-Control-Allow-Origin"),
            Some("https://example.com")
        );
        assert_eq!(
            header_value(&headers, "Access-Control-Allow-Methods"),
            Some("GET, POST, OPTIONS")
        );
        assert_eq!(
            header_value(&headers, "Access-Control-Allow-Headers"),
            Some("Content-Type, Authorization, X-Api-Key, X-Go-On-Key")
        );
        assert!(header_value(&headers, "Access-Control-Expose-Headers").is_some());
        assert_eq!(
            header_value(&headers, "Access-Control-Max-Age"),
            Some("86400")
        );
    }

    #[test]
    fn test_build_cors_headers_no_origin() {
        let config = wildcard_config();
        let headers = build_cors_headers(None, &config);
        assert!(headers.is_empty(), "expected empty headers when no origin");
    }

    #[test]
    fn test_build_cors_headers_empty_origin() {
        let config = wildcard_config();
        let headers = build_cors_headers(Some(""), &config);
        assert!(
            headers.is_empty(),
            "expected empty headers for empty origin"
        );
    }

    #[test]
    fn test_build_cors_headers_disallowed_origin() {
        let config = CorsConfig {
            allowed_origins: vec!["https://trusted.com".to_string()],
            ..wildcard_config()
        };
        let headers = build_cors_headers(Some("https://evil.com"), &config);
        assert!(
            headers.is_empty(),
            "expected empty headers for disallowed origin"
        );
    }

    #[test]
    fn test_build_cors_headers_wildcard_origin() {
        let config = wildcard_config();
        let headers = build_cors_headers(Some("*"), &config);
        assert_eq!(
            header_value(&headers, "Access-Control-Allow-Origin"),
            Some("*")
        );
    }

    // -- build_preflight_response_headers ----------------------------------

    #[test]
    fn test_build_preflight_response_headers_with_wildcard() {
        let config = wildcard_config();
        let headers = build_preflight_response_headers(None, &config);

        assert_eq!(
            header_value(&headers, "Access-Control-Allow-Origin"),
            Some("*")
        );
        assert_eq!(
            header_value(&headers, "Access-Control-Allow-Methods"),
            Some("GET, POST, OPTIONS")
        );
        assert!(header_value(&headers, "Access-Control-Allow-Headers").is_some());
        assert_eq!(
            header_value(&headers, "Access-Control-Max-Age"),
            Some("86400")
        );
    }

    #[test]
    fn test_build_preflight_response_headers_filters_request_headers() {
        let config = wildcard_config();
        let headers = build_preflight_response_headers(Some("X-Api-Key, X-Go-On-Key"), &config);

        let allow_headers = header_value(&headers, "Access-Control-Allow-Headers")
            .expect("Access-Control-Allow-Headers should be present");
        // Only the requested headers that are also in allowed_headers should appear.
        assert!(allow_headers.contains("X-Api-Key"));
        assert!(allow_headers.contains("X-Go-On-Key"));
        // Content-Type was not requested, so it should NOT appear.
        assert!(!allow_headers.contains("Content-Type"));
    }

    #[test]
    fn test_build_preflight_response_headers_allows_all_when_no_request_headers() {
        let config = wildcard_config();
        let headers = build_preflight_response_headers(None, &config);

        let allowed = header_value(&headers, "Access-Control-Allow-Headers")
            .expect("Access-Control-Allow-Headers should be present");
        // When nothing is requested, all configured allowed headers are returned.
        for h in &config.allowed_headers {
            assert!(
                allowed.contains(h),
                "expected {h} to be in the allowed headers list"
            );
        }
    }

    #[test]
    fn test_build_preflight_response_headers_unknown_request_headers_omitted() {
        let config = wildcard_config();
        let headers = build_preflight_response_headers(Some("X-Invented, X-Other"), &config);

        let allowed = header_value(&headers, "Access-Control-Allow-Headers");
        // Since neither "X-Invented" nor "X-Other" are in the configured
        // allowed_headers list, the intersection is empty → no header emitted.
        assert!(
            allowed.is_none()
                || allowed
                    .expect("allowed headers should be present")
                    .is_empty()
        );
    }

    // -- Default config construction ---------------------------------------

    #[test]
    fn test_default_config_has_expected_fields() {
        let config = CorsConfig::default();
        assert!(
            config.allowed_origins.is_empty(),
            "expected no default allowed origins"
        );
        assert_eq!(config.allowed_methods, vec!["GET", "POST", "OPTIONS"]);
        assert_eq!(
            config.allowed_headers,
            vec!["Content-Type", "Authorization", "X-Api-Key", "X-Go-On-Key"]
        );
        assert!(config.expose_headers.contains(&"Content-Type".to_string()));
        assert_eq!(config.max_age_seconds, 86400);
    }

    // -- CORS_DEFAULT_ALLOWED_ORIGINS constant -----------------------------

    // -- Helpers -----------------------------------------------------------

    /// Convenience helper to extract a header value from a `Vec<(String, String)>`.
    fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}
