//! ProtocolNegotiator — auto-detect and negotiate protocol mode
//!
//! Handles the 5 protocol modes:
//! - auto (adaptive)
//! - acp stdio
//! - acp http
//! - mcp stdio
//! - mcp http

// F-GAP-49: Module not yet wired into production protocol pipeline.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolMode {
    Auto,
    AcpStdio,
    AcpHttp,
    McpStdio,
    McpHttp,
}

impl ProtocolMode {
    /// Priority order for negotiation (higher = more preferred)
    fn priority(self) -> u32 {
        match self {
            ProtocolMode::Auto => 5,
            ProtocolMode::AcpHttp => 4,
            ProtocolMode::AcpStdio => 3,
            ProtocolMode::McpHttp => 2,
            ProtocolMode::McpStdio => 1,
        }
    }

    /// Fallback chain: if current protocol fails, what to try next
    pub fn fallback(self) -> Option<ProtocolMode> {
        match self {
            ProtocolMode::AcpHttp => Some(ProtocolMode::AcpStdio),
            ProtocolMode::AcpStdio => Some(ProtocolMode::McpHttp),
            ProtocolMode::McpHttp => Some(ProtocolMode::McpStdio),
            ProtocolMode::McpStdio | ProtocolMode::Auto => None,
        }
    }

    /// Returns true if this mode uses HTTP transport
    pub fn is_http(self) -> bool {
        matches!(self, ProtocolMode::AcpHttp | ProtocolMode::McpHttp)
    }

    /// Returns true if this mode uses stdio transport
    pub fn is_stdio(self) -> bool {
        matches!(self, ProtocolMode::AcpStdio | ProtocolMode::McpStdio)
    }
}

impl FromStr for ProtocolMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" | "adaptive" => Ok(ProtocolMode::Auto),
            "acp_stdio" | "acp-stdio" | "acp stdio" => Ok(ProtocolMode::AcpStdio),
            "acp_http" | "acp-http" | "acp http" => Ok(ProtocolMode::AcpHttp),
            "mcp_stdio" | "mcp-stdio" | "mcp stdio" => Ok(ProtocolMode::McpStdio),
            "mcp_http" | "mcp-http" | "mcp http" => Ok(ProtocolMode::McpHttp),
            _ => Err(format!("unknown protocol mode: {s}")),
        }
    }
}

impl std::fmt::Display for ProtocolMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolMode::Auto => write!(f, "auto"),
            ProtocolMode::AcpStdio => write!(f, "acp-stdio"),
            ProtocolMode::AcpHttp => write!(f, "acp-http"),
            ProtocolMode::McpStdio => write!(f, "mcp-stdio"),
            ProtocolMode::McpHttp => write!(f, "mcp-http"),
        }
    }
}

/// Result of protocol negotiation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegotiatedProtocol {
    /// The agreed protocol mode
    pub mode: ProtocolMode,
    /// Protocol version string
    pub version: String,
    /// Whether the connection detected client capabilities
    pub auto_detected: bool,
}

/// ProtocolNegotiator — manages protocol auto-detection and fallback
#[derive(Debug)]
pub struct ProtocolNegotiator {
    /// Current active protocol mode
    active: ProtocolMode,
    /// Whether auto-detection is enabled
    auto_detect: bool,
}

impl Default for ProtocolNegotiator {
    fn default() -> Self {
        Self {
            active: ProtocolMode::Auto,
            auto_detect: true,
        }
    }
}

impl ProtocolNegotiator {
    pub fn new(mode: ProtocolMode) -> Self {
        Self {
            active: mode,
            auto_detect: mode == ProtocolMode::Auto,
        }
    }

    /// Negotiate protocol with a client hint
    pub fn negotiate(&self, client_hint: Option<&str>) -> NegotiatedProtocol {
        let mode = if let Some(hint) = client_hint {
            match ProtocolMode::from_str(hint) {
                Ok(client_mode) => {
                    if self.auto_detect {
                        // Pick the higher priority mode between client and server
                        let server_default = ProtocolMode::AcpHttp;
                        if client_mode.priority() > server_default.priority() {
                            client_mode
                        } else {
                            server_default
                        }
                    } else {
                        self.active
                    }
                }
                Err(_) => {
                    warn!("unknown client protocol hint: {hint}, using default");
                    ProtocolMode::AcpHttp
                }
            }
        } else {
            ProtocolMode::AcpHttp // Default to ACP HTTP
        };

        NegotiatedProtocol {
            mode,
            version: format!("go-on/v1.1.0/{}", mode),
            auto_detected: self.auto_detect && client_hint.is_some(),
        }
    }

    /// Attempt fallback to next protocol in the chain
    pub fn try_fallback(&mut self) -> Option<ProtocolMode> {
        let fallback = self.active.fallback()?;
        warn!("protocol fallback: {} → {}", self.active, fallback);
        self.active = fallback;
        Some(fallback)
    }

    /// Get current active protocol
    pub fn active(&self) -> ProtocolMode {
        self.active
    }
}

/// Unified error code translation across protocols
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    /// JSON-RPC error code
    pub code: i32,
    /// Human-readable message
    pub message: String,
    /// Protocol-specific error data
    pub data: Option<serde_json::Value>,
}

impl ProtocolError {
    /// Translate a JSON-RPC error to MCP-compatible error
    pub fn to_mcp(&self) -> Self {
        // MCP uses the same JSON-RPC error codes
        self.clone()
    }

    /// Translate an MCP error to ACP-compatible error
    pub fn to_acp(&self) -> Self {
        // ACP uses extended error codes
        let code = match self.code {
            -32700 => -32700, // Parse error
            -32600 => -32600, // Invalid request
            -32601 => -32601, // Method not found
            -32602 => -32602, // Invalid params
            -32603 => -32603, // Internal error
            _ => {
                if self.code <= -32000 && self.code > -32100 {
                    -32603 // Map MCP server errors to ACP internal error
                } else {
                    self.code
                }
            }
        };
        ProtocolError {
            code,
            message: self.message.clone(),
            data: self.data.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_negotiate_prefers_higher_priority() {
        let negotiator = ProtocolNegotiator::default();
        let result = negotiator.negotiate(Some("mcp_http"));
        assert_eq!(result.mode, ProtocolMode::AcpHttp);
        assert!(result.auto_detected);
    }

    #[test]
    fn test_explicit_mode_does_not_auto_detect() {
        let negotiator = ProtocolNegotiator::new(ProtocolMode::McpStdio);
        assert!(!negotiator.auto_detect);
    }

    #[test]
    fn test_negotiate_without_hint() {
        let negotiator = ProtocolNegotiator::default();
        let result = negotiator.negotiate(None);
        assert_eq!(result.mode, ProtocolMode::AcpHttp);
        assert!(!result.auto_detected);
    }

    #[test]
    fn test_fallback_chain() {
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        assert_eq!(negotiator.try_fallback(), Some(ProtocolMode::AcpStdio));
        assert_eq!(negotiator.try_fallback(), Some(ProtocolMode::McpHttp));
        assert_eq!(negotiator.try_fallback(), Some(ProtocolMode::McpStdio));
        assert_eq!(negotiator.try_fallback(), None);
    }

    #[test]
    fn test_protocol_error_translation() {
        let mcp_err = ProtocolError {
            code: -32000,
            message: "MCP server error".into(),
            data: None,
        };
        let acp_err = mcp_err.to_acp();
        assert_eq!(acp_err.code, -32603); // Mapped to ACP internal error
    }

    #[test]
    fn test_display() {
        assert_eq!(ProtocolMode::Auto.to_string(), "auto");
        assert_eq!(ProtocolMode::AcpHttp.to_string(), "acp-http");
        assert_eq!(ProtocolMode::McpStdio.to_string(), "mcp-stdio");
    }

    #[test]
    fn test_from_str() {
        assert_eq!("auto".parse::<ProtocolMode>().unwrap(), ProtocolMode::Auto);
        assert_eq!(
            "acp-http".parse::<ProtocolMode>().unwrap(),
            ProtocolMode::AcpHttp
        );
        assert!("invalid".parse::<ProtocolMode>().is_err());
    }
}
