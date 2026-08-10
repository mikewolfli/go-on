use crate::shared::protocol_mode::ProtocolMode;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolCapability {
    AcpOnly,
    McpOnly,
    DualStack,
}

impl ProtocolCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcpOnly => "acp_only",
            Self::McpOnly => "mcp_only",
            Self::DualStack => "dual_stack",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDispatchMode {
    Auto,
    Acp,
    Mcp,
}

impl RequestDispatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Acp => "acp",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Stdio,
    Http,
}

impl TransportMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessSelection {
    pub configured_mode: String,
    pub protocol_capability: ProtocolCapability,
    pub request_dispatch_mode: RequestDispatchMode,
    pub startup_transport: TransportMode,
    pub transport_strategy: &'static str,
    pub selection_reason: &'static str,
}

pub fn normalize_protocol_mode(raw: &str) -> Option<&'static str> {
    ProtocolMode::parse_canonical(raw)
}

pub fn canonical_configured_mode(raw: Option<&str>) -> &'static str {
    raw.and_then(normalize_protocol_mode).unwrap_or("adaptive")
}

pub fn request_dispatch_mode(raw: Option<&str>) -> RequestDispatchMode {
    match canonical_configured_mode(raw) {
        "adaptive" => RequestDispatchMode::Auto,
        "acp_stdio" | "acp_http" => RequestDispatchMode::Acp,
        "mcp_stdio" | "mcp_http" => RequestDispatchMode::Mcp,
        _ => RequestDispatchMode::Auto,
    }
}

fn has_http_bind(http_bind: Option<&str>) -> bool {
    http_bind
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

/// Resolve the effective access selection from the configured protocol mode
/// and HTTP bind address.
///
/// ## Dual-stack (adaptive) method-routing semantics
///
/// In `adaptive` mode (`RequestDispatchMode::Auto`) both ACP and MCP clients
/// are served. Bare standard MCP method names (`ping`, `tools/list`,
/// `resources/*`, `notifications/initialized`, ...) are normalized to their
/// `mcp.*` form before dispatch (`normalize_mcp_method` in
/// `acp/impl/request/protocol.rs`), so MCP clients route to the `mcp.*`
/// handlers. `initialize` is the deliberate exception: it keeps ACP semantics
/// in Auto mode so an ACP client's handshake is not hijacked by the MCP
/// bridge. ACP method names (e.g. `chat`, `session/new`) pass through
/// unchanged.
pub fn resolve_access_selection(
    configured_mode: Option<&str>,
    http_bind: Option<&str>,
) -> AccessSelection {
    let resolved = canonical_configured_mode(configured_mode);
    match resolved {
        "adaptive" => {
            if has_http_bind(http_bind) {
                AccessSelection {
                    configured_mode: "adaptive".to_string(),
                    protocol_capability: ProtocolCapability::DualStack,
                    request_dispatch_mode: RequestDispatchMode::Auto,
                    startup_transport: TransportMode::Http,
                    transport_strategy: "client_adaptive_http_available",
                    selection_reason: "adaptive_client_type_with_http_available",
                }
            } else {
                AccessSelection {
                    configured_mode: "adaptive".to_string(),
                    protocol_capability: ProtocolCapability::DualStack,
                    request_dispatch_mode: RequestDispatchMode::Auto,
                    startup_transport: TransportMode::Stdio,
                    transport_strategy: "client_adaptive_stdio_only",
                    selection_reason: "adaptive_client_type_stdio_only",
                }
            }
        }
        "acp_stdio" => AccessSelection {
            configured_mode: "acp_stdio".to_string(),
            protocol_capability: ProtocolCapability::AcpOnly,
            request_dispatch_mode: RequestDispatchMode::Acp,
            startup_transport: TransportMode::Stdio,
            transport_strategy: "fixed_from_config",
            selection_reason: "configured_explicit_mode",
        },
        "acp_http" => AccessSelection {
            configured_mode: "acp_http".to_string(),
            protocol_capability: ProtocolCapability::AcpOnly,
            request_dispatch_mode: RequestDispatchMode::Acp,
            startup_transport: TransportMode::Http,
            transport_strategy: "fixed_from_config",
            selection_reason: "configured_explicit_mode",
        },
        "mcp_stdio" => AccessSelection {
            configured_mode: "mcp_stdio".to_string(),
            protocol_capability: ProtocolCapability::McpOnly,
            request_dispatch_mode: RequestDispatchMode::Mcp,
            startup_transport: TransportMode::Stdio,
            transport_strategy: "fixed_from_config",
            selection_reason: "configured_explicit_mode",
        },
        "mcp_http" => AccessSelection {
            configured_mode: "mcp_http".to_string(),
            protocol_capability: ProtocolCapability::McpOnly,
            request_dispatch_mode: RequestDispatchMode::Mcp,
            startup_transport: TransportMode::Http,
            transport_strategy: "fixed_from_config",
            selection_reason: "configured_explicit_mode",
        },
        other => {
            warn!(
                "unknown protocol mode '{}' — falling back to adaptive",
                other
            );
            AccessSelection {
                configured_mode: "adaptive".to_string(),
                protocol_capability: ProtocolCapability::DualStack,
                request_dispatch_mode: RequestDispatchMode::Auto,
                startup_transport: TransportMode::Stdio,
                transport_strategy: "fallback_from_unknown",
                selection_reason: "fallback_unknown_mode",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{request_dispatch_mode, resolve_access_selection, RequestDispatchMode};

    #[test]
    fn adaptive_without_http_bind_defaults_to_acp_stdio() {
        let selection = resolve_access_selection(Some("adaptive"), None);
        assert_eq!(selection.configured_mode, "adaptive");
        assert_eq!(selection.protocol_capability.as_str(), "dual_stack");
        assert_eq!(selection.request_dispatch_mode.as_str(), "auto");
        assert_eq!(selection.startup_transport.as_str(), "stdio");
    }

    #[test]
    fn adaptive_with_http_bind_resolves_to_acp_http() {
        let selection = resolve_access_selection(Some("adaptive"), Some("127.0.0.1:8090"));
        assert_eq!(selection.protocol_capability.as_str(), "dual_stack");
        assert_eq!(selection.startup_transport.as_str(), "http");
    }

    #[test]
    fn aliases_are_normalized_to_canonical_modes() {
        let selection = resolve_access_selection(Some("mcp"), None);
        assert_eq!(selection.configured_mode, "mcp_stdio");
        assert_eq!(selection.protocol_capability.as_str(), "mcp_only");
        assert_eq!(request_dispatch_mode(Some("acp")), RequestDispatchMode::Acp);
    }
}
