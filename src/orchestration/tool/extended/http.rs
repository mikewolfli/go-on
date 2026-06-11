//! HTTP request tool

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use std::time::Duration;
use tracing::debug;

pub struct HttpRequestTool;

impl Tool for HttpRequestTool {
    fn name(&self) -> &'static str {
        "http_request"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let url = input.payload["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_url")))?;
        let method = input.payload["method"].as_str().unwrap_or("GET");
        let body = input.payload["body"].as_str();
        let timeout_ms = input.payload["timeout_ms"].as_u64().unwrap_or(15_000);

        debug!(method = %method, url = %url, "tool: making HTTP request");

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .context("failed to build HTTP client")?;

        let request_builder = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => {
                let mut builder = client.post(url);
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
