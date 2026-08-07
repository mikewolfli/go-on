//! ACP method name constants.
//!
//! All standard ACP method names are defined here as associated constants.
//! Handlers live in `acp/impl/` and dispatch through the ACP server.
//!   - session/resume, session/close — session lifecycle management
//!   - session/request_permission — permission response handler
//!   - terminal/create, terminal/output, terminal/release, terminal/kill,
//!     terminal/wait_for_exit — full terminal process management
//!
//! Moved from `protocol::acp_methods` to `acp::method_names` because
//! these are ACP-specific constants, not general protocol types.

/// ACP method name constants.
///
/// Provides compile-time checked method name strings for ACP protocol methods.
/// The canonical dispatch table lives in `acp/impl/request/protocol.rs`
/// (`ACP_METHODS`, sorted for `binary_search`); the constants below are kept
/// for reference/documentation and test assertions.
pub struct AcpMethodNames;

impl AcpMethodNames {
    /// Return true if the given method name is a known ACP method.
    ///
    /// Delegates to the authoritative dispatch table
    /// (`acp::r#impl::request::protocol::is_acp_request`, a sorted slice
    /// searched with `binary_search`). The previous `ALL` fast-path slice was
    /// removed because every entry in it is already present in that table,
    /// making the linear `contains` scan redundant on the hot path.
    pub fn is_known(method: &str) -> bool {
        crate::acp::r#impl::request::is_acp_request(method)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Named constants used only by tests (production callers use the
    // dispatch table directly via `is_known`).
    const INITIALIZE: &str = "initialize";
    const AUTHENTICATE: &str = "authenticate";
    const LOGOUT: &str = "logout";
    const SESSION_NEW: &str = "session/new";
    const SESSION_PROMPT: &str = "session/prompt";
    const SESSION_SET_MODE: &str = "session/set_mode";
    const TERMINAL_CREATE: &str = "terminal/create";
    const TERMINAL_WAIT_FOR_EXIT: &str = "terminal/wait_for_exit";

    #[test]
    fn test_constants_are_known() {
        // The named constants must all resolve through the authoritative
        // dispatch table so risk scoring never treats them as novel.
        assert!(AcpMethodNames::is_known(SESSION_NEW));
        assert!(AcpMethodNames::is_known(TERMINAL_CREATE));
        assert!(AcpMethodNames::is_known(INITIALIZE));
        assert!(AcpMethodNames::is_known(LOGOUT));
        assert!(AcpMethodNames::is_known(AUTHENTICATE));
        assert!(AcpMethodNames::is_known(SESSION_PROMPT));
        assert!(AcpMethodNames::is_known(SESSION_SET_MODE));
        assert!(AcpMethodNames::is_known(TERMINAL_WAIT_FOR_EXIT));
    }

    #[test]
    fn test_is_known() {
        assert!(AcpMethodNames::is_known("session/new"));
        assert!(AcpMethodNames::is_known("session/delete"));
        assert!(AcpMethodNames::is_known("terminal/create"));
        assert!(AcpMethodNames::is_known("workflow.execute"));
        assert!(AcpMethodNames::is_known("mcp.tools.call"));
        assert!(!AcpMethodNames::is_known("unknown/method"));
    }
}
