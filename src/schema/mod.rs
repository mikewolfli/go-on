//! ACP + MCP Protocol Type Definitions
//!
//! Complete Rust type definitions for the Agent Client Protocol (ACP) v1
//! and Model Context Protocol (MCP), mirroring the official
//! `agent-client-protocol-schema` v0.13.2 crate.
//!
//! Source of truth: https://github.com/agentclientprotocol/agent-client-protocol
//!
//! Many types are defined here for future use and may trigger dead_code
//! warnings until they are adopted by the ACP handlers.

//!
//! ## Serde rules
//!
//! | Rule | Applies to |
//! |------|-----------|
//! | `rename_all = "camelCase"` | All structs |
//! | `rename_all = "snake_case"` | All enums (StopReason, SessionUpdate, etc.) |
//! | `tag = "sessionUpdate"` | SessionUpdate enum discriminator |
//! | `tag = "type"` | ContentBlock, AuthMethod, McpServer, SessionConfigKind |
//! | `skip_serializing_none` | Optional fields |
//! | `#[serde(rename = "_meta")]` | Extensibility field |
//! | `transparent` | Newtype wrappers |

mod agent;
mod client;
mod content;
mod mcp;
mod skills;

#[allow(unused_imports)]
pub use agent::*; // re-exported for ACP spec public API surface
#[allow(unused_imports)]
pub use client::*; // re-exported for ACP spec public API surface
#[allow(unused_imports)]
pub use content::*; // re-exported for ACP spec public API surface
#[allow(unused_imports)]
pub use mcp::*; // re-exported for ACP spec public API surface
#[allow(unused_imports)]
pub use skills::*; // re-exported for ACP spec public API surface

use serde::{Deserialize, Serialize};
// ═══════════════════════════════════════════════════════════════════════════════
// Core shared types
// ═══════════════════════════════════════════════════════════════════════════════

/// Session ID (transparent string in JSON).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct SessionId(pub String);
impl SessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}
impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct ProtocolVersion(u16);
impl ProtocolVersion {
    pub const V1: Self = Self(1);
    pub const V2: Self = Self(2);
    pub const V3: Self = Self(3);
    pub const LATEST: Self = Self::V3;
    // NOTE: When adding future versions (V4, V5, ...), update
    // `supported_versions()` below and bump `LATEST` to the newest constant.

    /// Return an ordered list of all supported versions (ascending).
    pub fn supported_versions() -> &'static [Self] {
        &[Self::V1, Self::V2, Self::V3]
    }

    /// Return the underlying numeric version.
    pub fn as_u16(self) -> u16 {
        self.0
    }
}

pub type Meta = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct Implementation {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}
impl Implementation {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: None,
            version: version.into(),
            meta: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct EnvVariable {
    pub name: String,
    pub value: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// HTTP header — name/value pair with optional metadata.
/// Used in MCP HTTP/SSE transport configurations.
/// Currently only consumed by schema::mcp protocol-spec types.
#[allow(dead_code, reason = "used by protocol-spec mcp.rs types")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

// Session ID types used across agent/client modules

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct SessionModeId(pub String);
impl SessionModeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}
impl From<String> for SessionModeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for SessionModeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct SessionConfigId(pub String);
impl SessionConfigId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}
impl From<String> for SessionConfigId {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for SessionConfigId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct SessionConfigValueId(pub String);
impl SessionConfigValueId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}
impl From<String> for SessionConfigValueId {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for SessionConfigValueId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct SessionConfigGroupId(pub String);
impl SessionConfigGroupId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}
impl From<String> for SessionConfigGroupId {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for SessionConfigGroupId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
