pub mod access_mode;
pub mod mcp_server;
pub mod negotiator;
// F-GAP-29 (v2): MultiChannelTransport with richer channel separation, serde_json payloads,
// QosLevel, priority-based delivery, TTL expiry, and full statistics. This is the enhanced
// implementation — gated behind `sub-bus-protocol`. When this feature is enabled, both
// `transport::MultiChannelTransport` (v1) and `multi_channel_transport::MultiChannelTransport` (v2)
// are available but do NOT collide because they live in different modules. Callers should
// migrate to the v2 version.
#[cfg(feature = "sub-bus-protocol")]
pub mod multi_channel_transport;
// F-GAP-99: Superseded by mcp/schema.rs — kept for existing callers (runtime.rs)
pub mod rpc_protocol;
// F-GAP-29 (v1): Original MultiChannelTransport implementation (always compiled). Used by
// capability_bus and fault_tolerance. Simpler design with TransportConfig/TransportMessage types.
// When `sub-bus-protocol` is enabled, the v2 `multi_channel_transport` module provides an
// alternative with richer features. The two struct names are intentionally identical because
// they live in separate modules, avoiding name collisions.
pub mod transport;
pub mod rate_limit;
