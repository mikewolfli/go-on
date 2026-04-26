#![allow(dead_code)]

//! Workflow optimization extension interface.
//!
//! The runtime currently uses reinforcement-driven planning + execution-path
//! adaptation in request/chat handlers. This module keeps a future-facing
//! plugin contract so advanced optimizers can be integrated without reshaping
//! the main-chain API.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowOptimizationInput {
    pub task: String,
    pub complexity: u8,
    pub phase_count: usize,
    pub candidate_parallelism: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowOptimizationOutput {
    pub recommended_parallelism: usize,
    pub risk_score: f64,
    pub notes: Vec<String>,
}

pub trait WorkflowOptimizerPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn optimize(&self, input: &WorkflowOptimizationInput) -> WorkflowOptimizationOutput;
}

/// Built-in baseline strategy that preserves current behavior.
pub struct NoopWorkflowOptimizer;

impl WorkflowOptimizerPlugin for NoopWorkflowOptimizer {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn optimize(&self, input: &WorkflowOptimizationInput) -> WorkflowOptimizationOutput {
        WorkflowOptimizationOutput {
            recommended_parallelism: input.candidate_parallelism.max(1),
            risk_score: 0.0,
            notes: vec!["noop optimizer: preserve existing runtime strategy".to_string()],
        }
    }
}
