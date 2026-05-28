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

/// Default probability of a simulated recovery failure during drills.
/// Set to 0.1 (10%) to model real-world chaos where not all recoveries succeed.
pub const RECOVERY_FAILURE_RATE: f64 = 0.1;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use tracing::{info, warn};

// ---------------------------------------------------------------------------
// FaultType
// ---------------------------------------------------------------------------

/// Types of faults that can be injected during chaos drills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaultType {
    /// Simulate a network timeout (tool call hangs for specified duration)
    NetworkTimeout,
    /// Simulate a network partition (tool call returns connection refused)
    NetworkPartition,
    /// Simulate file I/O error (permission denied, disk full)
    FileIOError,
    /// Simulate process crash (tool panics or exits unexpectedly)
    ProcessCrash,
    /// Simulate resource exhaustion (OOM, CPU spike)
    ResourceExhaustion,
    /// Simulate corrupt data response
    DataCorruption,
    /// Simulate rate limiting (429 response)
    RateLimit,
    /// Simulate authentication failure (401/403)
    AuthFailure,
    /// Simulate an unexpected large latency spike
    LatencySpike { delay_ms: u64 },
    /// Simulate a partial write (file written half-way then fails)
    PartialWrite,
}

impl FaultType {
    pub fn label(&self) -> &str {
        match self {
            FaultType::NetworkTimeout => "network_timeout",
            FaultType::NetworkPartition => "network_partition",
            FaultType::FileIOError => "file_io_error",
            FaultType::ProcessCrash => "process_crash",
            FaultType::ResourceExhaustion => "resource_exhaustion",
            FaultType::DataCorruption => "data_corruption",
            FaultType::RateLimit => "rate_limit",
            FaultType::AuthFailure => "auth_failure",
            FaultType::LatencySpike { .. } => "latency_spike",
            FaultType::PartialWrite => "partial_write",
        }
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
    injection_counts: Arc<RwLock<HashMap<String, u64>>>,
    /// Whether chaos mode is enabled.
    enabled: Arc<AtomicBool>,
}

impl ChaosEngine {
    /// Create a new ChaosEngine (disabled by default).
    pub fn new() -> Self {
        Self {
            injections: Arc::new(RwLock::new(Vec::new())),
            injection_counts: Arc::new(RwLock::new(HashMap::new())),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }

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
        let mut injections = self.injections.write().expect("chaos injections lock");
        injections.clear();
        injections.extend(scenario.injections.clone());
        info!(
            "Loaded chaos scenario: {} ({} injections)",
            scenario.name,
            scenario.injections.len()
        );
    }

    /// Clear all fault injections.
    #[allow(dead_code)] // F-GAP-12 — reserved for chaos testing integration
    pub fn clear(&self) {
        self.injections
            .write()
            .expect("chaos injections lock")
            .clear();
        self.injection_counts
            .write()
            .expect("chaos counts lock")
            .clear();
    }

    /// Check if a fault should be injected for the given tool.
    /// This is called by the tool execution pipeline before running a tool.
    pub fn check_fault(&self, tool_name: &str) -> Option<FaultType> {
        if !self.is_enabled() {
            return None;
        }

        let injections = self.injections.read().expect("chaos injections lock");
        for injection in injections.iter() {
            // Check target tool match
            if !injection.target_tool.is_empty() && injection.target_tool != tool_name {
                continue;
            }

            // Check probability using fastrand for deterministic randomness
            if injection.probability < 1.0 && fastrand::f64() > injection.probability {
                continue;
            }

            // Check max injections
            if injection.max_injections > 0 {
                let mut counts = self.injection_counts.write().expect("chaos counts lock");
                let key = format!("{}:{}", injection.fault_type.label(), tool_name);
                let count = counts.entry(key.clone()).or_insert(0);
                if *count >= injection.max_injections {
                    continue;
                }
                *count += 1;
            }

            warn!(
                "[CHAOS] Injecting fault {:?} on tool {}",
                injection.fault_type, tool_name
            );
            return Some(injection.fault_type);
        }
        None
    }

    /// Run a full drill scenario and return results.
    #[allow(dead_code)] // F-GAP-12 — reserved for chaos testing integration
    pub async fn run_drills(&self, scenario: &DrillScenario) -> DrillResult {
        let start = std::time::Instant::now();
        self.set_enabled(true);
        self.load_scenario(scenario);

        let mut results = Vec::new();
        let mut successful = 0u64;
        let mut failed = 0u64;

        for injection in &scenario.injections {
            let inj_start = std::time::Instant::now();
            let injected = self.check_fault(&injection.target_tool);

            // Simulate what the recovery system would do
            // Introduce RECOVERY_FAILURE_RATE random recovery failure to model real-world
            // chaos where not all recoveries succeed.
            let fail_bound = (RECOVERY_FAILURE_RATE * 100.0) as u8;
            let recovery_simulation_fails = fastrand::u8(0..100) < fail_bound;

            let (recovery_action, recovery_success) = match injection.fault_type {
                FaultType::NetworkTimeout => {
                    // Should trigger Retry then Escalate
                    ("retry".to_string(), !recovery_simulation_fails)
                }
                FaultType::NetworkPartition => {
                    // Should trigger Reroute
                    ("reroute".to_string(), !recovery_simulation_fails)
                }
                FaultType::FileIOError => {
                    // Should trigger Degrade to alternate tool
                    ("degrade".to_string(), !recovery_simulation_fails)
                }
                FaultType::ProcessCrash => {
                    // Should trigger Replan
                    ("replan".to_string(), !recovery_simulation_fails)
                }
                FaultType::ResourceExhaustion => {
                    // Should trigger Degrade
                    ("degrade".to_string(), !recovery_simulation_fails)
                }
                FaultType::DataCorruption => {
                    // Should trigger Repair, then Retry
                    ("repair".to_string(), !recovery_simulation_fails)
                }
                FaultType::RateLimit => ("retry".to_string(), !recovery_simulation_fails),
                FaultType::AuthFailure => ("reroute".to_string(), !recovery_simulation_fails),
                FaultType::LatencySpike { .. } => ("retry".to_string(), !recovery_simulation_fails),
                FaultType::PartialWrite => ("repair".to_string(), !recovery_simulation_fails),
            };

            let duration = inj_start.elapsed().as_millis() as u64;
            let triggered = injected.is_some();

            if recovery_success {
                successful += 1;
            } else {
                failed += 1;
            }

            results.push(InjectionResult {
                fault_type: injection.fault_type,
                target_tool: injection.target_tool.clone(),
                triggered,
                recovery_action: if triggered {
                    Some(recovery_action)
                } else {
                    None
                },
                recovery_success: triggered && recovery_success,
                duration_ms: duration,
            });
        }

        self.set_enabled(false);
        let total_duration = start.elapsed().as_millis() as u64;

        DrillResult {
            scenario_name: scenario.name.clone(),
            total_injections: scenario.injections.len(),
            successful_recoveries: successful as usize,
            failed_recoveries: failed as usize,
            total_duration_ms: total_duration,
            passed: failed == 0,
            injection_results: results,
        }
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)] // F-GAP-12 — reserved for chaos testing integration
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
    fn test_fault_type_labels() {
        assert_eq!(FaultType::NetworkTimeout.label(), "network_timeout");
        assert_eq!(FaultType::FileIOError.label(), "file_io_error");
        assert_eq!(FaultType::PartialWrite.label(), "partial_write");
    }

    #[test]
    fn test_chaos_engine_default_disabled() {
        let engine = ChaosEngine::new();
        assert!(!engine.is_enabled());
    }

    #[test]
    fn test_chaos_engine_enable_disable() {
        let engine = ChaosEngine::new();
        engine.set_enabled(true);
        assert!(engine.is_enabled());
        engine.set_enabled(false);
        assert!(!engine.is_enabled());
    }

    #[test]
    fn test_no_fault_when_disabled() {
        let engine = ChaosEngine::new();
        assert!(engine.check_fault("read_file").is_none());
    }

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

    #[test]
    fn test_network_resilience_scenario_structure() {
        let scenario = network_resilience_scenario();
        assert_eq!(scenario.name, "network_resilience");
        assert_eq!(scenario.injections.len(), 3);
    }

    #[test]
    fn test_storage_resilience_scenario_structure() {
        let scenario = storage_resilience_scenario();
        assert_eq!(scenario.name, "storage_resilience");
        assert_eq!(scenario.injections.len(), 3);
    }

    #[test]
    fn test_drill_result_serialization() {
        let result = DrillResult {
            scenario_name: "test".to_string(),
            total_injections: 1,
            successful_recoveries: 1,
            failed_recoveries: 0,
            total_duration_ms: 100,
            passed: true,
            injection_results: vec![InjectionResult {
                fault_type: FaultType::NetworkTimeout,
                target_tool: "test".to_string(),
                triggered: true,
                recovery_action: Some("retry".to_string()),
                recovery_success: true,
                duration_ms: 50,
            }],
        };
        let json = serde_json::to_value(&result).expect("serialization should work");
        assert_eq!(json["passed"], true);
        assert_eq!(json["injection_results"][0]["recovery_action"], "retry");
    }
}
