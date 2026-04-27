//! Token layer chain — L0-L5 layered token gate architecture.
//!
//! Implements the Token Gate architecture (BLUE35 S7): L0 (fast reject) →
//! L1 (cache reuse) → L2 (cheap classify) → L3 (context compress) →
//! L4 (primary generation) → L5 (verify/escalate). Each layer applies
//! conditions that may Allow, Reject, Route, or RequireApproval.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Token cost estimation per layer
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct TokenCostEstimate {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
}

/// Verdict returned by a token layer gate
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum TokenGateVerdict {
    /// Allow the request to pass through this gate
    Allow,
    /// Reject the request at this gate with a reason
    Reject(String),
    /// Route to a higher/lower layer
    Route(String),
    /// Require human approval with a reason
    RequireApproval(String),
}

/// Request layer classification (L0–L5)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum RequestLayer {
    /// Fast reject / routing
    L0FastReject,
    /// Cache reuse check
    L1CacheReuse,
    /// Cheap classification
    L2CheapClassify,
    /// Context compression
    L3ContextCompress,
    /// Primary generation
    L4PrimaryGeneration,
    /// Verify / escalate
    L5VerifyEscalate,
}

/// A single gate in the token layer chain
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct TokenGate {
    pub name: String,
    pub layer: RequestLayer,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_cost_usd: f64,
    pub enabled: bool,
}

/// Gate condition evaluator — one of four conditions (A–D)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub enum GateCondition {
    /// Gate A: Token budget check
    TokenBudget { max_input: u64, max_output: u64 },
    /// Gate B: Cache availability
    CacheAvailable,
    /// Gate C: Complexity check
    LowComplexity { max_keywords: usize },
    /// Gate D: Escalation check
    NeedsFullGeneration { min_confidence: f64 },
}

/// Context passed to gate conditions for evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct GateContext {
    pub request_id: String,
    pub estimated_input_tokens: u64,
    pub estimated_output_tokens: u64,
    pub keywords: Vec<String>,
    pub has_cache_hit: bool,
    pub confidence_score: f64,
    pub request_text: String,
}

impl GateCondition {
    /// Evaluate this condition against the given context.
    #[allow(dead_code)]
    pub fn evaluate(&self, context: &GateContext) -> TokenGateVerdict {
        match self {
            GateCondition::TokenBudget {
                max_input,
                max_output,
            } => {
                if context.estimated_input_tokens > *max_input {
                    TokenGateVerdict::Reject(format!(
                        "Input tokens {} exceeds max {}",
                        context.estimated_input_tokens, max_input
                    ))
                } else if context.estimated_output_tokens > *max_output {
                    TokenGateVerdict::Route("L4".to_string())
                } else {
                    TokenGateVerdict::Allow
                }
            }
            GateCondition::CacheAvailable => {
                if context.has_cache_hit {
                    TokenGateVerdict::Route("L1".to_string())
                } else {
                    TokenGateVerdict::Allow
                }
            }
            GateCondition::LowComplexity { max_keywords } => {
                if context.keywords.len() <= *max_keywords {
                    TokenGateVerdict::Route("L2".to_string())
                } else {
                    TokenGateVerdict::Allow
                }
            }
            GateCondition::NeedsFullGeneration { min_confidence } => {
                if context.confidence_score >= *min_confidence {
                    TokenGateVerdict::Allow
                } else {
                    TokenGateVerdict::RequireApproval(format!(
                        "Confidence {:.2} below threshold {:.2}",
                        context.confidence_score, min_confidence
                    ))
                }
            }
        }
    }
}

/// Token layer chain — evaluates requests through L0–L5 gates sequentially.
///
/// Each layer has one or more conditions. If a condition returns Allow, the
/// next condition in the layer is tried. If all conditions in a layer pass,
/// the request moves to the next layer. Any Reject, Route, or
/// RequireApproval verdict short-circuits and is returned immediately.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TokenLayerChain {
    /// Ordered list of (layer, conditions) pairs.
    gates: Vec<(RequestLayer, Vec<GateCondition>)>,
    /// Per-layer counters: (allow_count, reject_count).
    counters: HashMap<RequestLayer, (u64, u64)>,
}

impl TokenLayerChain {
    /// Create a new chain with default thresholds.
    ///
    /// L0 – Fast reject: token budget (8K in / 4K out)
    /// L1 – Cache reuse: cache available?
    /// L2 – Cheap classify: ≤5 keywords?
    /// L3 – Context compress: token budget (32K in / 16K out)
    /// L4 – Primary generation: token budget (128K in / 64K out)
    /// L5 – Verify / escalate: confidence ≥ 0.8
    #[allow(dead_code)]
    pub fn new() -> Self {
        let layers = vec![
            RequestLayer::L0FastReject,
            RequestLayer::L1CacheReuse,
            RequestLayer::L2CheapClassify,
            RequestLayer::L3ContextCompress,
            RequestLayer::L4PrimaryGeneration,
            RequestLayer::L5VerifyEscalate,
        ];
        let mut counters = HashMap::new();
        for l in &layers {
            counters.insert(l.clone(), (0, 0));
        }
        Self {
            gates: vec![
                (
                    RequestLayer::L0FastReject,
                    vec![GateCondition::TokenBudget {
                        max_input: 8_000,
                        max_output: 4_000,
                    }],
                ),
                (
                    RequestLayer::L1CacheReuse,
                    vec![GateCondition::CacheAvailable],
                ),
                (
                    RequestLayer::L2CheapClassify,
                    vec![GateCondition::LowComplexity { max_keywords: 5 }],
                ),
                (
                    RequestLayer::L3ContextCompress,
                    vec![GateCondition::TokenBudget {
                        max_input: 32_000,
                        max_output: 16_000,
                    }],
                ),
                (
                    RequestLayer::L4PrimaryGeneration,
                    vec![GateCondition::TokenBudget {
                        max_input: 128_000,
                        max_output: 64_000,
                    }],
                ),
                (
                    RequestLayer::L5VerifyEscalate,
                    vec![GateCondition::NeedsFullGeneration {
                        min_confidence: 0.8,
                    }],
                ),
            ],
            counters,
        }
    }

    /// Evaluate a request through all layers, returning the final verdict.
    #[allow(dead_code)]
    pub fn evaluate(&mut self, context: &GateContext) -> TokenGateVerdict {
        for (layer, conditions) in &self.gates {
            let mut layer_verdict = TokenGateVerdict::Allow;

            for condition in conditions {
                match condition.evaluate(context) {
                    TokenGateVerdict::Allow => continue,
                    other => {
                        layer_verdict = other;
                        break;
                    }
                }
            }

            match &layer_verdict {
                TokenGateVerdict::Allow => {
                    if let Some((ref mut allow, _)) = self.counters.get_mut(layer) {
                        *allow += 1;
                    }
                }
                TokenGateVerdict::Reject(_) | TokenGateVerdict::RequireApproval(_) => {
                    if let Some((_, ref mut reject)) = self.counters.get_mut(layer) {
                        *reject += 1;
                    }
                    return layer_verdict;
                }
                TokenGateVerdict::Route(target) => {
                    if let Some((_, ref mut reject)) = self.counters.get_mut(layer) {
                        *reject += 1;
                    }
                    return TokenGateVerdict::Route(format!("routed_to_{}", target));
                }
            }
        }

        TokenGateVerdict::Allow
    }

    /// Get per-layer counter snapshot (allow_count, reject_count) keyed by layer name.
    #[allow(dead_code)]
    pub fn layer_stats(&self) -> HashMap<String, (u64, u64)> {
        self.counters
            .iter()
            .map(|(k, v)| (format!("{:?}", k), *v))
            .collect()
    }

    /// Reset all counters to zero.
    #[allow(dead_code)]
    pub fn reset_counters(&mut self) {
        for val in self.counters.values_mut() {
            *val = (0, 0);
        }
    }
}

impl Default for TokenLayerChain {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_context() -> GateContext {
        GateContext {
            request_id: "test-001".to_string(),
            estimated_input_tokens: 500,
            estimated_output_tokens: 200,
            keywords: vec!["fix".to_string(), "bug".to_string()],
            has_cache_hit: false,
            confidence_score: 0.95,
            request_text: "Fix the login bug".to_string(),
        }
    }

    #[test]
    fn test_l0_rejects_over_budget() {
        let mut chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 10_000,
            estimated_output_tokens: 500,
            ..sample_context()
        };
        let verdict = chain.evaluate(&ctx);
        assert!(matches!(verdict, TokenGateVerdict::Reject(ref s) if s.contains("exceeds")));
    }

    #[test]
    fn test_l1_routes_to_cache() {
        let mut chain = TokenLayerChain::new();
        let ctx = GateContext {
            has_cache_hit: true,
            ..sample_context()
        };
        let verdict = chain.evaluate(&ctx);
        // Should route to L1 (cache hit)
        assert!(matches!(verdict, TokenGateVerdict::Route(ref s) if s.contains("L1")));
    }

    #[test]
    fn test_l2_routes_simple_request() {
        let mut chain = TokenLayerChain::new();
        let ctx = GateContext {
            keywords: vec!["hello".to_string()],
            ..sample_context()
        };
        let verdict = chain.evaluate(&ctx);
        // ≤5 keywords → route to L2
        assert!(matches!(verdict, TokenGateVerdict::Route(ref s) if s.contains("L2")));
    }

    #[test]
    fn test_l4_allows_normal_request() {
        let mut chain = TokenLayerChain::new();
        // Normal request: small tokens, no cache hit, >5 keywords
        let ctx = GateContext {
            keywords: vec![
                "implement".to_string(),
                "new".to_string(),
                "feature".to_string(),
                "user".to_string(),
                "authentication".to_string(),
                "oauth".to_string(),
            ],
            ..sample_context()
        };
        let verdict = chain.evaluate(&ctx);
        assert!(matches!(verdict, TokenGateVerdict::Allow));
    }

    #[test]
    fn test_l5_requires_approval() {
        let mut chain = TokenLayerChain::new();
        let ctx = GateContext {
            keywords: vec![
                "implement".to_string(),
                "new".to_string(),
                "feature".to_string(),
                "user".to_string(),
                "authentication".to_string(),
                "oauth".to_string(),
            ],
            confidence_score: 0.5,
            ..sample_context()
        };
        let verdict = chain.evaluate(&ctx);
        assert!(matches!(verdict, TokenGateVerdict::RequireApproval(ref s) if s.contains("0.50")));
    }

    #[test]
    fn test_layer_stats_after_evaluation() {
        let mut chain = TokenLayerChain::new();
        let ctx = sample_context();
        let _ = chain.evaluate(&ctx);
        let stats = chain.layer_stats();
        // At least L0 should have an allow
        assert!(stats.contains_key("L0FastReject"));
        let (allow, _) = stats.get("L0FastReject").unwrap();
        assert!(*allow > 0);
    }

    #[test]
    fn test_reset_counters() {
        let mut chain = TokenLayerChain::new();
        let ctx = sample_context();
        let _ = chain.evaluate(&ctx);
        chain.reset_counters();
        let stats = chain.layer_stats();
        for (_, (allow, reject)) in &stats {
            assert_eq!(*allow, 0);
            assert_eq!(*reject, 0);
        }
    }

    #[test]
    fn test_chain_falls_through_to_allow() {
        let mut chain = TokenLayerChain::new();
        // Request that passes all gates: >5 keywords to skip L2 route,
        // small tokens for budget gates, high confidence for L5
        let ctx = GateContext {
            estimated_input_tokens: 100,
            estimated_output_tokens: 50,
            has_cache_hit: false,
            keywords: vec![
                "refactor".to_string(),
                "database".to_string(),
                "migration".to_string(),
                "schema".to_string(),
                "version".to_string(),
                "rollback".to_string(),
                "index".to_string(),
            ],
            confidence_score: 0.95,
            ..sample_context()
        };
        let verdict = chain.evaluate(&ctx);
        assert!(matches!(verdict, TokenGateVerdict::Allow));
    }

    #[test]
    fn test_gate_condition_token_budget_allow() {
        let condition = GateCondition::TokenBudget {
            max_input: 1000,
            max_output: 500,
        };
        let ctx = GateContext {
            estimated_input_tokens: 500,
            estimated_output_tokens: 200,
            ..sample_context()
        };
        assert!(matches!(condition.evaluate(&ctx), TokenGateVerdict::Allow));
    }

    #[test]
    fn test_gate_condition_cache_available() {
        let condition = GateCondition::CacheAvailable;
        let ctx = GateContext {
            has_cache_hit: true,
            ..sample_context()
        };
        assert!(matches!(
            condition.evaluate(&ctx),
            TokenGateVerdict::Route(_)
        ));
    }

    #[test]
    fn test_gate_condition_needs_full_generation() {
        let condition = GateCondition::NeedsFullGeneration {
            min_confidence: 0.9,
        };
        let ctx = GateContext {
            confidence_score: 0.7,
            ..sample_context()
        };
        assert!(matches!(
            condition.evaluate(&ctx),
            TokenGateVerdict::RequireApproval(_)
        ));
    }
}
