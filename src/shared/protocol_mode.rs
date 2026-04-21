#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolMode {
    Adaptive,
    AcpStdio,
    AcpHttp,
    McpStdio,
    McpHttp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolModeError {
    FromConfigNotSupported,
    InvalidValue(String),
}

impl ProtocolMode {
    pub const CANONICAL_MODES: [&'static str; 5] = [
        "adaptive",
        "acp_stdio",
        "acp_http",
        "mcp_stdio",
        "mcp_http",
    ];

    pub fn from_str(value: &str) -> Result<Self, ProtocolModeError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "adaptive" => Ok(Self::Adaptive),
            "acp_stdio" | "acp+stdio" => Ok(Self::AcpStdio),
            "acp_http" | "acp+http" => Ok(Self::AcpHttp),
            "mcp_stdio" | "mcp+stdio" => Ok(Self::McpStdio),
            "mcp_http" | "mcp+http" => Ok(Self::McpHttp),
            "from_config" => Err(ProtocolModeError::FromConfigNotSupported),
            "auto" => Ok(Self::Adaptive),
            "acp" => Ok(Self::AcpStdio),
            "mcp" => Ok(Self::McpStdio),
            other => Err(ProtocolModeError::InvalidValue(other.to_string())),
        }
    }

    pub fn to_cli_arg(self) -> &'static str {
        match self {
            Self::Adaptive => "adaptive",
            Self::AcpStdio => "acp_stdio",
            Self::AcpHttp => "acp_http",
            Self::McpStdio => "mcp_stdio",
            Self::McpHttp => "mcp_http",
        }
    }

    pub fn parse_canonical(value: &str) -> Option<&'static str> {
        Self::from_str(value).ok().map(Self::to_cli_arg)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProtocolMode, ProtocolModeError};

    #[test]
    fn protocol_mode_accepts_legacy_aliases() {
        assert_eq!(ProtocolMode::from_str("auto").unwrap(), ProtocolMode::Adaptive);
        assert_eq!(ProtocolMode::from_str("acp").unwrap(), ProtocolMode::AcpStdio);
        assert_eq!(ProtocolMode::from_str("mcp").unwrap(), ProtocolMode::McpStdio);
    }

    #[test]
    fn protocol_mode_rejects_from_config_for_backend_cli() {
        assert_eq!(
            ProtocolMode::from_str("from_config"),
            Err(ProtocolModeError::FromConfigNotSupported)
        );
    }
}
