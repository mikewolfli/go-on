//! MCP server configuration types (used within ACP session/new requests).
//! Mirrors `agent-client-protocol-schema` v0.13.2 agent.rs McpServer types.

use crate::schema::{EnvVariable, HttpHeader, Meta};
use serde::{Deserialize, Serialize};

/// MCP server configuration. Tagged by `"type"` in JSON.
/// Stdio variant is untagged (legacy format without explicit type field).
// activated, formerly F-GAP-25 — used by agent.rs NewSessionRequest / LoadSessionRequest / ResumeSessionRequest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServerConfig {
    /// HTTP transport (requires agent mcpCapabilities.http).
    Http(McpServerHttp),
    /// SSE transport (requires agent mcpCapabilities.sse).
    Sse(McpServerSse),
    /// Stdio transport (all agents MUST support this).
    #[serde(untagged)]
    Stdio(McpServerStdio),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerHttp {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HttpHeader>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerSse {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HttpHeader>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStdio {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVariable>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

impl McpServerStdio {
    #[allow(dead_code)] // Public API for MCP server consumers
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: vec![],
            env: vec![],
            meta: None,
        }
    }
}

impl From<McpServerStdio> for McpServerConfig {
    fn from(s: McpServerStdio) -> Self {
        McpServerConfig::Stdio(s)
    }
}
