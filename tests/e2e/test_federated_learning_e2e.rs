//! Federated Learning End-to-End
//!
//! Validates the federated learning lifecycle using real go-on types:
//!   node registration → weight submission → privacy budget tracking →
//!   DP noise calibration → weight aggregation → round lifecycle
//!
//! Uses real structs from the `go_on::intelligence::reinforcement` module
//! to verify behavioral invariants, not constructor tautologies.

use std::collections::HashMap;

use go_on::intelligence::reinforcement::federated::{
    AggregationMethod, FederatedConfig, FederatedLearning, FederatedRL, FederatedRLConfig,
    FederatedRound, ModelWeights,
};
use go_on::intelligence::reinforcement::federated_privacy::{
    add_gaussian_noise, clip_gradients, DifferentialPrivacyConfig, PrivacyBudget,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build a minimal `ModelWeights` from a list of (key, value) pairs.
/// The first ~half of entries go into `q_table_snapshot`, the rest into
/// `policy_params`, simulating a typical RL weight structure.
fn make_weights(entries: Vec<(&str, f64)>, version: u64) -> ModelWeights {
    let mut q_table_snapshot = HashMap::new();
    // Put all entries into q_table_snapshot so tests can assert against one map.
    // Entries passed to add_gaussian_noise/clip_gradients are checked against
    // both maps via total L2 norm, so this distribution is fine for tests.
    for (k, v) in &entries {
        q_table_snapshot.insert(k.to_string(), *v);
    }
    // policy_params is left empty to simplify test assertions.
    let policy_params = HashMap::new();
    // But make the second half also go to policy_params for tests that need them.
    // Actually, just use q_table_snapshot.
    ModelWeights {
        q_table_snapshot,
        policy_params,
        version,
    }
}

/// Extract a single value from a parameter map for test assertions.
fn get_val(map: &HashMap<String, f64>, key: &str) -> f64 {
    map.get(key).copied().unwrap_or(f64::NAN)
}

// ── 1. PrivacyBudget: allocation and exhaustion ─────────────────────────────

/// Privacy budget decreases correctly after multiple `spend_round` calls
/// and refuses allocation when the budget is exhausted.
#[test]
fn test_privacy_budget_tracks_spending_and_exhaustion() {
    let dp = DifferentialPrivacyConfig::new(1.0, 1e-5, 1.0).unwrap();
    let mut budget = PrivacyBudget::new(5.0, 5, dp);

    // Spend 3 rounds and verify the budget decreases each time.
    for round in 1..=3 {
        let remaining_before = budget.rounds_remaining;
        let spent_before = budget.epsilon_spent;
        budget.spend_round().unwrap();
        assert_eq!(
            budget.rounds_remaining,
            remaining_before - 1,
            "round {} should decrement rounds_remaining",
            round
        );
        assert!(
            budget.epsilon_spent > spent_before,
            "round {} should increase epsilon_spent (was {}, now {})",
            round,
            spent_before,
            budget.epsilon_spent
        );
    }

    assert!(
        !budget.is_exhausted(),
        "budget should not be exhausted after 3/5 rounds"
    );

    // Spend the remaining 2 rounds.
    budget.spend_round().unwrap();
    budget.spend_round().unwrap();

    assert!(
        budget.is_exhausted(),
        "budget should be exhausted after 5/5 rounds"
    );
    assert_eq!(budget.rounds_remaining, 0, "no rounds should remain");

    // Attempting another spend must fail.
    let err = budget.spend_round().unwrap_err();
    let msg = format!("{:#}", err);
    assert!(
        msg.contains("exhausted") || msg.contains("budget"),
        "exhausted spend error should mention budget exhaustion, got: {}",
        msg
    );
}

/// fraction_consumed and epsilon_remaining report correct values.
#[test]
fn test_privacy_budget_fraction_and_remaining() {
    let dp = DifferentialPrivacyConfig::new(2.0, 1e-5, 1.0).unwrap();
    let mut budget = PrivacyBudget::new(10.0, 5, dp);

    // Initial state: nothing consumed.
    assert!((budget.fraction_consumed() - 0.0).abs() < 1e-10);
    assert!((budget.epsilon_remaining() - 10.0).abs() < 1e-10);

    // Spend one round (costs epsilon=2.0 per the config).
    budget.spend_round().unwrap();
    assert!((budget.epsilon_remaining() - 8.0).abs() < 1e-10);
    assert!((budget.fraction_consumed() - 0.2).abs() < 1e-10);

    // Spend three more rounds (total 8.0 spent).
    budget.spend_round().unwrap();
    budget.spend_round().unwrap();
    budget.spend_round().unwrap();
    assert!((budget.epsilon_remaining() - 2.0).abs() < 1e-10);
    assert!((budget.fraction_consumed() - 0.8).abs() < 1e-10);
}

// ── 2. FederatedLearning: node registration and weight aggregation ──────────

/// Two nodes with different weights submit local model weights.
/// `aggregate_round()` via FedAvg produces a correct averaged result.
#[test]
fn test_federated_aggregate_round_with_two_nodes() {
    let mut fl = FederatedLearning::new(FederatedConfig {
        min_clients: 2,
        ..Default::default()
    });

    fl.register_client("sensor-a", 1.0).unwrap();
    fl.register_client("sensor-b", 1.0).unwrap();

    // Sensor A: q_table values [10.0, 20.0], Sensor B: q_table values [30.0, 40.0]
    let mut w_a = make_weights(vec![("q1", 10.0), ("q2", 20.0)], 1);
    let mut w_b = make_weights(vec![("q1", 30.0), ("q2", 40.0)], 2);
    w_a.policy_params.clear();
    w_a.q_table_snapshot.insert("q2".into(), 20.0);
    w_b.policy_params.clear();
    w_b.q_table_snapshot.insert("q2".into(), 40.0);

    fl.submit_local_weights("sensor-a", w_a, 0.0).unwrap();
    fl.submit_local_weights("sensor-b", w_b, 0.0).unwrap();

    let round: FederatedRound = fl.aggregate_round().unwrap();

    // FedAvg: (10 + 30) / 2 = 20, (20 + 40) / 2 = 30
    let q = &round.global_weights.q_table_snapshot;
    assert!(
        (get_val(q, "q1") - 20.0).abs() < 1e-10,
        "FedAvg q1 expected 20.0, got {}",
        get_val(q, "q1")
    );
    assert!(
        (get_val(q, "q2") - 30.0).abs() < 1e-10,
        "FedAvg q2 expected 30.0, got {}",
        get_val(q, "q2")
    );

    // Round metadata is populated.
    assert_eq!(round.round_id, 1, "first round should have id 1");
    assert_eq!(round.clients_participated.len(), 2);
    assert!(round.aggregated_at_ms > 0);
}

/// Nodes with different registered weights produce a correct FedWeighted result.
#[test]
fn test_federated_aggregate_round_weighted() {
    let mut fl = FederatedLearning::new(FederatedConfig {
        min_clients: 2,
        aggregation_method: AggregationMethod::FedWeighted,
        ..Default::default()
    });

    // sensor-a has weight 3.0, sensor-b has weight 1.0 → total = 4.0
    fl.register_client("sensor-a", 3.0).unwrap();
    fl.register_client("sensor-b", 1.0).unwrap();

    let mut w_a = make_weights(vec![("q1", 10.0), ("q2", 20.0)], 1);
    let mut w_b = make_weights(vec![("q1", 30.0), ("q2", 40.0)], 1);
    w_a.policy_params.clear();
    w_a.q_table_snapshot.insert("q2".into(), 20.0);
    w_b.policy_params.clear();
    w_b.q_table_snapshot.insert("q2".into(), 40.0);

    fl.submit_local_weights("sensor-a", w_a, 0.0).unwrap();
    fl.submit_local_weights("sensor-b", w_b, 0.0).unwrap();

    let round = fl.aggregate_round().unwrap();

    // FedWeighted: alpha weight 3/4, beta weight 1/4
    // q1 = 10*(3/4) + 30*(1/4) = 7.5 + 7.5 = 15.0
    // q2 = 20*(3/4) + 40*(1/4) = 15.0 + 10.0 = 25.0
    let q = &round.global_weights.q_table_snapshot;
    assert!(
        (get_val(q, "q1") - 15.0).abs() < 1e-10,
        "FedWeighted q1 expected 15.0, got {}",
        get_val(q, "q1")
    );
    assert!(
        (get_val(q, "q2") - 25.0).abs() < 1e-10,
        "FedWeighted q2 expected 25.0, got {}",
        get_val(q, "q2")
    );
}

/// Aggregation with fewer clients than `min_clients` must fail.
#[test]
fn test_federated_aggregate_insufficient_clients_fails() {
    let mut fl = FederatedLearning::new(FederatedConfig {
        min_clients: 3,
        ..Default::default()
    });

    fl.register_client("a", 1.0).unwrap();
    fl.register_client("b", 1.0).unwrap();

    let w = make_weights(vec![("q1", 1.0)], 1);
    fl.submit_local_weights("a", w.clone(), 0.0).unwrap();
    fl.submit_local_weights("b", w, 0.0).unwrap();

    let err = fl.aggregate_round().unwrap_err();
    let msg = format!("{:#}", err);
    assert!(
        msg.contains("insufficient"),
        "error should mention insufficient clients, got: {}",
        msg
    );
}

// ── 3. DP noise: epsilon extremes produce expected behavior ─────────────────

/// With a very large epsilon (weak privacy), `add_gaussian_noise` adds
/// negligible noise. Note that after clipping the weights to `clip_norm`,
/// the values are reduced (the total L2 norm of [42, 100] is ~108.5,
/// so with clip_norm=1.0 they become ~[0.387, 0.921]). We verify that:
/// 1. The noise scale σ → 0 as ε → ∞
/// 2. The weights after clipping are stable (noise is negligible)
#[test]
fn test_dp_noise_large_epsilon_produces_small_noise() {
    let mut weights = make_weights(vec![("p1", 42.0), ("p2", 100.0)], 1);

    // Very large epsilon → tiny noise scale.
    let sigma = add_gaussian_noise(&mut weights, 1_000_000.0, 1e-5, 1.0);

    // Noise scale should be near zero.
    assert!(
        sigma < 0.001,
        "noise scale with ε=1e6 should be tiny, got {}",
        sigma
    );

    // After clipping to norm 1.0, weights are scaled by clip_norm/total_norm.
    // total_norm = sqrt(42^2 + 100^2) ≈ 108.5, scale = 1/108.5 ≈ 0.00922
    let scale = 1.0_f64 / (42.0_f64.powi(2) + 100.0_f64.powi(2)).sqrt();
    let expected_p1 = 42.0 * scale;
    let expected_p2 = 100.0 * scale;

    // Weights should be close to their clipped values (noise is negligible).
    assert!(
        (get_val(&weights.q_table_snapshot, "p1") - expected_p1).abs() < 0.01,
        "p1 should be ~{:.4} after clipping + negligible noise, got {}",
        expected_p1,
        get_val(&weights.q_table_snapshot, "p1")
    );
    assert!(
        (get_val(&weights.q_table_snapshot, "p2") - expected_p2).abs() < 0.01,
        "p2 should be ~{:.4} after clipping + negligible noise, got {}",
        expected_p2,
        get_val(&weights.q_table_snapshot, "p2")
    );
}

/// With a small epsilon (strong privacy), `add_gaussian_noise` produces
/// a large noise scale and visibly perturbs the weights.
#[test]
fn test_dp_noise_small_epsilon_produces_large_noise() {
    let mut weights = make_weights(vec![("p1", 42.0), ("p2", 100.0)], 1);

    // Tiny epsilon → large noise scale.
    let sigma = add_gaussian_noise(&mut weights, 0.01, 1e-5, 1.0);

    // Noise scale should be large ( >> 1.0).
    assert!(
        sigma > 10.0,
        "noise scale with ε=0.01 should be large, got {}",
        sigma
    );

    // The weights will have changed — we can't assert a specific value
    // (it's random), but we can assert they're no longer exactly the originals.
    // (There's a vanishingly small probability the noise sums to exactly 0.)
    let p1 = get_val(&weights.q_table_snapshot, "p1");
    let p2 = get_val(&weights.q_table_snapshot, "p2");
    assert!(
        (p1 - 42.0).abs() > 1e-10 || (p2 - 100.0).abs() > 1e-10,
        "weights should be perturbed by large noise, but both are unchanged: p1={}, p2={}",
        p1,
        p2
    );
}

// ── 4. FederatedDiscovery: peer discovery returns registered nodes ──────────
//
// The `federated_discovery` module is feature-gated behind
// `sub-bus-distributed-memory`, so these tests are conditionally compiled.

#[cfg(feature = "sub-bus-distributed-memory")]
use go_on::intelligence::reinforcement::federated_discovery::{
    NodeDiscovery, NodeInfo, NodeRole, StaticDiscovery,
};
#[cfg(feature = "sub-bus-distributed-memory")]
use go_on::intelligence::reinforcement::federated_transport::PeerInfo;

/// `StaticDiscovery::discover()` returns the peers it was initialised with.
#[cfg(feature = "sub-bus-distributed-memory")]
#[tokio::test]
async fn test_static_discovery_returns_registered_peers() {
    let peers = vec![
        PeerInfo {
            id: "worker-1".into(),
            addr: "127.0.0.1:9001".into(),
            role: NodeRole::Worker,
            capabilities: HashMap::new(),
        },
        PeerInfo {
            id: "coordinator-1".into(),
            addr: "127.0.0.1:9000".into(),
            role: NodeRole::Coordinator,
            capabilities: HashMap::new(),
        },
    ];

    let discovery = StaticDiscovery::new(&peers);
    let discovered = discovery.discover().await.unwrap();

    assert_eq!(discovered.len(), 2, "should discover 2 peers");
    let ids: Vec<&str> = discovered.iter().map(|n| n.id.as_str()).collect();
    assert!(
        ids.contains(&"worker-1"),
        "discovered set should include worker-1"
    );
    assert!(
        ids.contains(&"coordinator-1"),
        "discovered set should include coordinator-1"
    );

    // New registrations are reflected in subsequent discover calls.
    let new_node = NodeInfo {
        id: "worker-2".into(),
        addr: "127.0.0.1:9002".into(),
        role: NodeRole::Worker,
        capabilities: HashMap::new(),
        online: true,
        last_heartbeat_ms: 0,
    };
    discovery.register(&new_node).await.unwrap();

    let discovered_after = discovery.discover().await.unwrap();
    assert_eq!(
        discovered_after.len(),
        3,
        "should discover 3 peers after registration"
    );
}

// ── 5. FederatedRL round lifecycle ──────────────────────────────────────────

/// Full distillation round lifecycle:
/// submit policies → start round → contribute → complete → verify merged result.
#[test]
fn test_federated_rl_round_lifecycle() {
    let frl = FederatedRL::new(FederatedRLConfig {
        min_contributors: 2,
        ..Default::default()
    });

    // Submit two policies from different nodes.
    let p1 = frl.submit_policy(
        "node-a".into(),
        "classification".into(),
        "policy:v1".into(),
        0.8,
        100,
    );
    let p2 = frl.submit_policy(
        "node-b".into(),
        "classification".into(),
        "policy:v2".into(),
        0.6,
        200,
    );

    // Policies are retrievable.
    let fetched = frl.get_policy(&p1).unwrap();
    assert_eq!(fetched.node_id, "node-a");
    assert!((fetched.reward_avg - 0.8).abs() < 1e-10);
    assert_eq!(fetched.sample_count, 100);

    // Start a distillation round.
    let round_id = frl.start_distillation_round();

    // Contribute both policies.
    frl.contribute_to_round(&round_id, &p1).unwrap();
    frl.contribute_to_round(&round_id, &p2).unwrap();

    // Complete the round.
    let completed = frl.complete_round(&round_id).unwrap();

    // Verify round metadata.
    assert_eq!(completed.contributor_count, 2);
    assert_eq!(completed.contributed_policy_ids.len(), 2);
    assert!(completed.contributed_policy_ids.contains(&p1));
    assert!(completed.contributed_policy_ids.contains(&p2));
    assert!(completed.completed_ms > 0);

    // The merged policy should contain a JSON structure with the weighted average.
    let merged = completed.merged_policy.unwrap();
    assert!(
        merged.contains("weighted_avg_reward"),
        "merged policy should contain reward info"
    );
    assert!(
        merged.contains("total_samples"),
        "merged policy should contain sample total count"
    );
    assert!(
        merged.contains("300"),
        "merged policy should reflect total samples (100+200)"
    );
}

/// Round completion fails when fewer than `min_contributors` contribute.
#[test]
fn test_federated_rl_round_insufficient_contributors_fails() {
    let frl = FederatedRL::new(FederatedRLConfig {
        min_contributors: 3,
        ..Default::default()
    });

    let p = frl.submit_policy("node-a".into(), "regression".into(), "p:v1".into(), 0.5, 50);
    let round_id = frl.start_distillation_round();
    frl.contribute_to_round(&round_id, &p).unwrap();

    let err = frl.complete_round(&round_id).unwrap_err();
    let msg = format!("{:#}", err);
    assert!(
        msg.contains("InsufficientContributors") || msg.contains("insufficient"),
        "error should mention insufficient contributors, got: {}",
        msg
    );
}

// ── 6. Clip-gradients boundary behavior ─────────────────────────────────────

/// `clip_gradients()` does not modify weights whose L2 norm is within the
/// clip bound, but clips weights that exceed it.
#[test]
fn test_clip_gradients_preserves_small_weights_and_clips_large() {
    // Small weights: L2 norm = sqrt(9 + 16) = 5.0, clip_norm = 10.0 → no change.
    let mut small = make_weights(vec![("a", 3.0), ("b", 4.0)], 1);
    clip_gradients(&mut small, 10.0);
    assert!(
        (get_val(&small.q_table_snapshot, "a") - 3.0).abs() < 1e-10,
        "small weight 'a' should be unchanged after clip with high bound"
    );
    assert!(
        (get_val(&small.q_table_snapshot, "b") - 4.0).abs() < 1e-10,
        "small weight 'b' should be unchanged after clip with high bound"
    );

    // Large weights: L2 norm = sqrt(60^2 + 80^2) = 100.0, clip_norm = 10.0 → scale by 0.1.
    let mut large = make_weights(vec![("x", 60.0), ("y", 80.0)], 1);
    clip_gradients(&mut large, 10.0);
    assert!(
        (get_val(&large.q_table_snapshot, "x") - 6.0).abs() < 1e-10,
        "large weight 'x' should be clipped to 6.0, got {}",
        get_val(&large.q_table_snapshot, "x")
    );
    assert!(
        (get_val(&large.q_table_snapshot, "y") - 8.0).abs() < 1e-10,
        "large weight 'y' should be clipped to 8.0, got {}",
        get_val(&large.q_table_snapshot, "y")
    );
}

// ── 7. Multiple rounds accumulate ───────────────────────────────────────────

/// Two successive aggregation rounds both produce correct results and the
/// profile reflects the accumulated state.
#[test]
fn test_multiple_rounds_accumulate_correctly() {
    let mut fl = FederatedLearning::new(FederatedConfig {
        min_clients: 2,
        ..Default::default()
    });

    fl.register_client("alpha", 1.0).unwrap();
    fl.register_client("beta", 1.0).unwrap();

    // Round 1: alpha submits 10, beta submits 30 → average = 20.
    let w1 = make_weights(vec![("q1", 10.0)], 1);
    let w2 = make_weights(vec![("q1", 30.0)], 1);
    fl.submit_local_weights("alpha", w1, 0.1).unwrap();
    fl.submit_local_weights("beta", w2, 0.2).unwrap();
    let r1 = fl.aggregate_round().unwrap();
    assert_eq!(r1.round_id, 1);
    assert!((get_val(&r1.global_weights.q_table_snapshot, "q1") - 20.0).abs() < 1e-10);

    // Round 2: alpha submits 50, beta submits 70 → average = 60.
    let w1b = make_weights(vec![("q1", 50.0)], 2);
    let w2b = make_weights(vec![("q1", 70.0)], 2);
    fl.submit_local_weights("alpha", w1b, 0.3).unwrap();
    fl.submit_local_weights("beta", w2b, 0.4).unwrap();
    let r2 = fl.aggregate_round().unwrap();
    assert_eq!(r2.round_id, 2);
    assert!((get_val(&r2.global_weights.q_table_snapshot, "q1") - 60.0).abs() < 1e-10);

    // Global weights reflect the latest round.
    let gw = fl.get_global_weights().unwrap();
    assert_eq!(gw.version, 2);
    assert!((get_val(&gw.q_table_snapshot, "q1") - 60.0).abs() < 1e-10);

    // Profile reflects two completed rounds.
    let p = fl.profile();
    assert_eq!(p.total_rounds, 2);
    assert_eq!(p.total_clients, 2);

    // After round 1: alpha avg_improvement = 0.1, beta = 0.2 => score = 0.15
    // After round 2: alpha = 0.1*(1/2)+0.3/2 = 0.2, beta = 0.2*(1/2)+0.4/2 = 0.3 => score = 0.25
    // total_improvement_sum = 0.15 + 0.25 = 0.40, avg = 0.40 / 2 = 0.20
    assert!(
        (p.avg_improvement - 0.20).abs() < 1e-10,
        "avg_improvement expected 0.20, got {}",
        p.avg_improvement
    );
}
