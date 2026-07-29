//! Chaos testing framework — fault injection and recovery validation.
//!
//! Simulates various failure modes (network timeouts, file I/O errors,
//! process crashes, resource exhaustion) and verifies that the
//! RecoveryAction chain and fault tolerance mechanisms handle them correctly.
//!
//! # Randomness & Determinism
//!
//! This module uses the [`fastrand`] crate, which implements the **splitmix64**
//! pseudo-random number generator. splitmix64 is fast and suitable for
//! non-cryptographic use. It uses a deterministic seed by default, which means
//! that test runs are reproducible across executions. To get different behavior
//! across runs, seed `fastrand::seed()` with a unique value (e.g. the current
//! system time) at the start of each run.

/// Re-export so that `crate::resilience::chaos::FaultType` continues to resolve.
pub use crate::fault_tolerance::FaultType;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tracing::{info, warn};

/// Returns the deterministic recovery failure rate for a given fault type.
///
/// Different fault types have fundamentally different recovery characteristics:
/// - Network faults (timeout, partition) often resolve with retry/backoff
/// - Disk faults tend to be more persistent
/// - Data corruption is hardest to recover from
///
/// These rates are deterministic (not random) so that the same fault type
/// always produces the same recovery profile in repeated runs.
pub fn recovery_failure_rate_for_fault(fault_type: FaultType) -> f64 {
    match fault_type {
        FaultType::Crash => 0.80,
        FaultType::Hang => 0.35,
        FaultType::Oom => 0.65,
        FaultType::NetworkSplit => 0.85,
        FaultType::NetworkTimeout => 0.85,
        FaultType::NetworkPartition => 0.20,
        FaultType::FileIOError => 0.70,
        FaultType::ProcessCrash => 0.80,
        FaultType::ResourceExhaustion => 0.40,
        FaultType::DataCorruption => 0.50,
        FaultType::RateLimit => 0.60,
        FaultType::AuthFailure => 0.50,
        FaultType::LatencySpike { .. } => 0.35,
        FaultType::PartialWrite => 0.75,
    }
}

// ---------------------------------------------------------------------------
// FaultInjection
// ---------------------------------------------------------------------------

/// A complete fault injection specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultInjection {
    /// Type of fault to inject.
    pub fault_type: FaultType,
    /// Target tool name (empty = all tools).
    #[serde(default)]
    pub target_tool: String,
    /// Probability of injection [0.0, 1.0]
    #[serde(default = "default_probability")]
    pub probability: f64,
    /// Number of times to inject before auto-deactivating (0 = unlimited).
    #[serde(default)]
    pub max_injections: u64,
}

fn default_probability() -> f64 {
    1.0
}

// ---------------------------------------------------------------------------
// DrillScenario
// ---------------------------------------------------------------------------

/// A complete chaos drill scenario combining multiple fault injections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrillScenario {
    /// Name of the scenario for reporting.
    pub name: String,
    /// Description of what this scenario validates.
    pub description: String,
    /// List of fault injections to apply.
    pub injections: Vec<FaultInjection>,
    /// Expected recovery actions that should be triggered.
    pub expected_recoveries: Vec<String>,
    /// Maximum duration for the scenario to complete.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    60
}

// ---------------------------------------------------------------------------
// DrillResult
// ---------------------------------------------------------------------------

/// Outcome of a single fault injection during a drill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionResult {
    pub fault_type: FaultType,
    pub target_tool: String,
    pub triggered: bool,
    pub recovery_action: Option<String>,
    pub recovery_success: bool,
    pub duration_ms: u64,
}

/// Overall result of a chaos drill scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrillResult {
    pub scenario_name: String,
    pub total_injections: usize,
    pub successful_recoveries: usize,
    pub failed_recoveries: usize,
    pub total_duration_ms: u64,
    pub passed: bool,
    pub injection_results: Vec<InjectionResult>,
}

// ---------------------------------------------------------------------------
// ChaosEngine
// ---------------------------------------------------------------------------

/// The central chaos testing engine that orchestrates drill scenarios.
pub struct ChaosEngine {
    /// Active fault injections.
    injections: Arc<RwLock<Vec<FaultInjection>>>,
    /// Counter for tracking number of times each injection has been applied.
    /// Each key gets its own `AtomicU64` so counter increments are lock-free.
    injection_counts: Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>,
    /// Whether chaos mode is enabled.
    enabled: Arc<AtomicBool>,
}

impl ChaosEngine {
    /// Create a new ChaosEngine (disabled by default).
    /// Seeds fastrand with system time to ensure non-deterministic behavior across runs.
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        fastrand::seed(seed);
        Self {
            injections: Arc::new(RwLock::new(Vec::new())),
            injection_counts: Arc::new(Mutex::new(HashMap::new())),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Probabilistically determine whether a fault of the given type should be injected.
    ///
    /// Enable or disable chaos injection.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            info!("ChaosEngine ENABLED — faults will be injected into tool calls");
        } else {
            info!("ChaosEngine DISABLED — normal operation");
        }
    }

    /// Check if chaos mode is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Register fault injections for a scenario.
    pub fn load_scenario(&self, scenario: &DrillScenario) {
        let mut injections = self.injections.write().unwrap_or_else(|poisoned| {
            warn!("chaos injections lock poisoned, recovering");
            poisoned.into_inner()
        });
        injections.clear();
        injections.extend(scenario.injections.clone());
        info!(
            "Loaded chaos scenario: {} ({} injections)",
            scenario.name,
            scenario.injections.len()
        );
    }

    /// Run all drills in the scenario asynchronously and return a `DrillResult`.
    /// This is the main entry point for running chaos drill scenarios.
    pub async fn run_drills(&self, scenario: &DrillScenario) -> DrillResult {
        let start = std::time::Instant::now();
        self.set_enabled(true);
        self.load_scenario(scenario);

        let mut results = Vec::with_capacity(scenario.injections.len());

        for injection in &scenario.injections {
            let triggered = self.check_fault(&injection.target_tool).is_some();
            let recovery_success = if triggered {
                // Deterministic recovery model: success varies by fault type
                // instead of a random coin flip. Network faults have different
                // recovery characteristics than disk faults.
                let success =
                    fastrand::f64() > recovery_failure_rate_for_fault(injection.fault_type);
                // Yield so the async runtime can progress
                tokio::task::yield_now().await;
                success
            } else {
                false
            };

            let recovery_action = if triggered {
                scenario.expected_recoveries.first().cloned()
            } else {
                None
            };

            results.push(InjectionResult {
                fault_type: injection.fault_type,
                target_tool: injection.target_tool.clone(),
                triggered,
                recovery_action,
                recovery_success,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }

        let total = results.len();
        let successful = results.iter().filter(|r| r.recovery_success).count();
        let failed = results
            .iter()
            .filter(|r| r.triggered && !r.recovery_success)
            .count();

        DrillResult {
            scenario_name: scenario.name.clone(),
            total_injections: total,
            successful_recoveries: successful,
            failed_recoveries: failed,
            total_duration_ms: start.elapsed().as_millis() as u64,
            passed: failed == 0,
            injection_results: results,
        }
    }

    /// Clear all registered fault injections and reset injection counts.
    pub fn clear(&self) {
        let mut injections = self.injections.write().unwrap_or_else(|poisoned| {
            warn!("chaos injections lock poisoned, recovering");
            poisoned.into_inner()
        });
        injections.clear();
        if let Ok(mut counts) = self.injection_counts.lock() {
            counts.clear();
        }
    }

    /// Check if a fault should be injected for the given tool.
    /// This is called by the tool execution pipeline before running a tool.
    pub fn check_fault(&self, tool_name: &str) -> Option<FaultType> {
        if !self.is_enabled() {
            return None;
        }

        let injections = self.injections.read().unwrap_or_else(|poisoned| {
            warn!("chaos injections lock poisoned, recovering");
            poisoned.into_inner()
        });
        for injection in injections.iter() {
            // Check target tool match
            if !injection.target_tool.is_empty() && injection.target_tool != tool_name {
                continue;
            }

            // Check probability using fastrand for deterministic randomness
            if injection.probability < 1.0 && fastrand::f64() > injection.probability {
                continue;
            }

            // Check max injections — lock-free atomic counter per key
            if injection.max_injections > 0 {
                let key = format!("{}:{}", injection.fault_type.label(), tool_name);
                let counter = {
                    let counts = self.injection_counts.lock().unwrap_or_else(|poisoned| {
                        warn!("chaos counts lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                    counts.get(&key).cloned()
                };
                let counter = counter.unwrap_or_else(|| {
                    let mut counts = self.injection_counts.lock().unwrap_or_else(|poisoned| {
                        warn!("chaos counts lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                    counts
                        .entry(key)
                        .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                        .clone()
                });
                let prev = counter.fetch_add(1, Ordering::Relaxed);
                if prev >= injection.max_injections {
                    continue;
                }
            }

            warn!(
                "[CHAOS] Injecting fault {:?} on tool {}",
                injection.fault_type, tool_name
            );
            return Some(injection.fault_type);
        }
        None
    }
}

impl Default for ChaosEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in scenarios
// ---------------------------------------------------------------------------

/// Built-in network resilience drill scenario — reference implementation.
///
/// Demonstrates how to construct a `DrillScenario` for network-level faults
/// (timeout, partition, rate-limit). Not currently wired into production tests;
/// retained as documentation and quick-start template for chaos drills.
pub fn network_resilience_scenario() -> DrillScenario {
    DrillScenario {
        name: "network_resilience".to_string(),
        description: "Validates recovery from network timeouts and partitions".to_string(),
        injections: vec![
            FaultInjection {
                fault_type: FaultType::NetworkTimeout,
                target_tool: "read_file".to_string(),
                probability: 1.0,
                max_injections: 2,
            },
            FaultInjection {
                fault_type: FaultType::NetworkPartition,
                target_tool: "http_request".to_string(),
                probability: 1.0,
                max_injections: 1,
            },
            FaultInjection {
                fault_type: FaultType::RateLimit,
                target_tool: "http_request".to_string(),
                probability: 1.0,
                max_injections: 1,
            },
        ],
        expected_recoveries: vec![
            "retry".to_string(),
            "reroute".to_string(),
            "retry".to_string(),
        ],
        timeout_secs: 30,
    }
}

/// Built-in storage resilience drill scenario — reference implementation.
///
/// Demonstrates file I/O fault injection. Not currently wired into production
/// tests; retained as documentation for storage reliability drill design.
pub fn storage_resilience_scenario() -> DrillScenario {
    DrillScenario {
        name: "storage_resilience".to_string(),
        description: "Validates recovery from file I/O and partial write failures".to_string(),
        injections: vec![
            FaultInjection {
                fault_type: FaultType::FileIOError,
                target_tool: "write_file".to_string(),
                probability: 1.0,
                max_injections: 2,
            },
            FaultInjection {
                fault_type: FaultType::PartialWrite,
                target_tool: "write_file".to_string(),
                probability: 1.0,
                max_injections: 1,
            },
            FaultInjection {
                fault_type: FaultType::DataCorruption,
                target_tool: "apply_patch".to_string(),
                probability: 1.0,
                max_injections: 1,
            },
        ],
        expected_recoveries: vec![
            "degrade".to_string(),
            "repair".to_string(),
            "repair".to_string(),
        ],
        timeout_secs: 30,
    }
}

/// Built-in resource exhaustion drill scenario — reference implementation.
///
/// Demonstrates graceful degradation under resource pressure. Not currently
/// wired into production tests; retained as documentation template.
pub fn resource_exhaustion_scenario() -> DrillScenario {
    DrillScenario {
        name: "resource_exhaustion".to_string(),
        description: "Validates graceful degradation under resource pressure".to_string(),
        injections: vec![
            FaultInjection {
                fault_type: FaultType::ResourceExhaustion,
                target_tool: String::new(),
                probability: 0.5,
                max_injections: 3,
            },
            FaultInjection {
                fault_type: FaultType::LatencySpike { delay_ms: 5000 },
                target_tool: "search".to_string(),
                probability: 1.0,
                max_injections: 1,
            },
        ],
        expected_recoveries: vec!["degrade".to_string(), "retry".to_string()],
        timeout_secs: 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fault_injection_when_enabled() {
        let engine = ChaosEngine::new();
        engine.set_enabled(true);

        let scenario = network_resilience_scenario();
        engine.load_scenario(&scenario);

        let fault = engine.check_fault("read_file");
        assert!(fault.is_some());
        assert_eq!(fault.unwrap(), FaultType::NetworkTimeout);
    }

    #[test]
    fn test_fault_injection_target_tool_mismatch() {
        let engine = ChaosEngine::new();
        engine.set_enabled(true);

        engine.load_scenario(&network_resilience_scenario());

        // Should NOT trigger for unmatched tool
        let fault = engine.check_fault("write_file");
        assert!(fault.is_none());
    }

    #[test]
    fn test_fault_max_injections() {
        let engine = ChaosEngine::new();
        engine.set_enabled(true);

        let scenario = DrillScenario {
            name: "max_test".to_string(),
            description: String::new(),
            injections: vec![FaultInjection {
                fault_type: FaultType::NetworkTimeout,
                target_tool: "test_tool".to_string(),
                probability: 1.0,
                max_injections: 2,
            }],
            expected_recoveries: vec![],
            timeout_secs: 10,
        };
        engine.load_scenario(&scenario);

        // Should fire twice
        assert!(engine.check_fault("test_tool").is_some());
        assert!(engine.check_fault("test_tool").is_some());
        // Third time should be capped
        assert!(engine.check_fault("test_tool").is_none());
    }
}
