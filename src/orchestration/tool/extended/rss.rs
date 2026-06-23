//! RSS/Atom feed reading tools
//!
//! Provides `RssReadTool` for fetching and parsing RSS/Atom feeds
//! using HTTP requests and basic XML parsing (uses `reqwest` which is already a dependency).
//! No new crate needed — parses XML minimally inline.
//! Always compiled (reqwest is already available).

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
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

        info!(url = %url, "rss_read: fetching feed");

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .user_agent("go-on/1.0")
            .build()
            .context("failed to build HTTP client")?;

        let response = client
            .get(url)
            .send()
            .with_context(|| format!("failed to fetch feed: {url}"))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .with_context(|| format!("failed to read body from {url}"))?;

        // Simple XML parsing for RSS 2.0 and Atom feeds
        // Uses basic string extraction to avoid adding an XML crate dependency
        let mut items = Vec::new();
        let body_lower = body.to_lowercase();

        // Detect feed type
        let feed_type = if body_lower.contains("<rss") {
            "RSS 2.0"
        } else if body_lower.contains("<feed")
            && body_lower.contains("xmlns=\"http://www.w3.org/2005/atom\"")
        {
            "Atom"
        } else if body_lower.contains("<feed") {
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

/// Extract the content of an XML tag (case-insensitive).
fn extract_xml_tag(text: &str, tag: &str) -> Option<String> {
    let open_tag_low = format!("<{}>", tag.to_lowercase());
    let close_tag_low = format!("</{}>", tag.to_lowercase());
    let text_lower = text.to_lowercase();

    let start = text_lower.find(&open_tag_low)?;
    let content_start = start + open_tag_low.len();
    let remaining = &text_lower[content_start..];
    let end = remaining.find(&close_tag_low)?;
    Some(text[content_start..content_start + end].to_string())
}
