//! Orchestration Council — F-GAP-15 (FUTURE5.M1 / BLUE38 §6.6).
//!
//! Multi-agent council that coordinates decision-making among multiple agents
//! through a voting-based governance model. Council members can submit proposals,
//! cast votes, and the system tallies results to reach consensus.

#[allow(clippy::module_inception)]
pub mod council;
pub mod proposal;
pub mod quorum;
pub mod types;
pub mod voting;

#[allow(unused_imports)]
pub use council::*;
#[allow(unused_imports)]
pub use proposal::*;
#[allow(unused_imports)]
pub use quorum::*;
#[allow(unused_imports)]
pub use types::*;
#[allow(unused_imports)]
pub use voting::*;
