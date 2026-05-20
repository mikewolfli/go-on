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
    crate::acp::prelude::now_ts_ms() as u64
}

/// Acquire a lock on a `Mutex`, recovering from a poisoned state with a warning.
pub fn lock_guard<T>(mtx: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mtx.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("intelligence mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

pub mod adaptive_selector;
pub mod consensus;
pub mod discovery;
pub mod matcher;
pub mod metacognitive;

pub mod capability_bus;
pub mod capability_graph;
pub mod consciousness;
pub mod evaluation;

pub mod model_selector;
pub mod quality_models;
pub mod reinforcement;
pub mod reputation;
pub mod token_cache;
pub mod verification;

pub mod self_model;

pub mod continuous_learning;

pub mod evolution_graph;
pub mod federated_rl;

pub mod world_model;
