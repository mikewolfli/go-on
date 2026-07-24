//! Intelligence modules for adaptive model selection, evaluation, and quality management.
//!
//! This module contains components responsible for intelligent decision-making
//! in the ACP proxy system, including:
//!
//! - **Model Selection**: Adaptive algorithms for choosing the best AI model
//! - **World Model**: Structured environment representation and tracking (F-GAP-23)
//! - **Quality Evaluation**: Metrics and models for assessing response quality
//! - **Promotion Logic**: Rules for promoting agents based on performance
//! - **Reinforcement Learning**: Learning from feedback to improve decisions
//! - **Verification Systems**: Ensuring output correctness and safety
//! - **Consciousness Metrics**: BLUE38 F-GAP-25 Agency Consciousness Metrics (M10)

/// Shared monotonic timestamp in milliseconds (epoch-based for human readability).
///
/// Many intelligence sub-modules previously defined their own `now_ms()` or `now_ts()`
/// with identical bodies. Use this shared helper instead of duplicating.
pub fn now_ms() -> u64 {
    crate::shared::timestamps::now_ts_ms() as u64
}

/// Acquire a lock on a `Mutex`, recovering from a poisoned state with a warning.
///
/// Delegates to the shared `lock_or_recover!` macro from `crate::shared::lock_utils`.
pub fn lock_guard<T>(mtx: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    crate::lock_or_recover!(mtx, "intelligence")
}

/// Acquire a read lock on an `RwLock`, recovering from a poisoned state with a warning.
///
/// Delegates to the shared `read_or_recover!` macro from `crate::shared::lock_utils`.
pub fn read_guard<T>(rw: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    crate::read_or_recover!(rw, "intelligence")
}

/// Acquire a write lock on an `RwLock`, recovering from a poisoned state with a warning.
///
/// Delegates to the shared `write_or_recover!` macro from `crate::shared::lock_utils`.
pub fn write_guard<T>(rw: &std::sync::RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    crate::write_or_recover!(rw, "intelligence")
}

pub mod adaptive_selector;
pub mod causal_bayesian_graph;
pub mod consensus;
pub mod continuous_learning;
pub mod discovery;
pub mod hot_failover;
pub mod hub;
pub mod matcher;
pub mod metacognitive;
pub mod quality_models;
pub mod reputation;
pub mod semantic_matcher;

pub mod code_quality;
pub mod metacognitive_persistence;

pub mod capability_bus;
pub mod capability_graph;
pub mod consciousness;
pub mod evaluation;

pub mod model_selector;
pub mod reinforcement;
pub mod token_cache;
pub mod verification;

pub mod self_model;

pub mod evolution_graph;

pub mod fusion_evolution_bridge;
pub mod triple_fusion;
pub mod voter_impls;
pub mod weighted_vote;
pub mod world_model;

// Multi-model voter for high-stakes decision consensus.
// Gated behind sub-bus-voter-future feature for advanced deployment profiles.
#[cfg(feature = "sub-bus-voter-future")]
pub mod multi_model_voter;
