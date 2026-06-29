//! MethodRouter — registration-based dispatch (B51-28).
//!
//! Replaces the monolithic `match method.as_ref() { ... }` block in
//! `handle_request` with a pluggable registration-based dispatch so
//! that new methods can be added without touching the giant dispatch match.

use crate::rpc_protocol::RequestTraceContext;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;

/// A handler for a single JSON-RPC method.
#[async_trait::async_trait]
pub trait MethodHandler: Send + Sync {
    async fn handle(
        &self,
        server: &crate::acp::server::AcpServer,
        params: Value,
        request_id: Option<Value>,
        trace: &RequestTraceContext,
    ) -> Result<()>;
}

/// Registration-based method router.
pub struct MethodRouter {
    handlers: HashMap<&'static str, Box<dyn MethodHandler>>,
}

impl MethodRouter {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a method name.
    pub fn register(&mut self, method: &'static str, handler: Box<dyn MethodHandler>) {
        self.handlers.insert(method, handler);
    }

    /// Dispatch to a registered handler.
    pub async fn dispatch(
        &self,
        method: &str,
        server: &crate::acp::server::AcpServer,
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
