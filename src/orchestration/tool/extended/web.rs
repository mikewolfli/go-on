//! Web scraping and data collection tools
//!
//! Provides `WebScrapeTool` for extracting structured content from web pages.
//! Only compiled when `feature = "document-html"` is enabled.

#[cfg(feature = "document-html")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "document-html")]
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
#[cfg(feature = "document-html")]
use anyhow::{Context, Result};
#[cfg(feature = "document-html")]
use std::time::Duration;
#[cfg(feature = "document-html")]
use tracing::info;

#[cfg(feature = "document-html")]
pub struct WebScrapeTool;

#[cfg(feature = "document-html")]
impl Tool for WebScrapeTool {
    fn name(&self) -> &'static str {
        "web_scrape"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let url = input.payload["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'url'"))?;
        let selector = input.payload["selector"].as_str();
        let timeout_ms = input.payload["timeout_ms"].as_u64().unwrap_or(15_000);

        // Enforce the same SSRF / private-IP sandbox as http_request.
        super::http::validate_url(url)?;

        info!(url = %url, "web_scrape: fetching page");

        // Reuse the tool-side blocking client (validated redirect policy —
        // redirect hops are re-checked against the URL policy, unlike the
        // shared default client which follows them unvalidated); the request
        // carries the per-call timeout.
        let client = super::http::blocking_client();

        let mut response = client
            .get(url)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .with_context(|| format!("failed to fetch {url}"))?;

        let status = response.status().as_u16();
        // Body size cap (same policy as http_request) enforced during the
        // read — a hostile page must not be fully buffered.
        let body_bytes = super::http::read_blocking_body_capped(&mut response, url)?;
        let html = String::from_utf8_lossy(&body_bytes).into_owned();

        let document = scraper::Html::parse_document(&html);

        let mut extracted = Vec::new();
        if let Some(sel_str) = selector {
            let sel = scraper::Selector::parse(sel_str)
                .map_err(|e| anyhow::anyhow!("invalid CSS selector '{sel_str}': {e}"))?;
            for element in document.select(&sel) {
                let text = element.text().collect::<Vec<_>>().join(" ");
                let html_inner = element.inner_html();
                extracted.push(serde_json::json!({
                    "text": text,
                    "html": html_inner,
                }));
            }
        } else {
            // No selector: extract all text from body
            let body_sel = scraper::Selector::parse("body")
                .map_err(|_| anyhow::anyhow!("failed to parse body selector"))?;
            if let Some(body) = document.select(&body_sel).next() {
                let text = body.text().collect::<Vec<_>>().join(" ");
                extracted.push(serde_json::json!({
                    "text": text,
                }));
            }
        }

        let title = document
            .select(&scraper::Selector::parse("title").expect("valid CSS selector 'title'"))
            .next()
            .map(|e| e.text().collect::<String>());

        let byte_size = html.len();

        info!(url = %url, status, elements = extracted.len(), "web_scrape completed");

        let report = tool_execution_report("web_scrape", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "url": url,
                "status": status,
                "title": title,
                "elements": extracted,
                "byte_size": byte_size,
                "element_count": extracted.len(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "web_scrape: {} elements from {} (HTTP {})",
                extracted.len(),
                url,
                status
            )),
            pua_report: Some(report),
        })
    }
}
