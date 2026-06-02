//! GAP-B52: Self-Evolution Infrastructure
//!
//! Provides the core infrastructure for the go-on self-evolution system:
//! sandboxed code patching, evolution loop with trigger sources, evolution
//! history tracking with auto-rollback, and the self-evolution agent.
//!
//! Sub-modules:
//! - `sandbox`: Safe patch application, build, and test execution
//! - `evolution_loop`: Trigger sources and lifecycle orchestration
//! - `evolution_history`: Persistent NDJSON history with rollback support

pub mod evolution_history; // GAP-B52-05
pub mod evolution_loop; // GAP-B52-02
pub mod sandbox; // GAP-B52-01
pub mod self_improvement_report; // GAP-B53-58
