//! ACP method name constants.
//!
//! All standard ACP method names are defined here as associated constants.
//! Handlers live in `protocol_pack.rs` and dispatch entries in `request.rs`:
//!   - session/resume, session/close — session lifecycle management
//!   - session/request_permission — permission response handler
//!   - terminal/create, terminal/output, terminal/release, terminal/kill,
//!     terminal/wait_for_exit — full terminal process management
//!
//! BLUE56-GAP-A09: Moved from `schema/mod.rs` to `protocol/acp_methods.rs`
//! because method names are a protocol concern, not a schema concern.

/// ACP method name constants.
///
/// Provides compile-time checked method name strings for ACP protocol methods.
#[allow(dead_code)] // F-GAP-49 — reserved ACP methods feature
pub struct AcpMethodNames;

#[allow(dead_code)] // F-GAP-49 — reserved ACP methods feature
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
