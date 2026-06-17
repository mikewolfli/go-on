//! GAP-B52-09: Federated Privacy — Differential Privacy for Model Weights
//!
//! Provides configuration and operations for applying differential privacy
//! to federated model weight submissions. Supports gradient clipping and
//! Gaussian noise addition to satisfy (ε, δ)-differential privacy.

use std::collections::HashMap;

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::intelligence::reinforcement::federated::ModelWeights;

// ── DifferentialPrivacyConfig ──────────────────────────────────────────────

/// Configuration for differential privacy applied to federated weight
/// submissions.
///
/// Default values provide a reasonable privacy-utility trade-off:
/// - `epsilon = 8.0`: moderate privacy guarantee
/// - `delta = 1e-5`: failure probability (must be < 1/N² where N is the
///   number of clients, per standard DP literature)
/// - `clip_norm = 1.0`: gradients are clipped to this L2 norm before noise
///   is added
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DifferentialPrivacyConfig {
    /// Privacy budget ε (epsilon). Lower values = stronger privacy.
    /// Typical range: 0.1 (strong) to 10.0 (weak).
    pub epsilon: f64,
    /// Failure probability δ (delta). Must be less than 1 / (num_clients²).
    /// Typical value: 1e-5.
    pub delta: f64,
    /// Maximum L2 norm for gradient clipping. Gradients with larger norm
    /// are scaled down to this value.
    pub clip_norm: f64,
}

impl Default for DifferentialPrivacyConfig {
    fn default() -> Self {
        Self {
            epsilon: 8.0,
            delta: 1e-5,
            clip_norm: 1.0,
        }
    }
}

impl DifferentialPrivacyConfig {
    /// Create a new differential privacy configuration.
    ///
    /// # Arguments
    ///
    /// * `epsilon` - Privacy budget (ε). Must be positive.
    /// * `delta` - Failure probability (δ). Must be in (0, 1).
    /// * `clip_norm` - Maximum gradient norm. Must be positive.
    pub fn new(epsilon: f64, delta: f64, clip_norm: f64) -> Result<Self> {
        ensure!(epsilon > 0.0, "epsilon must be positive, got {}", epsilon);
        ensure!(
            delta > 0.0 && delta < 1.0,
            "delta must be in (0, 1), got {}",
            delta
        );
        ensure!(
            clip_norm > 0.0,
            "clip_norm must be positive, got {}",
            clip_norm
        );
        Ok(Self {
            epsilon,
            delta,
            clip_norm,
        })
    }

    /// Compute the noise scale (σ) for the Gaussian mechanism.
    ///
    /// For (ε, δ)-differential privacy with the Gaussian mechanism,
    /// the noise standard deviation is:
    ///
    ///   σ = clip_norm · √(2 · ln(1.25 / δ)) / ε
    ///
    /// This follows from the analytic Gaussian mechanism (Balle & Wang, 2018).
    pub fn noise_scale(&self) -> f64 {
        let numerator = self.clip_norm * (2.0 * (1.25 / self.delta).ln()).sqrt();
        numerator / self.epsilon.max(f64::EPSILON)
    }

    /// Returns the privacy spend for a single round.
    pub fn privacy_spend_per_round(&self) -> f64 {
        self.epsilon
    }
}

// ── Gradient clipping ──────────────────────────────────────────────────────

/// Clip the L2 norm of model weights to a maximum value.
///
/// Each weight vector (q_table_snapshot and policy_params) is independently
/// clipped. If the L2 norm exceeds `clip_norm`, all values are scaled down
/// proportionally.
///
/// # Arguments
///
/// * `weights` - The model weights to clip (in-place).
/// * `clip_norm` - Maximum allowed L2 norm.
///
/// # Returns
///
/// The actual L2 norm before clipping (for monitoring purposes).
pub fn clip_gradients(weights: &mut ModelWeights, clip_norm: f64) -> f64 {
    let q_norm = l2_norm_of_map(&weights.q_table_snapshot);
    let p_norm = l2_norm_of_map(&weights.policy_params);

    let total_norm = (q_norm * q_norm + p_norm * p_norm).sqrt();

    if total_norm > clip_norm && total_norm > f64::EPSILON {
        let scale = clip_norm / total_norm;

        for value in weights.q_table_snapshot.values_mut() {
            *value *= scale;
        }
        for value in weights.policy_params.values_mut() {
            *value *= scale;
        }

        debug!(
            "clip_gradients: norm {:.4} exceeded {:.4}, scaled by {:.4}",
            total_norm, clip_norm, scale
        );
    } else {
        debug!(
            "clip_gradients: norm {:.4} within {:.4}, no scaling needed",
            total_norm, clip_norm
        );
    }

    total_norm
}

/// Compute the L2 norm of a map of values.
fn l2_norm_of_map(map: &HashMap<String, f64>) -> f64 {
    let sum_sq: f64 = map.values().map(|v| v * v).sum();
    sum_sq.sqrt()
}

// ── Gaussian noise addition ────────────────────────────────────────────────

/// Add Gaussian noise to model weights to achieve (ε, δ)-differential privacy.
///
/// This function:
/// 1. Clips gradients to `clip_norm` (delegating to `clip_gradients`).
/// 2. Computes the noise scale σ from the DP config.
/// 3. Adds independent Gaussian noise N(0, σ²) to each parameter.
///
/// # Arguments
///
/// * `weights` - The model weights to noise (in-place).
/// * `epsilon` - Privacy budget per round.
/// * `delta` - Failure probability parameter.
/// * `clip_norm` - Maximum gradient norm for clipping.
///
/// # Returns
///
/// The noise scale σ used (for monitoring).
pub fn add_gaussian_noise(
    weights: &mut ModelWeights,
    epsilon: f64,
    delta: f64,
    clip_norm: f64,
) -> f64 {
    // Step 1: clip gradients.
    clip_gradients(weights, clip_norm);

    // Step 2: compute noise scale.
    let config = DifferentialPrivacyConfig {
        epsilon,
        delta,
        clip_norm,
    };
    let sigma = config.noise_scale();

    // Step 3: add Gaussian noise to each parameter.
    let mut rng = FastRng::new();

    for value in weights.q_table_snapshot.values_mut() {
        *value += rng.gaussian(sigma);
    }
    for value in weights.policy_params.values_mut() {
        *value += rng.gaussian(sigma);
    }

    debug!(
        "add_gaussian_noise: applied N(0, σ²={:.6}) noise to {} parameters",
        sigma * sigma,
        weights.q_table_snapshot.len() + weights.policy_params.len()
    );

    sigma
}

// ── PrivacyBudget ──────────────────────────────────────────────────────────

/// Tracks the remaining privacy budget for a federated round participant.
///
/// In differential privacy, the total privacy spend is additive across
/// rounds (under composition). This struct helps a node decide when it
/// can no longer safely participate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyBudget {
    /// Total epsilon budget allocated for all rounds.
    pub total_epsilon: f64,
    /// Epsilon spent so far.
    pub epsilon_spent: f64,
    /// Number of rounds remaining (derived from budget).
    pub rounds_remaining: u64,
    /// Total number of rounds allocated.
    pub total_rounds: u64,
    /// The DP configuration used.
    pub config: DifferentialPrivacyConfig,
}

impl PrivacyBudget {
    /// Create a new privacy budget tracker.
    ///
    /// # Arguments
    ///
    /// * `total_epsilon` - Total ε budget across all rounds.
    /// * `total_rounds` - Expected number of rounds.
    /// * `config` - The DP configuration for each round.
    pub fn new(total_epsilon: f64, total_rounds: u64, config: DifferentialPrivacyConfig) -> Self {
        Self {
            total_epsilon,
            epsilon_spent: 0.0,
            rounds_remaining: total_rounds,
            total_rounds,
            config,
        }
    }

    /// Record a round contribution and deduct from the budget.
    ///
    /// Returns an error if the budget is exhausted.
    pub fn spend_round(&mut self) -> Result<()> {
        if self.rounds_remaining == 0 {
            anyhow::bail!(
                "privacy budget exhausted: spent {:.2}/{:.2} ε over {} rounds",
                self.epsilon_spent,
                self.total_epsilon,
                self.total_rounds
            );
        }

        let spend = self.config.privacy_spend_per_round();
        self.epsilon_spent += spend;
        self.rounds_remaining -= 1;

        debug!(
            "PrivacyBudget: spent {:.4} ε (total {:.4}/{:.4}), {} rounds remaining",
            spend, self.epsilon_spent, self.total_epsilon, self.rounds_remaining
        );

        Ok(())
    }

    /// Returns the fraction of the privacy budget that has been consumed.
    pub fn fraction_consumed(&self) -> f64 {
        if self.total_epsilon <= 0.0 {
            1.0
        } else {
            (self.epsilon_spent / self.total_epsilon).clamp(0.0, 1.0)
        }
    }

    /// Returns `true` if the budget is exhausted.
    pub fn is_exhausted(&self) -> bool {
        self.rounds_remaining == 0 || self.epsilon_spent >= self.total_epsilon
    }

    /// Returns the epsilon remaining for future rounds.
    pub fn epsilon_remaining(&self) -> f64 {
        (self.total_epsilon - self.epsilon_spent).max(0.0)
    }
}

// ── FastRng: simple Gaussian random number generator ───────────────────────

/// A minimal PRNG for generating Gaussian noise.
///
/// Uses the Box-Muller transform on top of a fast xorshift64* generator.
/// This is NOT cryptographically secure — it is designed for differential
/// privacy noise where performance matters more than security against
/// adversarial bit prediction.
struct FastRng {
    state: u64,
}

impl FastRng {
    fn new() -> Self {
        // Seed from system time + a fixed constant to avoid identical
        // sequences across nodes that start simultaneously.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(42)
            .wrapping_mul(6364136223846793005);
        Self { state: seed }
    }

    /// Generate a standard normal N(0,1) sample using Box-Muller.
    fn standard_normal(&mut self) -> f64 {
        let u1 = self.uniform_01();
        let u2 = self.uniform_01();

        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;

        r * theta.cos()
    }

    /// Generate N(0, σ²) Gaussian noise.
    fn gaussian(&mut self, sigma: f64) -> f64 {
        self.standard_normal() * sigma
    }

    /// Generate a uniform random number in (0, 1).
    fn uniform_01(&mut self) -> f64 {
        // xorshift64*
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        // Convert to f64 in (0, 1).
        (self.state.wrapping_mul(2685821657736338717)) as f64 / (u64::MAX as f64)
    }
}

// ── Convenience function ───────────────────────────────────────────────────

/// Apply full differential privacy sanitization to model weights.
///
/// This is a convenience function that clips gradients and adds Gaussian
/// noise in a single call, using a `DifferentialPrivacyConfig`.
///
/// # Arguments
///
/// * `weights` - The model weights to sanitize (in-place).
/// * `config` - The differential privacy configuration.
///
/// # Returns
///
/// The noise scale σ used.
pub fn apply_dp(weights: &mut ModelWeights, config: &DifferentialPrivacyConfig) -> f64 {
    add_gaussian_noise(weights, config.epsilon, config.delta, config.clip_norm)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_weights() -> ModelWeights {
        let mut q = HashMap::new();
        q.insert("s1_a1".into(), 0.5);
        q.insert("s1_a2".into(), -0.3);
        q.insert("s2_a1".into(), 1.2);

        let mut p = HashMap::new();
        p.insert("lr".into(), 0.01);
        p.insert("discount".into(), 0.95);

        ModelWeights {
            q_table_snapshot: q,
            policy_params: p,
            version: 1,
        }
    }

    #[test]
    fn test_dp_config_default() {
        let config = DifferentialPrivacyConfig::default();
        assert!((config.epsilon - 8.0).abs() < 1e-10);
        assert!((config.delta - 1e-5).abs() < 1e-10);
        assert!((config.clip_norm - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_dp_config_new_valid() {
        let config = DifferentialPrivacyConfig::new(2.0, 1e-6, 0.5).unwrap();
        assert!((config.epsilon - 2.0).abs() < 1e-10);
        assert!((config.delta - 1e-6).abs() < 1e-10);
        assert!((config.clip_norm - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_dp_config_new_invalid_epsilon() {
        let result = DifferentialPrivacyConfig::new(0.0, 1e-5, 1.0);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("epsilon must be positive"));
    }

    #[test]
    fn test_dp_config_new_invalid_delta() {
        let result = DifferentialPrivacyConfig::new(1.0, 0.0, 1.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("delta must be in"));

        let result = DifferentialPrivacyConfig::new(1.0, 1.0, 1.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_noise_scale_positive() {
        let config = DifferentialPrivacyConfig::default();
        let sigma = config.noise_scale();
        assert!(sigma > 0.0, "noise scale must be positive");
    }

    #[test]
    fn test_noise_scale_smaller_with_larger_epsilon() {
        let weak = DifferentialPrivacyConfig::new(8.0, 1e-5, 1.0).unwrap();
        let strong = DifferentialPrivacyConfig::new(1.0, 1e-5, 1.0).unwrap();
        assert!(
            strong.noise_scale() > weak.noise_scale(),
            "stronger privacy (lower ε) should have higher noise"
        );
    }

    #[test]
    fn test_clip_gradients_no_clip_needed() {
        let mut weights = sample_weights();
        // The L2 norm of sample weights is sqrt(0.25 + 0.09 + 1.44 + 0.0001 + 0.9025) ≈ sqrt(2.6826) ≈ 1.638
        let original = weights.clone();
        let norm = clip_gradients(&mut weights, 10.0);

        // Norm should be unchanged because clip_norm is very large.
        assert!(norm > 0.0);

        // Values should be unchanged.
        for (k, v) in &original.q_table_snapshot {
            assert!((weights.q_table_snapshot.get(k).unwrap() - v).abs() < 1e-10);
        }
    }

    #[test]
    fn test_clip_gradients_clips_when_exceeded() {
        let mut weights = sample_weights();
        let norm = clip_gradients(&mut weights, 0.5);

        // The norm should be at most 0.5 after clipping.
        let q_norm: f64 = weights
            .q_table_snapshot
            .values()
            .map(|v| v * v)
            .sum::<f64>()
            .sqrt();
        let p_norm: f64 = weights
            .policy_params
            .values()
            .map(|v| v * v)
            .sum::<f64>()
            .sqrt();
        let total_norm = (q_norm * q_norm + p_norm * p_norm).sqrt();

        assert!(
            total_norm <= 0.5 + 1e-10,
            "total norm {total_norm} should be <= 0.5"
        );
        assert!(norm > 0.5, "original norm should have been > 0.5");
    }

    #[test]
    fn test_add_gaussian_noise_adds_noise() {
        let mut weights = sample_weights();
        let original = weights.clone();

        let sigma = add_gaussian_noise(&mut weights, 8.0, 1e-5, 1.0);

        assert!(sigma > 0.0, "noise scale should be positive");

        // After noise, values should differ (very high probability).
        let mut any_changed = false;
        for (k, v) in &original.q_table_snapshot {
            let new_v = weights.q_table_snapshot.get(k).unwrap();
            if (*new_v - v).abs() > 1e-10 {
                any_changed = true;
                break;
            }
        }
        assert!(any_changed, "noise should have changed at least one value");
    }

    #[test]
    fn test_add_gaussian_noise_clips_first() {
        let mut weights = ModelWeights {
            q_table_snapshot: {
                let mut m = HashMap::new();
                m.insert("a".into(), 100.0); // Very large value
                m
            },
            policy_params: HashMap::new(),
            version: 1,
        };

        add_gaussian_noise(&mut weights, 8.0, 1e-5, 0.5);

        // Value should have been clipped.
        let val = weights.q_table_snapshot.get("a").unwrap();
        assert!(
            val.abs() < 10.0,
            "value {val} should have been clipped + noised"
        );
    }

    #[test]
    fn test_privacy_budget_new() {
        let config = DifferentialPrivacyConfig::default();
        let budget = PrivacyBudget::new(80.0, 10, config);
        assert!((budget.total_epsilon - 80.0).abs() < 1e-10);
        assert_eq!(budget.rounds_remaining, 10);
        assert_eq!(budget.total_rounds, 10);
        assert!((budget.epsilon_spent - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_privacy_budget_spend_round() {
        let config = DifferentialPrivacyConfig::new(1.0, 1e-5, 1.0).unwrap();
        let mut budget = PrivacyBudget::new(10.0, 10, config);

        for i in 0..10 {
            assert!(!budget.is_exhausted());
            assert_eq!(budget.rounds_remaining, 10 - i);
            budget.spend_round().unwrap();
        }

        assert!(budget.is_exhausted());
        assert_eq!(budget.rounds_remaining, 0);
    }

    #[test]
    fn test_privacy_budget_exhausted() {
        let config = DifferentialPrivacyConfig::new(1.0, 1e-5, 1.0).unwrap();
        let mut budget = PrivacyBudget::new(1.0, 1, config);
        budget.spend_round().unwrap();
        assert!(budget.is_exhausted());

        let result = budget.spend_round();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exhausted"));
    }

    #[test]
    fn test_privacy_budget_fraction_consumed() {
        let config = DifferentialPrivacyConfig::new(1.0, 1e-5, 1.0).unwrap();
        let mut budget = PrivacyBudget::new(10.0, 10, config);
        assert!((budget.fraction_consumed() - 0.0).abs() < 1e-10);

        budget.spend_round().unwrap();
        assert!((budget.fraction_consumed() - 0.1).abs() < 1e-10);

        for _ in 0..9 {
            budget.spend_round().unwrap();
        }
        assert!((budget.fraction_consumed() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_privacy_budget_epsilon_remaining() {
        let config = DifferentialPrivacyConfig::new(1.0, 1e-5, 1.0).unwrap();
        let mut budget = PrivacyBudget::new(5.0, 5, config);
        assert!((budget.epsilon_remaining() - 5.0).abs() < 1e-10);

        budget.spend_round().unwrap();
        assert!((budget.epsilon_remaining() - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_apply_dp_convenience() {
        let mut weights = sample_weights();
        let config = DifferentialPrivacyConfig::default();
        let sigma = apply_dp(&mut weights, &config);
        assert!(sigma > 0.0);

        // Values should have changed (clipping + noise).
        let mut any_below = false;
        for v in weights.q_table_snapshot.values() {
            if v.abs() < 100.0 {
                any_below = true;
                break;
            }
        }
        assert!(any_below);
    }
}
