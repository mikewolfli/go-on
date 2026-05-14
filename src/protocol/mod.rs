pub mod access_mode;
pub mod mcp_server;
// F-GAP-29 (v2): Enhanced MultiChannelTransport with richer channel separation, serde_json payloads,
// QosLevel, priority-based delivery, TTL expiry, and full statistics. This is the newer, better
// implementation — gated behind `sub-bus-protocol`. When this feature is enabled, this module
// takes precedence over `transport::MultiChannelTransport` and both names do NOT collide
// because they live in different modules. Callers should migrate to this version.
#[cfg(feature = "sub-bus-protocol")]
pub mod multi_channel_transport;
// F-GAP-99: Superseded by mcp/schema.rs — kept for existing callers (runtime.rs)
pub mod rpc_protocol;
// F-GAP-29 (v1): Original MultiChannelTransport implementation (always compiled). Used by
// capability_bus and fault_tolerance. Simpler design with TransportConfig/TransportMessage types.
// When `sub-bus-protocol` is enabled, multi_channel_transport provides a more feature-rich
// alternative. TODO: Consider disambiguating the two struct names to avoid confusion.
pub mod transport;
