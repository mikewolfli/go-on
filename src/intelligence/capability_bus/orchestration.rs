//! Orchestration subsystem — council and agent factory helpers
//!
//! Extracted from `core.rs` to isolate OrchestrationCouncil and AgentFactory
//! integration within the sense/decide/evolve pipeline (F-GAP-13, F-GAP-15).
//!
//! Note: MultiChannelTransport (previously defined here) was removed as dead code.
//! - evolve_send_transport_event() replaced with tracing::info!() in core.rs evolve()
//! - ~740 lines of transport code + ~432 lines of tests eliminated
