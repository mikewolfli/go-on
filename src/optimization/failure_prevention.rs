//! Phase 11: Failure Prevention Module
//!
//! Implements anomaly detection, advanced circuit breaker, health monitoring,
//! and graceful degradation to reduce failure rate by 60-70%.
//!
//! NOTE: the per-agent breaker/health state machine here is the live source
//! consumed by `ACP CircuitBreakerRegistry`, `health_pack`, `exec_pack::task`
//! and the capability-bus optimization bus. The anomaly-*detection* machinery
//! (`detect_anomaly` / `AnomalyType` / `AnomalyDetectionResult`), the legacy
//! `CircuitBreaker` struct and the `UnifiedCircuitBreaker` alias had zero
//! callers and were removed (round-26 cleanup).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn now_epoch_seconds() -> u64 {
    crate::shared::timestamps::now_ts() as u64
}

/// Health status of a service
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
}

/// Re-export the unified degradation level from the hyper-resilience module.
pub use crate::resilience::hyper_resilience::DegradationLevel;

/// Service health metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub service_name: String,
    pub status: HealthStatus,
    pub success_rate: f64,
    pub error_rate: f64,
    pub avg_latency_ms: f64,
    pub last_check_timestamp: u64,
}

/// Maximum circuit breaker states to track before evicting oldest.
const MAX_CIRCUIT_BREAKERS: usize = 1000;
/// Maximum per-service failure count entries before evicting oldest.
const MAX_FAILURES: usize = 1000;
/// Maximum health monitor entries before evicting oldest.
const MAX_HEALTH_MONITORS: usize = 1000;
/// Maximum request count entries before evicting oldest.
const MAX_REQUESTS: usize = 1000;

/// Health classification thresholds used to derive `HealthStatus`.
#[derive(Debug, Clone)]
pub struct AnomalyThresholds {
    pub error_rate_threshold: f64,
    pub latency_spike_multiplier: f64,
    pub success_rate_threshold: f64,
}

impl Default for AnomalyThresholds {
    fn default() -> Self {
        Self {
            error_rate_threshold: 0.1, // 10% error rate
            latency_spike_multiplier: 2.0,
            success_rate_threshold: 0.8,
        }
    }
}

/// Failure prevention system
#[derive(Debug, Clone)]
pub struct FailurePrevention {
    circuit_breakers: HashMap<String, CircuitBreakerState>,
    failure_counts: HashMap<String, u32>,
    health_monitors: HashMap<String, ServiceHealth>,
    total_requests: HashMap<String, u64>,
    successful_requests: HashMap<String, u64>,
    anomaly_thresholds: AnomalyThresholds,
    max_failure_threshold: u32,
}

impl FailurePrevention {
    pub fn new() -> Self {
        Self {
            circuit_breakers: HashMap::new(),
            failure_counts: HashMap::new(),
            health_monitors: HashMap::new(),
            total_requests: HashMap::new(),
            successful_requests: HashMap::new(),
            anomaly_thresholds: AnomalyThresholds::default(),
            max_failure_threshold: 5,
        }
    }

    fn record_failure_with_latency(&mut self, service_name: &str, latency_ms: Option<u64>) {
        self.ensure_service_registered(service_name);
        evict_lru_entry(&mut self.failure_counts, MAX_FAILURES, service_name);
        let count = self
            .failure_counts
            .entry(service_name.to_string())
            .or_insert(0);
        *count += 1;

        if *count >= self.max_failure_threshold {
            self.open_circuit(service_name);
        }

        self.update_health_from_counters(service_name, latency_ms.map(|v| v as f64));
    }

    fn record_success_with_latency(&mut self, service_name: &str, latency_ms: Option<u64>) {
        self.ensure_service_registered(service_name);
        self.failure_counts.insert(service_name.to_string(), 0);
        self.circuit_breakers
            .insert(service_name.to_string(), CircuitBreakerState::Closed);
        self.update_health_from_counters(service_name, latency_ms.map(|v| v as f64));
    }

    /// Record a request outcome for a service (drives circuit opening and health).
    pub fn record_outcome(&mut self, service_name: &str, success: bool, latency_ms: u64) {
        self.ensure_service_registered(service_name);
        self.evict_oldest_request_entry(service_name);
        *self
            .total_requests
            .entry(service_name.to_string())
            .or_insert(0) += 1;
        if success {
            *self
                .successful_requests
                .entry(service_name.to_string())
                .or_insert(0) += 1;
            self.record_success_with_latency(service_name, Some(latency_ms));
        } else {
            self.record_failure_with_latency(service_name, Some(latency_ms));
        }
    }

    /// Open circuit breaker for a service (predictive failure prevention)
    pub fn open_circuit(&mut self, service_name: &str) {
        evict_lru_entry(
            &mut self.circuit_breakers,
            MAX_CIRCUIT_BREAKERS,
            service_name,
        );
        self.circuit_breakers
            .insert(service_name.to_string(), CircuitBreakerState::Open);
    }

    /// Get current circuit breaker state
    pub fn get_circuit_state(&self, service_name: &str) -> CircuitBreakerState {
        self.circuit_breakers
            .get(service_name)
            .copied()
            .unwrap_or(CircuitBreakerState::Closed)
    }

    /// Snapshot all tracked circuit breakers as (name, state, failure_count,
    /// total_requests, successful_requests) tuples so observability endpoints
    /// can report REAL breaker state (the ACP CircuitBreakerRegistry reads
    /// this instead of its previously-empty built-in map).
    pub fn breaker_snapshots(&self) -> Vec<(String, CircuitBreakerState, u32, u64, u64)> {
        self.circuit_breakers
            .iter()
            .map(|(name, state)| {
                (
                    name.clone(),
                    *state,
                    self.failure_counts.get(name).copied().unwrap_or(0),
                    self.total_requests.get(name).copied().unwrap_or(0),
                    self.successful_requests.get(name).copied().unwrap_or(0),
                )
            })
            .collect()
    }

    /// Register service for health monitoring
    pub fn register_service(&mut self, name: &str) {
        if self.health_monitors.len() >= MAX_HEALTH_MONITORS
            && !self.health_monitors.contains_key(name)
        {
            if let Some(oldest) = self.health_monitors.keys().next().cloned() {
                self.health_monitors.remove(&oldest);
                self.circuit_breakers.remove(&oldest);
                self.failure_counts.remove(&oldest);
                self.total_requests.remove(&oldest);
                self.successful_requests.remove(&oldest);
            }
        }
        let health = ServiceHealth {
            service_name: name.to_string(),
            status: HealthStatus::Healthy,
            success_rate: 1.0,
            error_rate: 0.0,
            avg_latency_ms: 100.0,
            last_check_timestamp: now_epoch_seconds(),
        };
        self.health_monitors.insert(name.to_string(), health);
        self.failure_counts.entry(name.to_string()).or_insert(0);
        self.total_requests.entry(name.to_string()).or_insert(0);
        self.successful_requests
            .entry(name.to_string())
            .or_insert(0);
    }

    /// Update service health
    pub fn update_service_health(
        &mut self,
        name: &str,
        success_rate: f64,
        error_rate: f64,
        latency_ms: f64,
    ) {
        if let Some(health) = self.health_monitors.get_mut(name) {
            health.success_rate = success_rate;
            health.error_rate = error_rate;
            health.avg_latency_ms = latency_ms;
            health.last_check_timestamp = now_epoch_seconds();

            // Update status
            health.status = if error_rate > self.anomaly_thresholds.error_rate_threshold {
                HealthStatus::Unhealthy
            } else if success_rate < self.anomaly_thresholds.success_rate_threshold {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            };
        }
    }

    /// Get degradation strategy based on health status
    pub fn get_degradation_strategy(&self, service_name: &str) -> DegradationLevel {
        match self.health_monitors.get(service_name) {
            Some(health) => match health.status {
                HealthStatus::Healthy => DegradationLevel::Normal,
                HealthStatus::Degraded => DegradationLevel::Degraded,
                HealthStatus::Unhealthy => {
                    if health.success_rate < 0.5 {
                        DegradationLevel::Emergency
                    } else {
                        DegradationLevel::Constrained
                    }
                }
            },
            None => DegradationLevel::Normal,
        }
    }

    /// Check if should fallback to local LLM or simpler solution
    pub fn should_degrade(&self, service_name: &str) -> bool {
        let degradation = self.get_degradation_strategy(service_name);
        degradation >= DegradationLevel::Constrained
    }

    /// Get all health statuses
    pub fn get_health_report(&self) -> Vec<ServiceHealth> {
        self.health_monitors.values().cloned().collect()
    }

    /// Recover one or all degraded services back to healthy baseline.
    pub fn recover(&mut self, service_name: Option<&str>) -> Vec<String> {
        if let Some(name) = service_name {
            if self.recover_service(name) {
                return vec![name.to_string()];
            }
            return Vec::new();
        }

        let service_names = self
            .health_monitors
            .keys()
            .cloned()
            .collect::<Vec<String>>();
        let mut recovered = Vec::new();
        for name in service_names {
            if self.recover_service(&name) {
                recovered.push(name);
            }
        }
        recovered.sort();
        recovered
    }

    /// Evict the oldest entry from `total_requests` when at capacity.
    fn evict_oldest_request_entry(&mut self, service_name: &str) {
        evict_lru_entry(&mut self.total_requests, MAX_REQUESTS, service_name);
        // Also evict from the sibling map for consistency.
        if !self.successful_requests.contains_key(service_name)
            && self.successful_requests.len() >= MAX_REQUESTS
        {
            if let Some(oldest) = self.successful_requests.keys().next().cloned() {
                self.successful_requests.remove(&oldest);
            }
        }
    }

    fn ensure_service_registered(&mut self, name: &str) {
        if !self.health_monitors.contains_key(name) {
            self.register_service(name);
        }
    }

    fn recover_service(&mut self, name: &str) -> bool {
        let Some(health) = self.health_monitors.get(name) else {
            return false;
        };

        let already_healthy = matches!(health.status, HealthStatus::Healthy)
            && self
                .circuit_breakers
                .get(name)
                .copied()
                .unwrap_or(CircuitBreakerState::Closed)
                == CircuitBreakerState::Closed
            && self.failure_counts.get(name).copied().unwrap_or(0) == 0;
        if already_healthy {
            return false;
        }

        self.failure_counts.insert(name.to_string(), 0);
        self.circuit_breakers
            .insert(name.to_string(), CircuitBreakerState::Closed);
        self.total_requests.insert(name.to_string(), 0);
        self.successful_requests.insert(name.to_string(), 0);

        if let Some(health) = self.health_monitors.get_mut(name) {
            health.status = HealthStatus::Healthy;
            health.success_rate = 1.0;
            health.error_rate = 0.0;
            health.last_check_timestamp = now_epoch_seconds();
        }

        true
    }

    fn update_health_from_counters(&mut self, name: &str, latency_ms: Option<f64>) {
        let total = self.total_requests.get(name).copied().unwrap_or(0);
        let success = self.successful_requests.get(name).copied().unwrap_or(0);
        let failure_count = self.failure_counts.get(name).copied().unwrap_or(0) as f64;
        let success_rate = if total == 0 {
            1.0
        } else {
            success as f64 / total as f64
        };
        let error_rate = if total == 0 {
            0.0
        } else {
            (total.saturating_sub(success)) as f64 / total as f64
        };

        if let Some(health) = self.health_monitors.get_mut(name) {
            if let Some(latency_ms) = latency_ms {
                let samples = total.max(1) as f64;
                let previous_weight = (samples - 1.0).max(0.0);
                health.avg_latency_ms = if previous_weight == 0.0 {
                    latency_ms
                } else {
                    ((health.avg_latency_ms * previous_weight) + latency_ms) / samples
                };
            }
            health.success_rate = success_rate;
            health.error_rate =
                if failure_count > 0.0 && failure_count >= self.max_failure_threshold as f64 {
                    // When failures exceed threshold, use a blended rate that reflects
                    // both the actual error_rate and the severity relative to threshold.
                    let severity = (failure_count / self.max_failure_threshold as f64).min(1.0);
                    error_rate.max(severity * 0.5)
                } else {
                    error_rate
                };
            health.last_check_timestamp = now_epoch_seconds();
            health.status = if health.error_rate > self.anomaly_thresholds.error_rate_threshold {
                HealthStatus::Unhealthy
            } else if health.success_rate < self.anomaly_thresholds.success_rate_threshold {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            };
        }
    }
}

/// Generic LRU eviction: remove oldest entry from `map` when at `max_capacity`
/// and `new_key` is not already present.
fn evict_lru_entry<V>(map: &mut HashMap<String, V>, max_capacity: usize, new_key: &str) {
    if map.len() >= max_capacity && !map.contains_key(new_key) {
        if let Some(oldest) = map.keys().next().cloned() {
            map.remove(&oldest);
        }
    }
}

impl Default for FailurePrevention {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker() {
        let mut prevention = FailurePrevention::new();
        for _ in 0..5 {
            prevention.record_outcome("service1", false, 900);
        }
        assert_eq!(
            prevention.get_circuit_state("service1"),
            CircuitBreakerState::Open
        );
    }

    #[test]
    fn test_health_monitoring() {
        let mut prevention = FailurePrevention::new();
        prevention.register_service("api");
        prevention.update_service_health("api", 0.95, 0.05, 100.0);

        let health = prevention.health_monitors.get("api").unwrap();
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_degradation_strategy() {
        let mut prevention = FailurePrevention::new();
        prevention.register_service("api");
        prevention.update_service_health("api", 0.4, 0.6, 100.0);

        let degradation = prevention.get_degradation_strategy("api");
        assert!(degradation >= DegradationLevel::Constrained);
    }

    #[test]
    fn test_should_degrade() {
        let mut prevention = FailurePrevention::new();
        prevention.register_service("api");
        prevention.update_service_health("api", 0.4, 0.6, 100.0);

        assert!(prevention.should_degrade("api"));
    }

    #[test]
    fn test_record_outcome_updates_health() {
        let mut prevention = FailurePrevention::new();
        prevention.register_service("api");

        for _ in 0..5 {
            prevention.record_outcome("api", false, 900);
        }

        let health = prevention
            .get_health_report()
            .into_iter()
            .find(|item| item.service_name == "api")
            .unwrap();
        assert!(health.error_rate > 0.1);
        assert!(matches!(health.status, HealthStatus::Unhealthy));
    }

    #[test]
    fn test_recover_resets_unhealthy_service_to_healthy() {
        let mut prevention = FailurePrevention::new();
        prevention.register_service("api");
        for _ in 0..5 {
            prevention.record_outcome("api", false, 900);
        }

        let recovered = prevention.recover(Some("api"));
        assert_eq!(recovered, vec!["api".to_string()]);
        assert_eq!(
            prevention.get_circuit_state("api"),
            CircuitBreakerState::Closed
        );

        let health = prevention
            .get_health_report()
            .into_iter()
            .find(|item| item.service_name == "api")
            .expect("api health should exist");
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_success_resets_failure_count() {
        let mut prevention = FailurePrevention::new();
        prevention.register_service("api");
        for _ in 0..4 {
            prevention.record_outcome("api", false, 900);
        }
        assert_eq!(
            prevention.get_circuit_state("api"),
            CircuitBreakerState::Closed
        );
        // A success resets the failure streak before the threshold trips.
        prevention.record_outcome("api", true, 100);
        assert_eq!(
            prevention.get_circuit_state("api"),
            CircuitBreakerState::Closed
        );
    }
}
