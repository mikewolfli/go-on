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
    match raw.trim().to_ascii_lowercase().as_str() {
        "adaptive" => Some("adaptive"),
        "acp_stdio" | "acp+stdio" => Some("acp_stdio"),
        "acp_http" | "acp+http" => Some("acp_http"),
        "mcp_stdio" | "mcp+stdio" => Some("mcp_stdio"),
        "mcp_http" | "mcp+http" => Some("mcp_http"),
        "auto" => Some("adaptive"),
        "acp" => Some("acp_stdio"),
        "mcp" => Some("mcp_stdio"),
        _ => None,
    }
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

pub fn resolve_access_selection(
    configured_mode: Option<&str>,
    http_bind: Option<&str>,
) -> AccessSelection {
    match canonical_configured_mode(configured_mode) {
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
        _ => unreachable!(),
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
