//! Agent Communication Module — BLUE70
//!
//! Multi-Agent tree-based communication system for go-on.
//! Provides hierarchical agent addressing, structured message passing,
//! context inheritance, and execution control.
//!
//! Architecture:
//! - `AgentPath`      — hierarchical path addressing (root/research/coder)
//! - `AgentMessage`   — structured inter-agent message types
//! - `ForkContext`    — parent-to-child context snapshot
//! - `AgentTree`      — lightweight hierarchical agent index
//! - `AgentMessenger` — message routing and delivery
//! - `ExecutionGovernor` — budget-aware execution control
//! - `CommunicationBus` — top-level bus aggregating all components

pub mod path;
pub mod message;
pub mod context;
pub mod budget;
pub mod tree;
pub mod messenger;
pub mod bus;
pub mod forker;
pub mod governor;
pub mod agent_thread;
pub mod lifecycle;
