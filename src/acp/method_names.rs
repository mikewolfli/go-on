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
/// Used by request dispatch, governance risk scoring, and method validation.
pub struct AcpMethodNames;

impl AcpMethodNames {
    pub const INITIALIZE: &'static str = "initialize";
    pub const AUTHENTICATE: &'static str = "authenticate";
    pub const LOGOUT: &'static str = "logout";
    pub const SESSION_NEW: &'static str = "session/new";
    pub const SESSION_LOAD: &'static str = "session/load";
    pub const SESSION_PROMPT: &'static str = "session/prompt";
    pub const SESSION_CANCEL: &'static str = "session/cancel";
    pub const SESSION_DELETE: &'static str = "session/delete";
    pub const SESSION_LIST: &'static str = "session/list";
    pub const SESSION_RESUME: &'static str = "session/resume";
    pub const SESSION_CLOSE: &'static str = "session/close";
    pub const SESSION_SET_MODE: &'static str = "session/set_mode";
    pub const SESSION_SET_CONFIG_OPTION: &'static str = "session/set_config_option";
    pub const SESSION_CONFIG_SET: &'static str = "session/config/set";
    pub const SESSION_CONFIG_GET: &'static str = "session/config/get";
    pub const SESSION_REQUEST_PERMISSION: &'static str = "session/request_permission";
    pub const TERMINAL_CREATE: &'static str = "terminal/create";
    pub const TERMINAL_OUTPUT: &'static str = "terminal/output";
    pub const TERMINAL_RELEASE: &'static str = "terminal/release";
    pub const TERMINAL_KILL: &'static str = "terminal/kill";
    pub const TERMINAL_WAIT_FOR_EXIT: &'static str = "terminal/wait_for_exit";

    /// The canonical ACP method-name constants, as a slice for iteration.
    ///
    /// Used as a fast-path in [`is_known`](Self::is_known) and available for
    /// introspection; the authoritative dispatch gate remains
    /// `acp::impl::request::protocol::is_acp_request` (the 140-entry table),
    /// which this slice deliberately does not try to replace.
    pub const ALL: &[&str] = &[
        Self::INITIALIZE,
        Self::AUTHENTICATE,
        Self::LOGOUT,
        Self::SESSION_NEW,
        Self::SESSION_LOAD,
        Self::SESSION_PROMPT,
        Self::SESSION_CANCEL,
        Self::SESSION_DELETE,
        Self::SESSION_LIST,
        Self::SESSION_RESUME,
        Self::SESSION_CLOSE,
        Self::SESSION_SET_MODE,
        Self::SESSION_SET_CONFIG_OPTION,
        Self::SESSION_CONFIG_SET,
        Self::SESSION_CONFIG_GET,
        Self::SESSION_REQUEST_PERMISSION,
        Self::TERMINAL_CREATE,
        Self::TERMINAL_OUTPUT,
        Self::TERMINAL_RELEASE,
        Self::TERMINAL_KILL,
        Self::TERMINAL_WAIT_FOR_EXIT,
    ];

    /// Return true if the given method name is a known ACP method.
    ///
    /// Fast path over the canonical constants, with the authoritative
    /// 140-entry dispatch table (`acp::impl::request::protocol::is_acp_request`)
    /// as the fallback so method additions never regress risk scoring.
    pub fn is_known(method: &str) -> bool {
        Self::ALL.contains(&method) || crate::acp::r#impl::request::is_acp_request(method)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants_are_known() {
        // The named constants must all resolve through the authoritative
        // dispatch table so risk scoring never treats them as novel.
        assert!(AcpMethodNames::is_known(AcpMethodNames::SESSION_NEW));
        assert!(AcpMethodNames::is_known(AcpMethodNames::TERMINAL_CREATE));
        assert!(AcpMethodNames::is_known(AcpMethodNames::INITIALIZE));
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
