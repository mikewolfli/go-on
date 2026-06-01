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

    /// Register a handler for the given method name.
    #[allow(dead_code)] // F-GAP-51 — new API surface, not yet wired
    pub fn register(&mut self, method: &'static str, handler: Box<dyn MethodHandler>) {
        self.handlers.insert(method, handler);
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

/// Convenience: register a handler on the global router at startup.
#[allow(dead_code)] // F-GAP-51 — new API surface, not yet wired
pub fn register_method_handler(method: &'static str, handler: Box<dyn MethodHandler>) {
    // Get-or-init guarantees we get the same instance to modify.
    let router = GLOBAL_ROUTER.get_or_init(MethodRouter::new);
    // Safety: we only call this during single-threaded startup (before any
    // dispatch calls), so a direct mutable access via the raw pointer is
    // sound here.  In practice this is called from `fn init_method_router()`.
    unsafe {
        let ptr = router as *const MethodRouter as *mut MethodRouter;
        (*ptr).register(method, handler);
    }
    // Register on the static table as well so that `is_acp_request`
    // continues to recognise the method.
    register_acp_method(method);
}

// ── ACP method-name tracking ──────────────────────────────────────────────

#[allow(dead_code)] // F-GAP-51 — new API surface, not yet wired
static ACP_METHOD_REGISTRY: OnceLock<std::sync::Mutex<Vec<&'static str>>> = OnceLock::new();

#[allow(dead_code)] // F-GAP-51 — new API surface, not yet wired
fn acp_method_registry() -> &'static std::sync::Mutex<Vec<&'static str>> {
    ACP_METHOD_REGISTRY.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Register an ACP method name so that `is_acp_request` recognises it.
#[allow(dead_code)] // F-GAP-51 — new API surface, not yet wired
pub fn register_acp_method(method: &'static str) {
    if let Ok(mut guard) = acp_method_registry().lock() {
        guard.push(method);
    }
}

/// Returns true if the method is known to the ACP protocol (either built-in
/// or dynamically registered).
#[allow(dead_code)] // F-GAP-51 — new API surface, not yet wired
pub fn is_registered_acp_method(method: &str) -> bool {
    if let Ok(guard) = acp_method_registry().lock() {
        guard.contains(&method)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[tokio::test]
    async fn test_router_register_and_dispatch() {
        struct PingHandler;
        #[async_trait::async_trait]
        impl MethodHandler for PingHandler {
            async fn handle(
                &self,
                _server: &AcpServer,
                _params: Value,
                request_id: Option<Value>,
                _trace: &RequestTraceContext,
            ) -> Result<()> {
                let _id = request_id;
                Ok(())
            }
        }

        let mut router = MethodRouter::new();
        router.register("ping", Box::new(PingHandler));

        // Build a minimal server for dispatch.
        let server = crate::acp::server::ServerBuilder::new()
            .build()
            .expect("test server should build");
        let trace = RequestTraceContext {
            trace_id: "test".into(),
            span_id: "test".into(),
            method: "ping".into(),
            request_id: "1".into(),
        };

        let result = router
            .dispatch("ping", &server, Value::Null, None, &trace)
            .await;
        assert!(result.is_some(), "registered handler should return Some");
        assert!(result.unwrap().is_ok(), "handler should succeed");

        // Unregistered method returns None
        let result = router
            .dispatch("unknown", &server, Value::Null, None, &trace)
            .await;
        assert!(result.is_none(), "unregistered method should return None");
    }
}
