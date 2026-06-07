//! CapabilityBus — Unified scheduling coordinator (BLUE38 ARCH-13)
//!
//! CapabilityBus is NOT a "big monolithic bus" — it is a **scheduling coordinator**
//! that mediates data flow between 13 independent sub-buses and drives the
//! reinforcement-learning feedback loop.
//! CapabilityBus — Unified scheduling coordinator (BLUE38 ARCH-13)
//!
//! # Architecture
//!
//! ```text
//!                    CapabilityBus (scheduling coordinator)
//!         ┌─────┬──────┬──────┬──────┬──────┬──────┬──────┐
//!         │     │      │      │      │      │      │      │
//!     ┌───┴┐ ┌─┴──┐ ┌┴───┐ ┌┴───┐ ┌┴───┐ ┌┴───┐ ┌┴─────┐
//!     │Work│ │Know│ │Dist│ │Repu│ │Capa│ │Rein│ │Harness│
//!     │flow│ │ledg│ │Memo│ │tati│ │bili│ │forc│ │Bus    │
//!     │Lear│ │e   │ │ry  │ │on  │ │ty  │ │emen│ │(strat│
//!     │ning│ │Bus │ │Bus │ │Stor│ │Grap│ │t   │ │egy)  │
//!     │Bus │ │    │ │    │ │e   │ │h   │ │Lear│ │      │
//!     └────┘ └────┘ └────┘ └────┘ └────┘ └────┘ └──────┘
//! ```
//!
//! # Lifecycle (Phase 0 implementation)
//!
//! 1. **Sensing**   — query sub-buses (capability graph, reputation, learning bus)
//! 2. **Decision**  — Q-Learning + HarnessBus policy → choose agent + strategy
//! 3. **Action**    — dispatch to agent; validate tool calls via HarnessBus
//! 4. **Feedback**  — write results back to sub-buses (learning, reputation)
//! 5. **Evolution** — update Q-table, decay exploration rate

pub mod consensus;
pub mod core;
pub mod discovery;
pub mod evolution;
pub mod learning;
pub mod metacognition;
pub mod orchestration;
#[cfg(feature = "sub-bus-distributed-memory")]
pub mod distributed_memory_bus;
#[cfg(feature = "sub-bus-memory")]
pub mod memory_bus;
#[cfg(feature = "sub-bus-observability")]
pub mod observability_bus;
#[cfg(feature = "sub-bus-optimization")]
pub mod optimization_bus;
#[cfg(feature = "sub-bus-orchestration")]
pub mod orchestration_bus;
#[cfg(feature = "sub-bus-protocol")]
pub mod protocol_bus;
#[cfg(feature = "sub-bus-tool")]
pub mod tool_bus;
