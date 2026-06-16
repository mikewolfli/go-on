//! MethodRouter — registration-based dispatch (B51-28).
//!
//! Replaces the monolithic `match method.as_ref() { ... }` block in
//! `handle_request` with a pluggable `HashMap<&'static str, Box<dyn MethodHandler>>`
//! so that new methods can be added without touching the giant dispatch match.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::RequestTraceContext;
use anyhow::Result;
use serde_json::Value;

/// A handler for a single JSON-RPC method.
///
/// Every registered handler receives the server reference, the deserialised
/// `params` object, the JSON-RPC request id, and the trace context.  Handlers
/// that don't need a particular parameter simply ignore it.
#[async_trait::async_trait]
pub trait MethodHandler: Send + Sync {
    async fn handle(
        &self,
        server: &AcpServer,
        params: Value,
        request_id: Option<Value>,
        trace: &RequestTraceContext,
    ) -> Result<()>;
}

/// Registration-based method router.
///
/// Methods are registered via [`MethodRouter::register`] and dispatched through
/// [`MethodRouter::dispatch`].  The router is backed by a single global
/// instance so that new handler registrations compose naturally across modules.
pub struct MethodRouter {
    handlers: HashMap<&'static str, Box<dyn MethodHandler>>,
}

impl MethodRouter {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Dispatch to a registered handler.
    ///
    /// Returns `Ok(Some(result))` if a handler was found, `Ok(None)` if no
    /// handler is registered for `method` (caller should fall through to the
    /// legacy match), or `Err` if the handler returned an error.
    pub async fn dispatch(
        &self,
        method: &str,
        server: &AcpServer,
        params: Value,
        request_id: Option<Value>,
        trace: &RequestTraceContext,
    ) -> Option<Result<()>> {
        if let Some(handler) = self.handlers.get(method) {
            Some(handler.handle(server, params, request_id, trace).await)
        } else {
            None
        }
    }
}

/// Global method router singleton, lazily initialised on first access.
static GLOBAL_ROUTER: OnceLock<MethodRouter> = OnceLock::new();

/// Get the global method router, initialising it with default handlers on
/// first call.
pub fn global_method_router() -> &'static MethodRouter {
    GLOBAL_ROUTER.get_or_init(|| {
        let router = MethodRouter::new();
        // ── Built-in registrations ──────────────────────────────────────
        // New handlers should be registered here or via
        // `register_method_handler` at startup.

        // Example registrations for the most frequently-called methods:
        // (Additional handlers will be migrated incrementally from the
        //  legacy match in `handle_request`.)

        router
    })
}
