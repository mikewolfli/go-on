//! HTTP request tool

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;
use tracing::debug;

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

pub struct HttpRequestTool;

impl Tool for HttpRequestTool {
    fn name(&self) -> &'static str {
        "http_request"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        // Primary: url from function call arguments.
        // Fallback: extract URL from the task objective when the AI model
        // fails to pass the url argument (common in some models).
        let url = if let Some(url_str) = input.payload["url"].as_str() {
            url_str.to_string()
        } else {
            extract_url(&input.objective)
                .or_else(|| extract_url(&input.payload.to_string()))
                .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_url")))?
        };
        let method = input.payload["method"].as_str().unwrap_or("GET");
        let body = input.payload["body"].as_str();

        // Environment-specific override: read timeout from payload, fall back to
        // env var OVERRIDE_TIMEOUT_MS, then to default 15_000ms.
        let timeout_ms = input.payload["timeout_ms"]
            .as_u64()
            .or_else(|| {
                std::env::var("OVERRIDE_TIMEOUT_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .unwrap_or(15_000);

        debug!(method = %method, url = %url, "tool: making HTTP request");

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
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

        // Custom headers from payload["headers"] as a JSON object
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

        // Bearer auth from payload["auth"]["bearer"]
        if let Some(auth_obj) = input.payload["auth"].as_object() {
            if let Some(bearer_token) = auth_obj.get("bearer").and_then(Value::as_str) {
                request_builder = request_builder.bearer_auth(bearer_token);
            }
        }

        // Query parameters from payload["query"] as a JSON object
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
        let success = (200..400).contains(&status);

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
            audit_log: Some(format!("HTTP {} {} -> {}", method, url, status)),
            pua_report: Some(tool_execution_report(
                "http_request",
                Some("http_request_completed"),
            )),
        })
    }
}
