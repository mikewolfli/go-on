//! Sub-AI Factory — F-GAP-13 (FUTURE4.M4 / BLUE38 §6.8).
//!
//! The Sub-AI Factory is an agent factory pattern that creates, configures,
//! and manages sub-agent instances dynamically. It provides template-based
//! agent creation with configurable overrides, lifecycle management, and
//! runtime metrics.

pub mod agent_factory;

#[cfg(any(
    feature = "sub-bus-tool",
    feature = "simple-server",
    feature = "multi-users-server",
))]
pub use agent_factory::{AgentFactory, AgentFactoryConfig};
