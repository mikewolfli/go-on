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

pub mod adaptive_selector;
pub mod causal_bayesian_graph;
pub mod continuous_learning;
pub mod discovery;
pub mod hot_failover;
pub mod hub;
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
