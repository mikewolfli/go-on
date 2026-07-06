use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    AmbiguousPrefix(String),
}

impl ProtocolMode {
    /// Priority order for negotiation (higher = more preferred).
    /// Not yet wired into production path.
    #[allow(dead_code)]
    pub fn priority(self) -> u32 {
        match self {
            ProtocolMode::Adaptive => 5,
            ProtocolMode::AcpHttp => 4,
            ProtocolMode::AcpStdio => 3,
            ProtocolMode::McpHttp => 2,
            ProtocolMode::McpStdio => 1,
        }
    }

    /// Fallback chain: if current protocol fails, what to try next.
    /// Not yet wired into production path.
    #[allow(dead_code)]
    pub fn fallback(self) -> Option<ProtocolMode> {
        match self {
            ProtocolMode::AcpHttp => Some(ProtocolMode::AcpStdio),
            ProtocolMode::AcpStdio => Some(ProtocolMode::McpHttp),
            ProtocolMode::McpHttp => Some(ProtocolMode::McpStdio),
            ProtocolMode::McpStdio | ProtocolMode::Adaptive => None,
        }
    }

    pub const CANONICAL_MODES: [&'static str; 5] =
        ["adaptive", "acp_stdio", "acp_http", "mcp_stdio", "mcp_http"];

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Result<Self, ProtocolModeError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "adaptive" => Ok(Self::Adaptive),
            "acp_stdio" | "acp+stdio" | "acp-stdio" => Ok(Self::AcpStdio),
            "acp_http" | "acp+http" | "acp-http" => Ok(Self::AcpHttp),
            "mcp_stdio" | "mcp+stdio" | "mcp-stdio" => Ok(Self::McpStdio),
            "mcp_http" | "mcp+http" | "mcp-http" => Ok(Self::McpHttp),
            "from_config" => Err(ProtocolModeError::FromConfigNotSupported),
            "auto" => Ok(Self::Adaptive),
            "acp" => Ok(Self::AcpStdio),
            "mcp" => Ok(Self::McpStdio),
            other => Err(ProtocolModeError::InvalidValue(other.to_string())),
        }
    }
}

impl std::str::FromStr for ProtocolMode {
    type Err = ProtocolModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        <Self>::from_str(s)
    }
}

impl ProtocolMode {
    pub fn from_fuzzy(value: &str) -> Result<Self, ProtocolModeError> {
        let trimmed = value.trim().to_ascii_lowercase();
        if trimmed.is_empty() {
            return Err(ProtocolModeError::InvalidValue(trimmed));
        }

        match Self::from_str(&trimmed) {
            Ok(mode) => return Ok(mode),
            Err(ProtocolModeError::FromConfigNotSupported) => {
                return Err(ProtocolModeError::FromConfigNotSupported)
            }
            Err(ProtocolModeError::InvalidValue(_))
            | Err(ProtocolModeError::AmbiguousPrefix(_)) => {}
        }

        let mut matched = Self::CANONICAL_MODES
            .iter()
            .copied()
            .filter(|mode| mode.starts_with(&trimmed));

        let first = matched.next();
        let second = matched.next();

        match (first, second) {
            (Some("adaptive"), None) => Ok(Self::Adaptive),
            (Some("acp_stdio"), None) => Ok(Self::AcpStdio),
            (Some("acp_http"), None) => Ok(Self::AcpHttp),
            (Some("mcp_stdio"), None) => Ok(Self::McpStdio),
            (Some("mcp_http"), None) => Ok(Self::McpHttp),
            (Some(_), Some(_)) => Err(ProtocolModeError::AmbiguousPrefix(trimmed)),
            _ => Err(ProtocolModeError::InvalidValue(trimmed)),
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

impl std::fmt::Display for ProtocolMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolMode::Adaptive => write!(f, "adaptive"),
            ProtocolMode::AcpStdio => write!(f, "acp-stdio"),
            ProtocolMode::AcpHttp => write!(f, "acp-http"),
            ProtocolMode::McpStdio => write!(f, "mcp-stdio"),
            ProtocolMode::McpHttp => write!(f, "mcp-http"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProtocolMode, ProtocolModeError};

    #[test]
    fn protocol_mode_accepts_legacy_aliases() {
        assert_eq!(
            ProtocolMode::from_str("auto").unwrap(),
            ProtocolMode::Adaptive
        );
        assert_eq!(
            ProtocolMode::from_str("acp").unwrap(),
            ProtocolMode::AcpStdio
        );
        assert_eq!(
            ProtocolMode::from_str("mcp").unwrap(),
            ProtocolMode::McpStdio
        );
    }

    #[test]
    fn protocol_mode_rejects_from_config_for_backend_cli() {
        assert_eq!(
            ProtocolMode::from_str("from_config"),
            Err(ProtocolModeError::FromConfigNotSupported)
        );
    }

    #[test]
    fn protocol_mode_accepts_unique_prefixes() {
        assert_eq!(
            ProtocolMode::from_fuzzy("adap").unwrap(),
            ProtocolMode::Adaptive
        );
        assert_eq!(
            ProtocolMode::from_fuzzy("mcp-http").unwrap(),
            ProtocolMode::McpHttp
        );
        assert_eq!(
            ProtocolMode::from_fuzzy("acp_h").unwrap(),
            ProtocolMode::AcpHttp
        );
    }

    #[test]
    fn protocol_mode_rejects_ambiguous_prefix() {
        assert_eq!(
            ProtocolMode::from_fuzzy("acp_").unwrap_err(),
            ProtocolModeError::AmbiguousPrefix("acp_".to_string())
        );
    }
}
