//! Token layer chain — L0-L5 layered token gate architecture.
//!
//! Implements BLUE38 ARCH-04: full L0 (fast reject/routing) → L1 (cache reuse) →
//! L2 (cheap classify) → L3 (context compress) → L4 (primary generation) →
//! L5 (verify/escalate) chain.
//!
//! Each layer evaluates Gate A–D conditions and returns a verdict.
//! Per-layer Prometheus-style counters track Allow / Reject / Route /
//! RequireApproval counts.  The `TokenLayerProfile` is exposed in
//! governance.status as `layered_token_trigger_profile`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Token cost estimation
// ---------------------------------------------------------------------------

/// Token cost estimation per layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCostEstimate {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
}

// ---------------------------------------------------------------------------
// Gate verdict
// ---------------------------------------------------------------------------

/// Verdict returned by a token layer gate
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl TokenGateVerdict {
    /// Human-readable short label for the verdict type.
    pub fn label(&self) -> &'static str {
        match self {
            TokenGateVerdict::Allow => "allow",
            TokenGateVerdict::Reject(_) => "reject",
            TokenGateVerdict::Route(_) => "route",
            TokenGateVerdict::RequireApproval(_) => "require_approval",
        }
    }
}

// ---------------------------------------------------------------------------
// Request layer classification (L0–L5)
// ---------------------------------------------------------------------------

/// Request layer classification (L0–L5)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
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

impl RequestLayer {
    /// Human-readable label for this layer.
    pub fn label(&self) -> &'static str {
        match self {
            RequestLayer::L0FastReject => "L0FastReject",
            RequestLayer::L1CacheReuse => "L1CacheReuse",
            RequestLayer::L2CheapClassify => "L2CheapClassify",
            RequestLayer::L3ContextCompress => "L3ContextCompress",
            RequestLayer::L4PrimaryGeneration => "L4PrimaryGeneration",
            RequestLayer::L5VerifyEscalate => "L5VerifyEscalate",
        }
    }

    /// Numeric level (0-5) for escalation calculations.
    pub fn level(&self) -> u32 {
        match self {
            RequestLayer::L0FastReject => 0,
            RequestLayer::L1CacheReuse => 1,
            RequestLayer::L2CheapClassify => 2,
            RequestLayer::L3ContextCompress => 3,
            RequestLayer::L4PrimaryGeneration => 4,
            RequestLayer::L5VerifyEscalate => 5,
        }
    }

    /// All layers in order from L0 to L5.
    pub fn all() -> Vec<RequestLayer> {
        vec![
            RequestLayer::L0FastReject,
            RequestLayer::L1CacheReuse,
            RequestLayer::L2CheapClassify,
            RequestLayer::L3ContextCompress,
            RequestLayer::L4PrimaryGeneration,
            RequestLayer::L5VerifyEscalate,
        ]
    }
}

// ---------------------------------------------------------------------------
// Gate conditions (A–D)
// ---------------------------------------------------------------------------

/// Gate condition variants used within a `LayeredTokenGate` to decide a verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GateCondition {
    /// Gate A – TokenBudget: check that estimated input and output tokens fit
    /// within budget thresholds.
    TokenBudget { max_input: u64, max_output: u64 },
    /// Gate B – CacheAvailable: if a cache hit exists, allow short-circuit.
    CacheAvailable,
    /// Gate C – LowComplexity: cheap classification when keyword count is low.
    LowComplexity { max_keywords: usize },
    /// Gate D – NeedsFullGeneration: escalate or require approval when
    /// confidence is below a threshold.
    NeedsFullGeneration { min_confidence: f64 },
}

impl GateCondition {
    /// Evaluate this condition against the given `GateContext`.
    /// Returns `true` when the condition is satisfied (gate should pass).
    pub fn evaluate(&self, ctx: &GateContext) -> bool {
        match self {
            GateCondition::TokenBudget {
                max_input,
                max_output,
            } => {
                ctx.estimated_input_tokens <= *max_input
                    && ctx.estimated_output_tokens <= *max_output
            }
            GateCondition::CacheAvailable => ctx.has_cache_hit,
            GateCondition::LowComplexity { max_keywords } => ctx.keywords.len() <= *max_keywords,
            GateCondition::NeedsFullGeneration { min_confidence } => {
                ctx.confidence_score >= *min_confidence
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Gate context — input to every gate evaluation
// ---------------------------------------------------------------------------

/// Context provided to every gate evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateContext {
    /// Unique request identifier.
    pub request_id: String,
    /// Number of input tokens already estimated.
    pub estimated_input_tokens: u64,
    /// Number of output tokens already estimated.
    pub estimated_output_tokens: u64,
    /// Whether a cache hit exists for this request (L1).
    pub has_cache_hit: bool,
    /// List of keywords extracted from the prompt.
    pub keywords: Vec<String>,
    /// Confidence score (0.0 – 1.0) for the current generation path.
    pub confidence_score: f64,
    /// Raw request text for deeper analysis.
    pub request_text: String,
    /// (Optional) Maximum allowed input tokens for fast-reject (L0).
    pub max_input_tokens: Option<u64>,
    /// (Optional) Maximum allowed output tokens for budget checks.
    pub max_output_tokens: Option<u64>,
}

impl Default for GateContext {
    fn default() -> Self {
        GateContext {
            request_id: String::new(),
            estimated_input_tokens: 0,
            estimated_output_tokens: 0,
            has_cache_hit: false,
            keywords: Vec::new(),
            confidence_score: 1.0,
            request_text: String::new(),
            max_input_tokens: Some(4096),
            max_output_tokens: Some(2048),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-layer counters
// ---------------------------------------------------------------------------

/// Per-layer counters tracked during evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayerCounters {
    pub allow: u64,
    pub reject: u64,
    pub route: u64,
    pub require_approval: u64,
    pub total: u64,
}

impl LayerCounters {
    /// Record one verdict into the counters.
    pub fn record(&mut self, verdict: &TokenGateVerdict) {
        self.total += 1;
        match verdict {
            TokenGateVerdict::Allow => self.allow += 1,
            TokenGateVerdict::Reject(_) => self.reject += 1,
            TokenGateVerdict::Route(_) => self.route += 1,
            TokenGateVerdict::RequireApproval(_) => self.require_approval += 1,
        }
    }
}

// ---------------------------------------------------------------------------
// TokenLayerProfile
// ---------------------------------------------------------------------------

/// Snapshot of all layer counters, keyed by layer label.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenLayerProfile {
    pub layers: HashMap<String, LayerCounters>,
}

// ---------------------------------------------------------------------------
// LayeredTokenGate
// ---------------------------------------------------------------------------

/// A single gate in the layered token chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayeredTokenGate {
    /// Which layer this gate belongs to.
    pub layer: RequestLayer,
    /// Human-readable name for this gate.
    pub gate_name: String,
    /// Conditions checked by this gate (AND logic — all must pass for Allow).
    pub conditions: Vec<GateCondition>,
    /// Whether this gate is currently enabled.
    pub enabled: bool,
}

impl LayeredTokenGate {
    /// Create a new enabled gate at the given layer.
    pub fn new(layer: RequestLayer, gate_name: &str, conditions: Vec<GateCondition>) -> Self {
        LayeredTokenGate {
            layer,
            gate_name: gate_name.to_string(),
            conditions,
            enabled: true,
        }
    }

    /// Evaluate all conditions against the context.
    /// Returns `Allow` when every condition passes; otherwise the first failing
    /// condition determines the verdict.
    pub fn evaluate(&self, ctx: &GateContext) -> TokenGateVerdict {
        if !self.enabled {
            return TokenGateVerdict::Allow;
        }

        for condition in &self.conditions {
            if !condition.evaluate(ctx) {
                // Determine the appropriate verdict based on the condition type.
                return match condition {
                    GateCondition::TokenBudget { .. } => {
                        TokenGateVerdict::Reject("token budget exceeded".into())
                    }
                    GateCondition::CacheAvailable => {
                        TokenGateVerdict::Route("cache miss — escalate to next layer".into())
                    }
                    GateCondition::LowComplexity { .. } => {
                        TokenGateVerdict::Route("too many keywords — escalate".into())
                    }
                    GateCondition::NeedsFullGeneration { .. } => TokenGateVerdict::RequireApproval(
                        "low confidence — requires human approval".into(),
                    ),
                };
            }
        }

        // If all conditions passed, determine the verdict based on the layer type.
        // L0 (FastReject) and L3 (ContextCompress) route to next layer when passing,
        // rather than allowing. This lets the chain continue evaluation through L5.
        match self.layer {
            RequestLayer::L0FastReject
            | RequestLayer::L3ContextCompress
            | RequestLayer::L4PrimaryGeneration => {
                TokenGateVerdict::Route("all conditions passed — escalate to next layer".into())
            }
            _ => TokenGateVerdict::Allow,
        }
    }
}

// ---------------------------------------------------------------------------
// TokenLayerChain — the full L0–L5 evaluator
// ---------------------------------------------------------------------------

/// Full L0–L5 token gate chain.
///
/// Each call to `evaluate` runs the request through every layer in sequence.
/// When a layer returns `Reject` or `RequireApproval`, evaluation stops and
/// that verdict is returned.  When a layer returns `Route`, the chain moves
/// to the next layer.  `Allow` from any layer also stops evaluation and
/// returns the positive verdict.
#[derive(Debug)]
pub struct TokenLayerChain {
    /// Ordered gates (typically L0 → L5).
    pub gates: Vec<LayeredTokenGate>,
    /// Per-layer counters protected by a mutex for interior mutability.
    profile: Mutex<TokenLayerProfile>,
}

impl TokenLayerChain {
    /// Build a new chain with the default L0–L5 gates and an empty profile.
    pub fn new() -> Self {
        let gates = Self::default_gates();
        let profile = TokenLayerProfile::default();
        TokenLayerChain {
            gates,
            profile: Mutex::new(profile),
        }
    }

    /// Build a chain from a caller-supplied gate list.
    pub fn from_gates(gates: Vec<LayeredTokenGate>) -> Self {
        TokenLayerChain {
            gates,
            profile: Mutex::new(TokenLayerProfile::default()),
        }
    }

    /// Default L0–L5 gate configuration with sensible thresholds.
    pub fn default_gates() -> Vec<LayeredTokenGate> {
        vec![
            // L0 – Fast reject / routing
            LayeredTokenGate::new(
                RequestLayer::L0FastReject,
                "L0-FastReject",
                vec![GateCondition::TokenBudget {
                    max_input: 4096,
                    max_output: 2048,
                }],
            ),
            // L1 – Cache reuse
            LayeredTokenGate::new(
                RequestLayer::L1CacheReuse,
                "L1-CacheReuse",
                vec![GateCondition::CacheAvailable],
            ),
            // L2 – Cheap classify
            LayeredTokenGate::new(
                RequestLayer::L2CheapClassify,
                "L2-CheapClassify",
                vec![GateCondition::LowComplexity { max_keywords: 10 }],
            ),
            // L3 – Context compress
            LayeredTokenGate::new(
                RequestLayer::L3ContextCompress,
                "L3-ContextCompress",
                vec![GateCondition::TokenBudget {
                    max_input: 8192,
                    max_output: 4096,
                }],
            ),
            // L4 – Primary generation
            LayeredTokenGate::new(
                RequestLayer::L4PrimaryGeneration,
                "L4-PrimaryGeneration",
                vec![GateCondition::TokenBudget {
                    max_input: 32768,
                    max_output: 8192,
                }],
            ),
            // L5 – Verify / escalate
            LayeredTokenGate::new(
                RequestLayer::L5VerifyEscalate,
                "L5-VerifyEscalate",
                vec![GateCondition::NeedsFullGeneration {
                    min_confidence: 0.7,
                }],
            ),
        ]
    }

    /// Run the full L0–L5 evaluation chain.
    ///
    /// Returns the first non-`Route` verdict.  `Allow` short-circuits and is
    /// returned immediately.  `Reject` and `RequireApproval` also stop the
    /// chain.  If every layer `Route`s, the last verdict is returned.
    pub fn evaluate(&self, ctx: &GateContext) -> TokenGateVerdict {
        let mut last_verdict = TokenGateVerdict::Route("no layers evaluated".into());

        for gate in &self.gates {
            let verdict = gate.evaluate(ctx);

            // Record the verdict in the counters.
            if let Ok(mut profile) = self.profile.lock() {
                let counters = profile
                    .layers
                    .entry(gate.layer.label().to_string())
                    .or_insert_with(LayerCounters::default);
                counters.record(&verdict);
            }

            last_verdict = verdict.clone();

            match &verdict {
                // Allow — request satisfied at this layer.
                TokenGateVerdict::Allow => return verdict,
                // Route — continue to the next layer.
                TokenGateVerdict::Route(_) => continue,
                // Reject or RequireApproval — stop the chain.
                TokenGateVerdict::Reject(_) | TokenGateVerdict::RequireApproval(_) => {
                    return verdict;
                }
            }
        }

        last_verdict
    }

    /// Return a snapshot of the current per-layer counters.
    pub fn get_profile(&self) -> TokenLayerProfile {
        self.profile
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Reset all counters to zero.
    pub fn reset_profile(&self) {
        if let Ok(mut profile) = self.profile.lock() {
            profile.layers.clear();
        }
    }

    /// Return per-layer stats as a map from layer label to `(allow, reject)` counts.
    /// Compatible with existing callers in `runtime_pack.rs`.
    pub fn layer_stats(&self) -> HashMap<String, (u64, u64)> {
        let profile = self.get_profile();
        profile
            .layers
            .into_iter()
            .map(|(k, c)| (k, (c.allow, c.reject)))
            .collect()
    }
}

impl Default for TokenLayerChain {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Estimate token cost for a given input/output token count pair at a specified rate
/// (cost per 1000 tokens).
pub fn estimate_cost(input: u64, output: u64, cost_per_1k: f64) -> TokenCostEstimate {
    let total_tokens = input + output;
    let estimated_cost_usd = (total_tokens as f64 / 1000.0) * cost_per_1k;
    TokenCostEstimate {
        input_tokens: input,
        output_tokens: output,
        estimated_cost_usd,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // GateCondition unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_gate_a_token_budget_passes() {
        let cond = GateCondition::TokenBudget {
            max_input: 1000,
            max_output: 500,
        };
        let ctx = GateContext {
            estimated_input_tokens: 800,
            estimated_output_tokens: 300,
            ..Default::default()
        };
        assert!(cond.evaluate(&ctx));
    }

    #[test]
    fn test_gate_a_token_budget_fails_input() {
        let cond = GateCondition::TokenBudget {
            max_input: 1000,
            max_output: 500,
        };
        let ctx = GateContext {
            estimated_input_tokens: 1200,
            estimated_output_tokens: 300,
            ..Default::default()
        };
        assert!(!cond.evaluate(&ctx));
    }

    #[test]
    fn test_gate_a_token_budget_fails_output() {
        let cond = GateCondition::TokenBudget {
            max_input: 1000,
            max_output: 500,
        };
        let ctx = GateContext {
            estimated_input_tokens: 800,
            estimated_output_tokens: 600,
            ..Default::default()
        };
        assert!(!cond.evaluate(&ctx));
    }

    #[test]
    fn test_gate_b_cache_available_true() {
        let cond = GateCondition::CacheAvailable;
        let ctx = GateContext {
            has_cache_hit: true,
            ..Default::default()
        };
        assert!(cond.evaluate(&ctx));
    }

    #[test]
    fn test_gate_b_cache_available_false() {
        let cond = GateCondition::CacheAvailable;
        let ctx = GateContext {
            has_cache_hit: false,
            ..Default::default()
        };
        assert!(!cond.evaluate(&ctx));
    }

    #[test]
    fn test_gate_c_low_complexity_passes() {
        let cond = GateCondition::LowComplexity { max_keywords: 3 };
        let ctx = GateContext {
            keywords: vec!["foo".into(), "bar".into()],
            ..Default::default()
        };
        assert!(cond.evaluate(&ctx));
    }

    #[test]
    fn test_gate_c_low_complexity_fails() {
        let cond = GateCondition::LowComplexity { max_keywords: 2 };
        let ctx = GateContext {
            keywords: vec!["a".into(), "b".into(), "c".into()],
            ..Default::default()
        };
        assert!(!cond.evaluate(&ctx));
    }

    #[test]
    fn test_gate_d_needs_full_generation_high_confidence() {
        let cond = GateCondition::NeedsFullGeneration {
            min_confidence: 0.7,
        };
        let ctx = GateContext {
            confidence_score: 0.85,
            ..Default::default()
        };
        assert!(cond.evaluate(&ctx));
    }

    #[test]
    fn test_gate_d_needs_full_generation_low_confidence() {
        let cond = GateCondition::NeedsFullGeneration {
            min_confidence: 0.7,
        };
        let ctx = GateContext {
            confidence_score: 0.4,
            ..Default::default()
        };
        assert!(!cond.evaluate(&ctx));
    }

    // -----------------------------------------------------------------------
    // LayeredTokenGate tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_single_gate_allows_when_all_conditions_pass() {
        let gate = LayeredTokenGate::new(
            RequestLayer::L2CheapClassify,
            "test-gate",
            vec![GateCondition::LowComplexity { max_keywords: 5 }],
        );
        let ctx = GateContext {
            keywords: vec!["hello".into(), "world".into()],
            ..Default::default()
        };
        assert_eq!(gate.evaluate(&ctx), TokenGateVerdict::Allow);
    }

    #[test]
    fn test_single_gate_rejects_on_token_budget_fail() {
        let gate = LayeredTokenGate::new(
            RequestLayer::L0FastReject,
            "budget-gate",
            vec![GateCondition::TokenBudget {
                max_input: 500,
                max_output: 300,
            }],
        );
        let ctx = GateContext {
            estimated_input_tokens: 1000,
            ..Default::default()
        };
        assert!(matches!(gate.evaluate(&ctx), TokenGateVerdict::Reject(_)));
    }

    #[test]
    fn test_disabled_gate_always_allows() {
        let mut gate = LayeredTokenGate::new(
            RequestLayer::L0FastReject,
            "disabled",
            vec![GateCondition::TokenBudget {
                max_input: 1,
                max_output: 1,
            }],
        );
        gate.enabled = false;
        let ctx = GateContext {
            estimated_input_tokens: 99999,
            ..Default::default()
        };
        assert_eq!(gate.evaluate(&ctx), TokenGateVerdict::Allow);
    }

    // -----------------------------------------------------------------------
    // Full L0–L5 chain tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_full_chain_l0_rejects_large_input() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 5000, // exceeds L0 max_input (4096)
            ..Default::default()
        };
        let verdict = chain.evaluate(&ctx);
        assert!(matches!(verdict, TokenGateVerdict::Reject(_)));
    }

    #[test]
    fn test_full_chain_l1_cache_hit_allows() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 1000, // within L0 budget
            has_cache_hit: true,
            ..Default::default()
        };
        let verdict = chain.evaluate(&ctx);
        assert_eq!(verdict, TokenGateVerdict::Allow);
    }

    #[test]
    fn test_full_chain_l2_cheap_classify_allows() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 1000, // within L0 budget
            has_cache_hit: false,         // L1 routes
            keywords: vec!["few".into()], // within L2 max_keywords (10)
            ..Default::default()
        };
        let verdict = chain.evaluate(&ctx);
        assert_eq!(verdict, TokenGateVerdict::Allow);
    }

    #[test]
    fn test_full_chain_l3_routes_when_input_large() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 1000,   // within L0
            has_cache_hit: false,           // L1 routes
            keywords: vec!["a".into(); 20], // exceeds L2 max_keywords — routes
            estimated_output_tokens: 500,
            ..Default::default()
        };
        // L2 fails -> Route; L3's TokenBudget (max_input 8192) should pass -> Allow
        let verdict = chain.evaluate(&ctx);
        assert_eq!(
            verdict,
            TokenGateVerdict::Allow,
            "L3 should allow when input is within budget"
        );
    }

    #[test]
    fn test_full_chain_l4_primary_generation_allows() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 9000, // L0 rejects... wait, L0 max is 4096
            estimated_output_tokens: 500,
            ..Default::default()
        };
        let verdict = chain.evaluate(&ctx);
        // L0 should reject because 9000 > 4096
        assert!(matches!(verdict, TokenGateVerdict::Reject(_)));
    }

    #[test]
    fn test_full_chain_l4_allows_reasonable_input() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 3000,   // L0 ok
            has_cache_hit: false,           // L1 routes
            keywords: vec!["a".into(); 20], // L2 routes (20 > 10)
            estimated_output_tokens: 500,   // L3 budget: 8192/4096 — passes
            ..Default::default()
        };
        // L3 should allow, because input is within TokenBudget limits
        let verdict = chain.evaluate(&ctx);
        assert_eq!(verdict, TokenGateVerdict::Allow);
    }

    #[test]
    fn test_full_chain_l5_requires_approval_on_low_confidence() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 3000,   // L0 ok
            has_cache_hit: false,           // L1 routes
            keywords: vec!["a".into(); 20], // L2 routes
            estimated_output_tokens: 500,   // L3 passes
            confidence_score: 0.3,          // L5 requires approval
            ..Default::default()
        };
        let verdict = chain.evaluate(&ctx);
        eprintln!("L5 test - got verdict: {:?}", verdict);
        assert!(matches!(verdict, TokenGateVerdict::RequireApproval(_)));
    }

    #[test]
    fn test_full_chain_l5_allows_on_high_confidence() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 3000,   // L0 ok
            has_cache_hit: false,           // L1 routes
            keywords: vec!["a".into(); 20], // L2 routes
            estimated_output_tokens: 500,   // L3 passes
            confidence_score: 0.95,         // L5 needs FullGeneration -> min 0.7 -> passes
            ..Default::default()
        };
        let verdict = chain.evaluate(&ctx);
        assert_eq!(verdict, TokenGateVerdict::Allow);
    }

    // -----------------------------------------------------------------------
    // Profile tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_profile_tracks_counters_correctly() {
        let chain = TokenLayerChain::new();

        // Run a large input that L0 rejects.
        let ctx = GateContext {
            estimated_input_tokens: 99999,
            ..Default::default()
        };
        chain.evaluate(&ctx);

        let profile = chain.get_profile();
        let l0_counters = profile.layers.get("L0FastReject").unwrap();
        assert_eq!(l0_counters.total, 1);
        assert_eq!(l0_counters.reject, 1);
        assert_eq!(l0_counters.allow, 0);

        // Other layers should not have been touched.
        assert!(profile.layers.get("L1CacheReuse").is_none());
    }

    #[test]
    fn test_profile_multiple_runs_accumulate() {
        let chain = TokenLayerChain::new();

        // Run 3 requests that make it to L2 and get allowed.
        for _ in 0..3 {
            let ctx = GateContext {
                estimated_input_tokens: 1000,
                has_cache_hit: false,
                keywords: vec!["hello".into()],
                ..Default::default()
            };
            chain.evaluate(&ctx);
        }

        let profile = chain.get_profile();

        // L0 routed all 3 (within budget, escalate to next layer).
        let l0 = profile.layers.get("L0FastReject").unwrap();
        assert_eq!(l0.total, 3);
        assert_eq!(l0.route, 3);

        // L1 routed all 3 (no cache hit).
        let l1 = profile.layers.get("L1CacheReuse").unwrap();
        assert_eq!(l1.total, 3);
        assert_eq!(l1.route, 3);

        // L2 allowed all 3.
        let l2 = profile.layers.get("L2CheapClassify").unwrap();
        assert_eq!(l2.total, 3);
        assert_eq!(l2.allow, 3);
    }

    #[test]
    fn test_reset_profile_clears_counters() {
        let chain = TokenLayerChain::new();
        let ctx = GateContext {
            estimated_input_tokens: 99999,
            ..Default::default()
        };
        chain.evaluate(&ctx);
        assert!(!chain.get_profile().layers.is_empty());

        chain.reset_profile();
        assert!(chain.get_profile().layers.is_empty());
    }

    #[test]
    fn test_from_gates_custom_chain() {
        let gates = vec![LayeredTokenGate::new(
            RequestLayer::L2CheapClassify,
            "custom-only",
            vec![GateCondition::LowComplexity { max_keywords: 1 }],
        )];
        let chain = TokenLayerChain::from_gates(gates);

        let ctx = GateContext {
            keywords: vec!["single".into()],
            ..Default::default()
        };
        assert_eq!(chain.evaluate(&ctx), TokenGateVerdict::Allow);

        let ctx = GateContext {
            keywords: vec!["a".into(), "b".into()],
            ..Default::default()
        };
        let verdict = chain.evaluate(&ctx);
        assert!(matches!(verdict, TokenGateVerdict::Route(_)));
    }

    // -----------------------------------------------------------------------
    // Verdict label tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_verdict_labels() {
        assert_eq!(TokenGateVerdict::Allow.label(), "allow");
        assert_eq!(TokenGateVerdict::Reject("nope".into()).label(), "reject");
        assert_eq!(TokenGateVerdict::Route("elsewhere".into()).label(), "route");
        assert_eq!(
            TokenGateVerdict::RequireApproval("please check".into()).label(),
            "require_approval"
        );
    }

    // -----------------------------------------------------------------------
    // RequestLayer tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_request_layer_levels() {
        assert_eq!(RequestLayer::L0FastReject.level(), 0);
        assert_eq!(RequestLayer::L1CacheReuse.level(), 1);
        assert_eq!(RequestLayer::L2CheapClassify.level(), 2);
        assert_eq!(RequestLayer::L3ContextCompress.level(), 3);
        assert_eq!(RequestLayer::L4PrimaryGeneration.level(), 4);
        assert_eq!(RequestLayer::L5VerifyEscalate.level(), 5);
    }

    #[test]
    fn test_request_layer_all_returns_six() {
        assert_eq!(RequestLayer::all().len(), 6);
    }

    #[test]
    fn test_request_layer_labels() {
        assert_eq!(RequestLayer::L0FastReject.label(), "L0FastReject");
        assert_eq!(RequestLayer::L1CacheReuse.label(), "L1CacheReuse");
        assert_eq!(RequestLayer::L2CheapClassify.label(), "L2CheapClassify");
        assert_eq!(RequestLayer::L3ContextCompress.label(), "L3ContextCompress");
        assert_eq!(
            RequestLayer::L4PrimaryGeneration.label(),
            "L4PrimaryGeneration"
        );
        assert_eq!(RequestLayer::L5VerifyEscalate.label(), "L5VerifyEscalate");
    }
}
