//! Rust SDK for go-on — Phase 4, 14-bus architecture, 21 F-GAP modules.
//!
//! Provides typed async clients for ACP JSON-RPC endpoints:
//!   - Runtime: health, initialize, shutdown
//!   - Governance: status, plan, audit
//!   - Observability: metrics, trace, health probes
//!   - Reliability: breaker, checkpoint, maintenance
//!   - Workflow / Task: execute, plan
//!   - Learning / Intelligence: summary, selector, knowledge, rl
//!   - Optimization / Operations: cost, config baseline, harness
//!   - Streaming chat: chat_stream
//!
//! All methods send JSON-RPC 2.0 requests to `POST {base_url}/rpc`.

pub mod client;
pub mod error;
pub mod types;

pub use client::{GoOnClient, GoOnClientBuilder};
pub use error::SdkError;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_types() {
        let timeout_err = SdkError::Timeout { elapsed_secs: 30 };
        assert!(timeout_err.to_string().contains("30"));

        let rate_err = SdkError::RateLimited {
            retry_after_secs: 5,
        };
        assert!(rate_err.to_string().contains("5"));

        let rpc_err = SdkError::JsonRpc {
            code: -32601,
            message: "method not found".into(),
        };
        assert!(rpc_err.to_string().contains("-32601"));
    }
}
