pub mod access_mode;
pub mod acp_methods;
pub mod mcp_server;
pub mod negotiator;
pub mod session_sync;
// BLUE56-GAP-A03: multi_channel_transport.rs (v2) removed — it was dead code with zero callers.
// V1 `transport::MultiChannelTransport` has all essential features (QoS, TTL, priority).
// V2's unique serde_json::Value payload support can be re-added when a concrete need arises.
// F-GAP-99: Superseded by mcp/schema.rs — kept for existing callers (runtime.rs)
pub mod rpc_protocol;
// F-GAP-29 (v1): Original MultiChannelTransport implementation (always compiled). Used by
// capability_bus and fault_tolerance. Simpler design with TransportConfig/TransportMessage types.
// When `sub-bus-protocol` is enabled, the v2 `multi_channel_transport` module provides an
// alternative with richer features. The two struct names are intentionally identical because
// they live in separate modules, avoiding name collisions.
pub mod rate_limit;
pub mod transport;
pub mod grpc;
pub mod websocket;

// Re-exports
#[allow(unused_imports)]
pub use session_sync::*;
#[allow(unused_imports)]
pub use websocket::*;
