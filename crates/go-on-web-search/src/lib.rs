//! Web search client for go-on.
//!
//! Provides a unified interface for searching the web via multiple providers.
//! The default provider is DuckDuckGo Instant Answer API (free, no API key required).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// DuckDuckGo Instant Answer API base URL (free, no API key).
const DDG_API_BASE: &str = "https://api.duckduckgo.com";

/// Cap for search API response bodies: a hostile/buggy endpoint must not be
/// able to grow the client's memory unboundedly (aligned with the backend's
/// MAX_BODY_SIZE).
const MAX_SEARCH_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Read a response body with a byte cap enforced DURING the read, then parse
/// it as JSON (replaces bare `response.json()`, which buffers the whole
/// payload before parsing).
async fn capped_json(mut response: reqwest::Response) -> Result<serde_json::Value> {
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("failed to read search API response")?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_SEARCH_RESPONSE_BYTES {
            anyhow::bail!("search API response exceeds the {MAX_SEARCH_RESPONSE_BYTES} byte limit");
        }
    }
    serde_json::from_slice(&body).context("failed to parse search API response")
}

// ---------------------------------------------------------------------------
// SearchResult
// ---------------------------------------------------------------------------

/// A single search result item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

// ---------------------------------------------------------------------------
// Provider configuration
// ---------------------------------------------------------------------------

/// Supported web search providers.
#[derive(Debug, Clone, Default)]
pub enum SearchProvider {
    /// DuckDuckGo Instant Answer API — free, no API key required.
    #[default]
    DuckDuckGo,
    /// A custom search endpoint (e.g. a self-hosted or commercial API).
    Custom {
        api_endpoint: String,
        api_key: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Client configuration
// ---------------------------------------------------------------------------

/// Configuration for the web search client.
#[derive(Debug, Clone)]
pub struct WebSearchConfig {
    /// The search provider to use.
    pub provider: SearchProvider,
    /// Request timeout in seconds (default: 15).
    pub timeout_secs: u64,
    /// Maximum number of results to return per search (default: 5).
    pub max_results: usize,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            provider: SearchProvider::DuckDuckGo,
            timeout_secs: 15,
            max_results: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// DuckDuckGo API response types
// ---------------------------------------------------------------------------

/// Top-level response from the DuckDuckGo Instant Answer API.
#[derive(Debug, Deserialize)]
struct DuckDuckGoResponse {
    #[serde(default)]
    abstract_text: String,
    #[serde(default)]
    abstract_url: String,
    #[serde(default)]
    abstract_source: String,
    #[serde(default)]
    results: Vec<DuckDuckGoResult>,
    #[serde(default)]
    related_topics: Vec<DuckDuckGoTopic>,
}

/// A single result entry from DuckDuckGo's `Results` array.
#[derive(Debug, Deserialize)]
struct DuckDuckGoResult {
    #[serde(default)]
    text: String,
    #[serde(default)]
    first_url: String,
}

/// A topic entry from DuckDuckGo's `RelatedTopics` array.
/// Each topic may itself contain nested `Topics` for categories.
#[derive(Debug, Deserialize)]
struct DuckDuckGoTopic {
    #[serde(default)]
    text: String,
    #[serde(default)]
    first_url: String,
    #[serde(default)]
    topics: Option<Vec<DuckDuckGoTopic>>,
}

// ---------------------------------------------------------------------------
// WebSearchClient
// ---------------------------------------------------------------------------

/// A client for performing web searches via configurable providers.
pub struct WebSearchClient {
    config: WebSearchConfig,
    http_client: reqwest::Client,
}

impl WebSearchClient {
    /// Create a new `WebSearchClient` with the given configuration.
    pub fn new(config: WebSearchConfig) -> Result<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            // Versioned from the crate version so the UA never drifts.
            .user_agent(concat!("go-on-web-search/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            config,
            http_client,
        })
    }

    /// Perform a web search and return up to `max_results` results.
    pub async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        match &self.config.provider {
            SearchProvider::DuckDuckGo => self.search_duckduckgo(query, max_results).await,
            SearchProvider::Custom {
                api_endpoint,
                api_key,
            } => {
                self.search_custom(query, max_results, api_endpoint, api_key.as_deref())
                    .await
            }
        }
    }

    /// Search using the DuckDuckGo Instant Answer API.
    async fn search_duckduckgo(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>> {
        let url = format!(
            "{DDG_API_BASE}/?q={}&format=json&no_html=1&skip_disambig=1",
            urlencoding(query)
        );

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("DuckDuckGo API request failed")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "DuckDuckGo API returned HTTP {}",
                response.status().as_u16()
            );
        }

        let ddg_response: DuckDuckGoResponse = serde_json::from_value(capped_json(response).await?)
            .context("Failed to parse DuckDuckGo API response")?;

        let mut results: Vec<SearchResult> = Vec::new();

        // 1. Abstract result (the "Instant Answer" box at the top)
        if !ddg_response.abstract_text.is_empty() {
            results.push(SearchResult {
                title: ddg_response.abstract_source.clone(),
                url: ddg_response.abstract_url.clone(),
                snippet: ddg_response.abstract_text.clone(),
            });
        }

        // 2. Explicit results from the `Results` array
        for r in &ddg_response.results {
            if results.len() >= max_results {
                break;
            }
            if let Some((title, snippet)) = parse_ddg_text(&r.text) {
                results.push(SearchResult {
                    title,
                    url: r.first_url.clone(),
                    snippet,
                });
            }
        }

        // 3. Related topics
        for topic in &ddg_response.related_topics {
            if results.len() >= max_results {
                break;
            }
            // Flatten nested topics (category groups)
            flatten_ddg_topics(topic, &mut results, max_results);
        }

        Ok(results)
    }

    /// Search using a custom provider endpoint.
    async fn search_custom(
        &self,
        query: &str,
        max_results: usize,
        api_endpoint: &str,
        api_key: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let url = api_endpoint.replace("{query}", &urlencoding(query));

        let mut request = self.http_client.get(&url);

        if let Some(key) = api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        let response = request
            .send()
            .await
            .context("Custom search API request failed")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Custom search API returned HTTP {}",
                response.status().as_u16()
            );
        }

        // Parse the response as JSON. NOTE: the comment below historically
        // promised a "raw text response" fallback that was never implemented;
        // custom endpoints must return the generic JSON results shape
        // parse_generic_json_results expects. The body is read with a byte
        // cap so a hostile endpoint cannot OOM the client.
        let body: serde_json::Value = capped_json(response).await?;

        let results = parse_generic_json_results(&body, max_results)?;

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// URL-encode a string for use in a query parameter.
fn urlencoding(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// Parse a DuckDuckGo `Text` field of the form `"Title - Description"`.
fn parse_ddg_text(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(dash_idx) = text.find(" - ") {
        let title = text[..dash_idx].trim().to_string();
        let snippet = text[dash_idx + 3..].trim().to_string();
        Some((title, snippet))
    } else {
        // No separator; use entire text as title
        Some((text.to_string(), String::new()))
    }
}

/// Recursively flatten DuckDuckGo topic entries (which may contain nested `Topics`).
fn flatten_ddg_topics(
    topic: &DuckDuckGoTopic,
    results: &mut Vec<SearchResult>,
    max_results: usize,
) {
    if results.len() >= max_results {
        return;
    }

    if !topic.text.is_empty() {
        if let Some((title, snippet)) = parse_ddg_text(&topic.text) {
            results.push(SearchResult {
                title,
                url: topic.first_url.clone(),
                snippet,
            });
        }
    }

    if let Some(ref sub_topics) = topic.topics {
        for sub in sub_topics {
            if results.len() >= max_results {
                break;
            }
            flatten_ddg_topics(sub, results, max_results);
        }
    }
}

/// Attempt to parse a generic JSON response into `SearchResult` items.
///
/// Supports the following shapes:
/// - `[{"title": ..., "url": ..., "snippet": ...}, ...]`
/// - `{"results": [{"title": ..., "url": ..., "snippet": ...}, ...]}`
/// - `{"items": [{"title": ..., "url": ..., "snippet": ...}, ...]}`
/// - `{"organic_results": [{"title": ..., "link": ..., "snippet": ...}, ...]}`
fn parse_generic_json_results(
    body: &serde_json::Value,
    max_results: usize,
) -> Result<Vec<SearchResult>> {
    let items: Vec<serde_json::Value> =
        try_extract_array(body, &["results", "items", "organic_results"])
            .or_else(|| body.as_array())
            .cloned()
            .unwrap_or_default();

    let mut results: Vec<SearchResult> = Vec::new();

    for item in items.iter().take(max_results) {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let url = item
            .get("url")
            .or_else(|| item.get("link"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let snippet = item
            .get("snippet")
            .or_else(|| item.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        results.push(SearchResult {
            title,
            url,
            snippet,
        });
    }

    Ok(results)
}

/// Try to extract an array from a JSON object using one of the given keys.
fn try_extract_array<'a>(
    body: &'a serde_json::Value,
    keys: &[&str],
) -> Option<&'a Vec<serde_json::Value>> {
    let obj = body.as_object()?;
    for key in keys {
        if let Some(arr) = obj.get(*key).and_then(|v| v.as_array()) {
            return Some(arr);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello%20world");
        assert_eq!(urlencoding("foo/bar"), "foo%2Fbar");
        assert_eq!(urlencoding("abc123"), "abc123");
    }

    #[test]
    fn test_parse_ddg_text_with_separator() {
        let (title, snippet) = parse_ddg_text("Hello World - This is a description").unwrap();
        assert_eq!(title, "Hello World");
        assert_eq!(snippet, "This is a description");
    }

    #[test]
    fn test_parse_ddg_text_no_separator() {
        let (title, snippet) = parse_ddg_text("Just a title").unwrap();
        assert_eq!(title, "Just a title");
        assert_eq!(snippet, "");
    }

    #[test]
    fn test_parse_ddg_text_empty() {
        assert!(parse_ddg_text("").is_none());
        assert!(parse_ddg_text("  ").is_none());
    }

    #[test]
    fn test_parse_generic_json_results_array() {
        let json = serde_json::json!([
            {"title": "Result 1", "url": "https://example.com/1", "snippet": "Snippet 1"},
            {"title": "Result 2", "url": "https://example.com/2", "snippet": "Snippet 2"}
        ]);
        let results = parse_generic_json_results(&json, 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Result 1");
        assert_eq!(results[0].url, "https://example.com/1");
        assert_eq!(results[0].snippet, "Snippet 1");
    }

    #[test]
    fn test_parse_generic_json_results_with_results_key() {
        let json = serde_json::json!({
            "results": [
                {"title": "R1", "url": "https://ex.com/1", "snippet": "S1"}
            ]
        });
        let results = parse_generic_json_results(&json, 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_parse_generic_json_results_with_organic_results() {
        let json = serde_json::json!({
            "organic_results": [
                {"title": "Google Result", "link": "https://google.com", "snippet": "A result"}
            ]
        });
        let results = parse_generic_json_results(&json, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://google.com");
    }

    #[test]
    fn test_parse_generic_json_results_with_items_key() {
        let json = serde_json::json!({
            "items": [
                {"title": "Item 1", "url": "https://ex.com/1", "description": "Desc 1"}
            ]
        });
        let results = parse_generic_json_results(&json, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].snippet, "Desc 1");
    }

    #[test]
    fn test_parse_generic_json_results_max_results() {
        let json = serde_json::json!([
            {"title": "R1", "url": "https://ex.com/1", "snippet": "S1"},
            {"title": "R2", "url": "https://ex.com/2", "snippet": "S2"},
            {"title": "R3", "url": "https://ex.com/3", "snippet": "S3"}
        ]);
        let results = parse_generic_json_results(&json, 2).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_provider_default() {
        let provider = SearchProvider::default();
        assert!(matches!(provider, SearchProvider::DuckDuckGo));
    }

    #[test]
    fn test_web_search_config_default() {
        let config = WebSearchConfig::default();
        assert!(matches!(config.provider, SearchProvider::DuckDuckGo));
        assert_eq!(config.timeout_secs, 15);
        assert_eq!(config.max_results, 5);
    }

    #[test]
    fn test_flatten_ddg_topics() {
        let topic = DuckDuckGoTopic {
            text: "Category - Description".to_string(),
            first_url: "https://example.com/cat".to_string(),
            topics: Some(vec![
                DuckDuckGoTopic {
                    text: "Sub 1 - Sub description".to_string(),
                    first_url: "https://example.com/sub1".to_string(),
                    topics: None,
                },
                DuckDuckGoTopic {
                    text: "Sub 2".to_string(),
                    first_url: "https://example.com/sub2".to_string(),
                    topics: None,
                },
            ]),
        };

        let mut results = Vec::new();
        flatten_ddg_topics(&topic, &mut results, 5);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].title, "Category");
        assert_eq!(results[1].title, "Sub 1");
        assert_eq!(results[2].title, "Sub 2");
    }
}
