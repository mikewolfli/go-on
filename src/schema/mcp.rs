//! MCP server configuration types mirroring `agent-client-protocol-schema` v0.13.2
//! agent.rs McpServer types. These are part of the ACP protocol spec and are
//! re-exported as public API surface even though nothing currently consumes them.
//!
//! #[allow(dead_code)] is legitimate here: these types are a protocol-spec mirror
//! published as a public API surface, not stubs or placeholders. They will be
//! wired when ACP MCP-config handlers are connected. This is NOT the
//! integration_gate anti-pattern (Rule #14) because the types are real, complete,
//! and match the external spec — not #[cfg(test)] stubs.
#![allow(dead_code, reason = "protocol-spec mirror — public API surface")]

use crate::schema::{EnvVariable, HttpHeader, Meta};
use serde::{Deserialize, Serialize};

/// MCP server configuration. Tagged by `"type"` in JSON.
/// Stdio variant is untagged (legacy format without explicit type field).
/// Public schema type — mirrors `agent-client-protocol-schema` v0.13.2.
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

/// HTTP MCP server transport configuration.
/// Public schema type — part of the ACP MCP server config enum.
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

/// SSE MCP server transport configuration.
/// Public schema type — part of the ACP MCP server config enum.
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

impl From<McpServerStdio> for McpServerConfig {
    fn from(s: McpServerStdio) -> Self {
        McpServerConfig::Stdio(s)
    }
}
