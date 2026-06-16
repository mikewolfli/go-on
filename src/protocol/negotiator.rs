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
use tracing::{info, warn};

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
    /// The list of client-supported versions that led to this negotiation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_versions: Option<Vec<u16>>,
}

/// ProtocolNegotiator — manages protocol auto-detection and fallback
#[derive(Debug)]
pub struct ProtocolNegotiator {
    /// Current active protocol mode
    active: ProtocolMode,
    /// Whether auto-detection is enabled
    pub auto_detect: bool,
}

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

    /// Negotiate protocol with a client hint and optional client-supported versions.
    ///
    /// Uses the server's configured mode (`self.active`) as the default.
    /// When an unrecognized hint is received and no fallback is available,
    /// the error is logged and the server's active mode is returned unchanged
    /// with `auto_detected = false`. Callers should check `auto_detected` to
    /// distinguish server-default from negotiated outcomes.
    ///
    /// # Version negotiation
    ///
    /// When `client_versions` is provided, the highest mutually-supported protocol
    /// version is selected via [`select_highest_common`].  If no common version
    /// exists, [`ProtocolVersion::LATEST`] is used as a backward-compatible
    /// fallback.
    pub fn negotiate(
        &mut self,
        client_hint: Option<&str>,
        client_versions: Option<&[ProtocolVersion]>,
    ) -> NegotiatedProtocol {
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
                    warn!(
                        "unknown client protocol hint: '{}' — attempting fallback",
                        hint
                    );
                    // If auto-detect is enabled, attempt fallback through protocol chain.
                    if self.auto_detect {
                        match self.try_fallback() {
                            Some(fallback_mode) => {
                                info!(
                                    "protocol fallback to {} after unknown hint '{}'",
                                    fallback_mode, hint
                                );
                                (fallback_mode, true)
                            }
                            None => {
                                let err = ProtocolError::NegotiationFailed {
                                    reason: format!(
                                        "unknown protocol hint '{hint}' and no fallback available"
                                    ),
                                };
                                warn!("{}", err);
                                (self.active, false)
                            }
                        }
                    } else {
                        (self.active, false)
                    }
                }
            }
        } else {
            // No client hint — use server configured mode
            (self.active, false)
        };

        // Real version negotiation: descend from LATEST until a common version is found.
        let (protocol_version, client_versions_list) = match client_versions {
            Some(versions) => {
                let negotiated =
                    Self::select_highest_common(versions).unwrap_or(ProtocolVersion::LATEST);
                (
                    negotiated,
                    Some(versions.iter().map(|v| v.as_u16()).collect()),
                )
            }
            None => (ProtocolVersion::LATEST, None),
        };

        NegotiatedProtocol {
            mode,
            version: format!("go-on/v1.1.0/{}", mode),
            auto_detected,
            protocol_version,
            client_versions: client_versions_list,
        }
    }

    /// Select the highest protocol version supported by both the server and the client.
    ///
    /// Iterates the server's supported versions in descending order (V3 → V2 → V1)
    /// and returns the first one present in `client_versions`.  Returns `None` when
    /// there is no overlap at all.
    pub fn select_highest_common(client_versions: &[ProtocolVersion]) -> Option<ProtocolVersion> {
        ProtocolVersion::select_highest_common(client_versions)
    }

    /// Negotiate protocol with a client hint and client-supported versions.
    ///
    /// This is a convenience wrapper around [`negotiate`] that accepts
    /// a `&[ProtocolVersion]` directly instead of `Option<&[ProtocolVersion]>`.
    #[allow(dead_code)] // Public API for test consumers
    pub fn negotiate_with_versions(
        &mut self,
        client_hint: Option<&str>,
        client_versions: &[ProtocolVersion],
    ) -> NegotiatedProtocol {
        self.negotiate(client_hint, Some(client_versions))
    }

    /// Attempt fallback to next protocol in the chain
    pub fn try_fallback(&mut self) -> Option<ProtocolMode> {
        let fallback = self.active.fallback()?;
        warn!("protocol fallback: {} → {}", self.active, fallback);
        self.active = fallback;
        Some(fallback)
    }
}

/// Protocol-level errors with semantic variants for cross-protocol error mapping.
///
/// Each variant carries the contextual detail needed to produce a meaningful
/// error message in the target protocol.
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::VersionMismatch { expected, got } => {
                write!(f, "version mismatch: expected {}, got {}", expected, got)
            }
            ProtocolError::NegotiationFailed { reason } => {
                write!(f, "negotiation failed: {}", reason)
            }
            ProtocolError::TransportError { detail } => {
                write!(f, "transport error: {}", detail)
            }
            ProtocolError::UnsupportedMethod { method } => {
                write!(f, "unsupported method: {}", method)
            }
            ProtocolError::CapabilityMissing { capability } => {
                write!(f, "capability missing: {}", capability)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolError {
    /// Protocol version mismatch between client and server
    VersionMismatch {
        /// The version the server expected
        expected: String,
        /// The version the client provided
        got: String,
    },
    /// Protocol negotiation failed (e.g. incompatible modes)
    NegotiationFailed {
        /// Human-readable reason for the failure
        reason: String,
    },
    /// Transport-level I/O or connection error
    TransportError {
        /// Details about the transport failure
        detail: String,
    },
    /// Client requested a method the server does not recognise
    UnsupportedMethod {
        /// Name of the unsupported method
        method: String,
    },
    /// A required capability is missing from the peer
    CapabilityMissing {
        /// Name of the missing capability
        capability: String,
    },
}

impl ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_negotiate_prefers_higher_priority() {
        // Default active is Adaptive (priority 5); McpHttp hint has priority 2.
        // Since 2 > 5 is false, the server mode (Adaptive) wins.
        let mut negotiator = ProtocolNegotiator::default();
        let result = negotiator.negotiate(Some("mcp_http"), None);
        assert_eq!(result.mode, ProtocolMode::Adaptive);
        assert!(result.auto_detected);
    }

    #[test]
    fn test_auto_negotiate_uses_server_mode_when_higher_priority() {
        // When server mode (AcpHttp, priority 4) is higher than client hint (McpStdio, priority 1).
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        let result = negotiator.negotiate(Some("mcp_stdio"), None);
        assert_eq!(result.mode, ProtocolMode::AcpHttp);
        assert!(result.auto_detected);
    }

    #[test]
    fn test_auto_negotiate_prefers_client_when_higher_priority() {
        // Server is McpStdio (priority 1), client hints McpHttp (priority 2) — client wins.
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::McpStdio);
        let result = negotiator.negotiate(Some("mcp_http"), None);
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
        let mut negotiator = ProtocolNegotiator::default();
        let result = negotiator.negotiate(None, None);
        // Without a hint, uses the server's active mode (Adaptive by default)
        assert_eq!(result.mode, ProtocolMode::Adaptive);
        assert!(!result.auto_detected);
    }

    #[test]
    fn test_negotiate_without_hint_explicit_mode() {
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::McpHttp);
        let result = negotiator.negotiate(None, None);
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
    fn test_negotiate_protocol_version_no_client_versions() {
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        // With no client versions supplied, defaults to LATEST.
        let result = negotiator.negotiate(Some("acp_http"), None);
        assert_eq!(result.protocol_version, ProtocolVersion::LATEST);
        assert!(result.client_versions.is_none());
    }

    #[test]
    fn test_negotiate_with_client_versions_selects_highest_common() {
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        let client_versions = vec![ProtocolVersion::V1, ProtocolVersion::V2];
        let result = negotiator.negotiate(Some("acp_http"), Some(&client_versions));
        // Highest common between server {1,2,3} and client {1,2} is V2
        assert_eq!(result.protocol_version, ProtocolVersion::V2);
        assert_eq!(result.client_versions, Some(vec![1, 2]));
    }

    #[test]
    fn test_negotiate_with_versions_only_v1() {
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        let client_versions = vec![ProtocolVersion::V1];
        let result = negotiator.negotiate(Some("acp_http"), Some(&client_versions));
        assert_eq!(result.protocol_version, ProtocolVersion::V1);
    }

    #[test]
    fn test_negotiate_with_versions_fallback_when_no_overlap() {
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        let client_versions = vec![ProtocolVersion::from_u16(999)];
        let result = negotiator.negotiate(Some("acp_http"), Some(&client_versions));
        // Falls back to LATEST for backward compatibility
        assert_eq!(result.protocol_version, ProtocolVersion::LATEST);
    }

    #[test]
    fn test_negotiate_with_versions_selects_highest_common() {
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        let client_versions = vec![ProtocolVersion::V1, ProtocolVersion::V3];
        let result = negotiator.negotiate_with_versions(Some("acp_http"), &client_versions);
        // Highest common between server {1,2,3} and client {1,3} is V3
        assert_eq!(result.protocol_version, ProtocolVersion::V3);
        assert_eq!(result.client_versions, Some(vec![1, 3]));
    }

    #[test]
    fn test_negotiate_with_versions_falls_back_to_latest() {
        let mut negotiator = ProtocolNegotiator::new(ProtocolMode::AcpHttp);
        // Client only supports V999, which server doesn't have
        let client_versions = vec![ProtocolVersion::from_u16(999)];
        let result = negotiator.negotiate_with_versions(Some("acp_http"), &client_versions);
        // Falls back to LATEST for backward compatibility
        assert_eq!(result.protocol_version, ProtocolVersion::LATEST);
    }

    #[test]
    fn test_select_highest_common() {
        let client = vec![ProtocolVersion::V1, ProtocolVersion::V2];
        assert_eq!(
            ProtocolVersion::select_highest_common(&client),
            Some(ProtocolVersion::V2)
        );

        let client = vec![ProtocolVersion::V1];
        assert_eq!(
            ProtocolVersion::select_highest_common(&client),
            Some(ProtocolVersion::V1)
        );

        // No overlap
        let client = vec![ProtocolVersion::from_u16(42)];
        assert_eq!(ProtocolVersion::select_highest_common(&client), None);
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
