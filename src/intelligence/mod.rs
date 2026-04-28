//! Intelligence modules for adaptive model selection, evaluation, and quality management.
//!
//! This module contains components responsible for intelligent decision-making
//! in the ACP proxy system, including:
//!
//! - **Model Selection**: Adaptive algorithms for choosing the best AI model
//! - **Quality Evaluation**: Metrics and models for assessing response quality
//! - **Promotion Logic**: Rules for promoting agents based on performance
//! - **Reinforcement Learning**: Learning from feedback to improve decisions
//! - **Verification Systems**: Ensuring output correctness and safety

pub mod adaptive_selector;
// pub mod consensus;  // F-GAP-16 — pre-existing compilation errors, disabled
pub mod discovery;
pub mod matcher;

pub mod capability_bus;
pub mod capability_graph;
pub mod evaluation;
pub mod model_selector;
pub mod quality_models;
pub mod reinforcement;
pub mod reputation;
pub mod token_cache;
pub mod verification;
