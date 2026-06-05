//! ProtocolNegotiator — auto-detect and negotiate protocol mode
//!
//! Handles the 5 protocol modes:
//! - adaptive (auto)
//! - acp stdio
//! - acp http
//! - mcp stdio
//! - mcp http

// F-GAP-49: Module wired into production protocol pipeline.

use serde::{Deserialize, Serialize};
use tracing::warn;

pub use crate::schema::ProtocolVersion;
pub use crate::shared::protocol_mode::ProtocolMode;

/// Result of protocol negotiation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegotiatedProtocol {
    /// The agreed protocol mode
    pub mode: ProtocolMode,
    /// Protocol version string
    pub version: String,
    /// Whether the connection detected client capabilities
    pub auto_detected: bool,
    /// Negotiated ACP protocol version for initialize handshake
    pub protocol_version: ProtocolVersion,
}

/// ProtocolNegotiator — manages protocol auto-detection and fallback
#[derive(Debug)]
pub struct ProtocolNegotiator {
    /// Current active protocol mode
    active: ProtocolMode,
    /// Whether auto-detection is enabled
    #[allow(dead_code)]
    auto_detect: bool,
}

#[allow(dead_code)] // F-GAP-49 — used in tests; reserved for generic construction
impl Default for ProtocolNegotiator {
    fn default() -> Self {
        Self {
            active: ProtocolMode::Adaptive,
            auto_detect: true,
        }
    }
}

impl ProtocolNegotiator {
    pub fn new(mode: ProtocolMode) -> Self {
        Self {
            active: mode,
            auto_detect: mode == ProtocolMode::Adaptive,
        }
    }

    /// Negotiate protocol with a client hint.
    ///
    /// Uses the server's configured mode (`self.active`) as the default,
    /// or falls back to adaptive comparison when auto-detect is enabled.
    /// Fails fast with a clear error if the client hint is unrecognized.
    pub fn negotiate(&self, client_hint: Option<&str>) -> NegotiatedProtocol {
        let (mode, auto_detected) = if let Some(hint) = client_hint {
            match ProtocolMode::from_str(hint) {
                Ok(client_mode) => {
                    // Always compare priorities when a client hint is provided.
                    // The client hint represents the client's capability/preference,
                    // and the higher-priority mode should win regardless of auto_detect.
                    if client_mode.priority() > self.active.priority() {
                        (client_mode, true)
                    } else {
                        (self.active, true)
                    }
                }
                Err(_) => {
                    // Fail fast with clear error per P2 recommendation
                    panic!(
                        "unknown client protocol hint: '{}'. Supported modes: adaptive, acp_stdio, acp_http, mcp_stdio, mcp_http",
                        hint
                    );
                }
            }
        } else {
            // No client hint — use server configured mode
            (self.active, false)
        };

        NegotiatedProtocol {
            mode,
            version: format!("go-on/v1.1.0/{}", mode),
            auto_detected,
            protocol_version: ProtocolVersion::LATEST,
        }
    }
    /// Attempt fallback to next protocol in the chain
    #[allow(dead_code)] // F-GAP-49 — reserved for fallback orchestration
    pub fn try_fallback(&mut self) -> Option<ProtocolMode> {
        let fallback = self.active.fallback()?;
        warn!("protocol fallback: {} → {}", self.active, fallback);
        self.active = fallback;
        Some(fallback)
    }

    /// Get current active protocol
    #[allow(dead_code)] // F-GAP-49 — reserved for fallback orchestration
    pub fn active(&self) -> ProtocolMode {
        self.active
    }
}

/// Unified error code translation across protocols
#[allow(dead_code)] // F-GAP-49 — reserved for cross-protocol error mapping
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
    #[allow(dead_code)] // F-GAP-49 — reserved for cross-protocol error mapping
    pub fn to_mcp(&self) -> Self {
        // MCP uses the same JSON-RPC error codes
        self.clone()
    }

    /// Translate an MCP error to ACP-compatible error
    #[allow(dead_code)] // F-GAP-49 — reserved for cross-protocol error mapping
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
        // Default active is Adaptive (priority 5); McpHttp hint has priority 2.
        // Since 2 > 5 is false, the server mode (Adaptive) wins.
        let negotiator = ProtocolNegotiator::default();
        let result = negotiator.negotiate(Some("mcp_http"));
        assert_eq!(result.mode, ProtocolMode::Adaptive);
        assert!(result.auto_detected);
    }

    #[test]
    fn test_auto_negotiate_uses_server_mode_when_higher_priority() {
        // When server mode (AcpHttp, priority 4) is higher than client hint (McpStdio, priority 1).
        let negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        let result = negotiator.negotiate(Some("mcp_stdio"));
        assert_eq!(result.mode, ProtocolMode::AcpHttp);
        assert!(result.auto_detected);
    }

    #[test]
    fn test_auto_negotiate_prefers_client_when_higher_priority() {
        // Server is McpStdio (priority 1), client hints McpHttp (priority 2) — client wins.
        let negotiator = ProtocolNegotiator::new(ProtocolMode::McpStdio);
        let result = negotiator.negotiate(Some("mcp_http"));
        assert_eq!(result.mode, ProtocolMode::McpHttp);
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
        // Without a hint, uses the server's active mode (Adaptive by default)
        assert_eq!(result.mode, ProtocolMode::Adaptive);
        assert!(!result.auto_detected);
    }

    #[test]
    fn test_negotiate_without_hint_explicit_mode() {
        let negotiator = ProtocolNegotiator::new(ProtocolMode::McpHttp);
        let result = negotiator.negotiate(None);
        assert_eq!(result.mode, ProtocolMode::McpHttp);
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
    fn test_negotiate_protocol_version() {
        let negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        let result = negotiator.negotiate(Some("acp_http"));
        assert_eq!(result.protocol_version, ProtocolVersion::LATEST);
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
        assert_eq!(ProtocolMode::Adaptive.to_string(), "adaptive");
        assert_eq!(ProtocolMode::AcpHttp.to_string(), "acp-http");
        assert_eq!(ProtocolMode::McpStdio.to_string(), "mcp-stdio");
    }

    #[test]
    fn test_from_str() {
        assert_eq!(
            "auto".parse::<ProtocolMode>().unwrap(),
            ProtocolMode::Adaptive
        );
        assert_eq!(
            "acp-http".parse::<ProtocolMode>().unwrap(),
            ProtocolMode::AcpHttp
        );
        assert!("invalid".parse::<ProtocolMode>().is_err());
    }
}
