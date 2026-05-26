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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProtocolVersion(u16);
impl ProtocolVersion {
    pub const V1: Self = Self(1);
    #[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
    pub const LATEST: Self = Self::V1;
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

#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct EnvVariable {
    pub name: String,
    pub value: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}
#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
impl EnvVariable {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            meta: None,
        }
    }
}

#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}
#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
impl HttpHeader {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            meta: None,
        }
    }
}

// NOTE: All standard ACP methods are fully implemented. Handlers live in
// protocol_pack.rs and dispatch entries in request.rs:
//   - session/resume, session/close — session lifecycle management
//   - session/request_permission — permission response handler
//   - terminal/create, terminal/output, terminal/release, terminal/kill,
//     terminal/wait_for_exit — full terminal process management
#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
pub struct AcpMethodNames;
#[allow(dead_code)] // F-GAP-25 — reserved ACP protocol type from v0.13.2 spec
impl AcpMethodNames {
    pub const INITIALIZE: &'static str = "initialize";
    pub const AUTHENTICATE: &'static str = "authenticate";
    pub const LOGOUT: &'static str = "logout";
    pub const SESSION_NEW: &'static str = "session/new";
    pub const SESSION_LOAD: &'static str = "session/load";
    pub const SESSION_PROMPT: &'static str = "session/prompt";
    pub const SESSION_CANCEL: &'static str = "session/cancel";
    pub const SESSION_LIST: &'static str = "session/list";
    pub const SESSION_RESUME: &'static str = "session/resume";
    pub const SESSION_CLOSE: &'static str = "session/close";
    pub const SESSION_SET_MODE: &'static str = "session/set_mode";
    pub const SESSION_SET_CONFIG_OPTION: &'static str = "session/set_config_option";
    pub const SESSION_UPDATE: &'static str = "session/update";
    pub const SESSION_REQUEST_PERMISSION: &'static str = "session/request_permission";
    pub const TERMINAL_CREATE: &'static str = "terminal/create";
    pub const TERMINAL_OUTPUT: &'static str = "terminal/output";
    pub const TERMINAL_RELEASE: &'static str = "terminal/release";
    pub const TERMINAL_KILL: &'static str = "terminal/kill";
    pub const TERMINAL_WAIT_FOR_EXIT: &'static str = "terminal/wait_for_exit";
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
