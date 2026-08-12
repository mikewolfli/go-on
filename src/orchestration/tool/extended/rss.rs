//! RSS/Atom feed reading tools
//!
//! Provides `RssReadTool` for fetching and parsing RSS/Atom feeds
//! using HTTP requests and basic XML parsing (uses `reqwest` which is already a dependency).
//! No new crate needed — parses XML minimally inline.
//! Always compiled (reqwest is already available).

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::extended::utils::extract_xml_tag;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use crate::shared::text::contains_ascii_case_insensitive;
use anyhow::{Context, Result};
use std::time::Duration;
use tracing::info;

pub struct RssReadTool;

impl Tool for RssReadTool {
    fn name(&self) -> &'static str {
        "rss_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let url = input.payload["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'url'"))?;
        let max_items = input.payload["max_items"].as_u64().unwrap_or(20) as usize;
        let timeout_ms = input.payload["timeout_ms"].as_u64().unwrap_or(15_000);

        // Enforce the same SSRF / private-IP sandbox as http_request.
        super::http::validate_url(url)?;

        info!(url = %url, "rss_read: fetching feed");

        // Reuse the shared blocking client (connection pooling); the request
        // carries the per-call timeout.
        let client = crate::shared::http_client::blocking_http_client()
            .context("failed to build HTTP client")?;

        let mut response = client
            .get(url)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .with_context(|| format!("failed to fetch feed: {url}"))?;

        let status = response.status().as_u16();
        // Body size cap (same policy as http_request) enforced during the
        // read — a hostile feed must not be fully buffered.
        let body_bytes = super::http::read_blocking_body_capped(&mut response, url)?;
        let body = String::from_utf8_lossy(&body_bytes).into_owned();

        // Simple XML parsing for RSS 2.0 and Atom feeds
        // Uses basic string extraction to avoid adding an XML crate dependency
        let mut items = Vec::new();

        // Detect feed type (ASCII case-insensitive, no lowercased copy of the
        // potentially large feed body)
        let feed_type = if contains_ascii_case_insensitive(&body, "<rss") {
            "RSS 2.0"
        } else if contains_ascii_case_insensitive(&body, "<feed")
            && contains_ascii_case_insensitive(&body, "xmlns=\"http://www.w3.org/2005/atom\"")
        {
            "Atom"
        } else if contains_ascii_case_insensitive(&body, "<feed") {
            "Atom (likely)"
        } else {
            "Unknown"
        };

        // Parse RSS items
        if feed_type.starts_with("RSS") {
            for item_body in body.split("<item>").skip(1) {
                if items.len() >= max_items {
                    break;
                }
                let item_end = item_body.find("</item>").unwrap_or(item_body.len());
                let item = &item_body[..item_end];

                let title = extract_xml_tag(item, "title").unwrap_or_default();
                let link = extract_xml_tag(item, "link").unwrap_or_default();
                let description = extract_xml_tag(item, "description").unwrap_or_default();
                let pub_date = extract_xml_tag(item, "pubdate").unwrap_or_default();

                items.push(serde_json::json!({
                    "title": title,
                    "link": link,
                    "description": description.chars().take(200).collect::<String>(),
                    "pub_date": pub_date,
                }));
            }
        } else {
            // Parse Atom entries
            for entry_body in body.split("<entry>").skip(1) {
                if items.len() >= max_items {
                    break;
                }
                let entry_end = entry_body.find("</entry>").unwrap_or(entry_body.len());
                let entry = &entry_body[..entry_end];

                let title = extract_xml_tag(entry, "title").unwrap_or_default();
                let link = extract_xml_tag(entry, "link").unwrap_or_default();
                let summary = extract_xml_tag(entry, "summary").unwrap_or_default();
                let updated = extract_xml_tag(entry, "updated").unwrap_or_default();

                items.push(serde_json::json!({
                    "title": title,
                    "link": link,
                    "description": summary.chars().take(200).collect::<String>(),
                    "pub_date": updated,
                }));
            }
        }

        info!(url = %url, status, feed_type, items = items.len(), "RSS feed read");

        let report = tool_execution_report("rss_read", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "feed_type": feed_type,
                "item_count": items.len(),
                "items": items,
                "byte_size": body.len(),
                "status": status,
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "rss_read: {} items from {} ({})",
                items.len(),
                url,
                feed_type
            )),
            pua_report: Some(report),
        })
    }
}
