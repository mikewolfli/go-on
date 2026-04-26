#![allow(dead_code)]

//! S7: Layered Token Trigger Gates
//!
//! Defines a chain of token-budget gate stages that are consulted before each
//! agent turn.  Each gate can block execution, trigger a summary, or pass through.
//! Gates are checked in-order; the first non-pass verdict wins.

use serde::{Deserialize, Serialize};

/// Verdict returned by a token gate
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenGateVerdict {
    Pass,
    /// Insert a summarization call before continuing
    Summarize,
    /// Hard block: budget exhausted
    Block,
}

/// A single gate stage with a trigger threshold (fraction of remaining budget)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenGateStage {
    pub name: String,
    /// Remaining budget fraction that triggers this stage (0.0 – 1.0)
    pub trigger_fraction: f32,
    pub verdict: TokenGateVerdict,
}

/// Configuration for the layered token trigger chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLayersConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Total token budget per turn (0 = disabled)
    #[serde(default = "default_budget")]
    pub total_budget: u32,
    /// Custom gate stages (if empty, built-in defaults are used)
    #[serde(default)]
    pub stages: Vec<TokenGateStage>,
}

fn default_enabled() -> bool {
    true
}
fn default_budget() -> u32 {
    16000
}

impl Default for TokenLayersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            total_budget: 16000,
            stages: Vec::new(),
        }
    }
}

fn default_stages() -> Vec<TokenGateStage> {
    vec![
        TokenGateStage {
            name: "warn".to_string(),
            trigger_fraction: 0.25,
            verdict: TokenGateVerdict::Summarize,
        },
        TokenGateStage {
            name: "hard_stop".to_string(),
            trigger_fraction: 0.10,
            verdict: TokenGateVerdict::Block,
        },
    ]
}

/// Token trigger chain evaluator
pub struct TokenLayerChain {
    pub config: TokenLayersConfig,
}

impl TokenLayerChain {
    pub fn new(config: TokenLayersConfig) -> Self {
        Self { config }
    }

    /// Evaluate the gate chain given tokens used so far.
    /// Returns the first non-pass verdict (or Pass if nothing triggered).
    pub fn evaluate(&self, tokens_used: u32) -> TokenGateVerdict {
        if !self.config.enabled || self.config.total_budget == 0 {
            return TokenGateVerdict::Pass;
        }
        let budget = self.config.total_budget as f32;
        let remaining = (self.config.total_budget.saturating_sub(tokens_used)) as f32;
        let remaining_fraction = remaining / budget;

        let stages = if self.config.stages.is_empty() {
            default_stages()
        } else {
            self.config.stages.clone()
        };

        for stage in &stages {
            if remaining_fraction <= stage.trigger_fraction {
                return stage.verdict.clone();
            }
        }
        TokenGateVerdict::Pass
    }
}
