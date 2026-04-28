//! ProtocolBus — Protocol-aware routing information sub-bus (BLUE38 §1, ARCH-13)
//!
//! ProtocolBus provides protocol-aware routing information to CapabilityBus.
//! It tracks active transport mode, protocol health, and latency statistics
//! to enable intelligent protocol recommendations for task execution.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// Number of latency measurements kept per protocol in the ring buffer.
const LATENCY_RING_BUFFER_CAPACITY: usize = 100;

/// Latency statistics for a single protocol, backed by a ring buffer.
#[derive(Debug, Clone)]
pub struct LatencyStats {
    /// Ring buffer of recent latency measurements (milliseconds).
    measurements: Vec<u64>,
    /// Next write index in the ring buffer.
    cursor: usize,
    /// Number of measurements inserted so far.
    count: u64,
}

impl LatencyStats {
    fn new() -> Self {
        Self {
            measurements: Vec::with_capacity(LATENCY_RING_BUFFER_CAPACITY),
            cursor: 0,
            count: 0,
        }
    }

    /// Record a single latency measurement into the ring buffer.
    fn record(&mut self, duration_ms: u64) {
        if self.measurements.len() < LATENCY_RING_BUFFER_CAPACITY {
            self.measurements.push(duration_ms);
        } else {
            self.measurements[self.cursor] = duration_ms;
        }
        self.cursor = (self.cursor + 1) % LATENCY_RING_BUFFER_CAPACITY;
        self.count += 1;
    }

    /// Average latency over all recorded measurements, or `u64::MAX` if none.
    fn average_ms(&self) -> u64 {
        if self.count == 0 {
            return u64::MAX;
        }
        // Only consider the portion of the ring buffer that has been written.
        let n = self.count.min(LATENCY_RING_BUFFER_CAPACITY as u64) as usize;
        let valid = self.measurements.iter().take(n);
        let sum: u64 = valid.sum();
        sum / n as u64
    }
}

/// Profile snapshot for the ProtocolBus.
#[derive(Debug, Clone)]
pub struct ProtocolBusProfile {
    /// Whether the protocol bus is enabled.
    pub enabled: bool,
    /// The currently active transport mode.
    pub active_transport: String,
    /// Number of protocols currently reporting healthy.
    pub healthy_protocols: u32,
    /// Total number of protocol switches performed.
    pub total_protocol_switches: u64,
}

/// Recommendation for which protocol to use for a given task.
#[derive(Debug, Clone)]
pub struct ProtocolRecommendation {
    /// The preferred protocol identifier (e.g. "acp-stdio", "acp-http", "mcp-stdio").
    pub preferred_protocol: String,
    /// Human-readable justification for the recommendation.
    pub reason: String,
    /// Confidence score between 0.0 and 1.0.
    pub confidence: f64,
}

/// ProtocolBus provides protocol-aware routing information to CapabilityBus.
///
/// This sub-bus tracks:
/// - Current active transport mode
/// - Per-protocol health status
/// - Per-protocol latency statistics via ring buffer
/// - Cumulative profile metrics
pub struct ProtocolBus {
    /// Current active transport mode.
    active_transport: Arc<RwLock<String>>,
    /// Protocol health status (protocol -> healthy).
    protocol_health: Arc<RwLock<HashMap<String, bool>>>,
    /// Protocol latency stats.
    protocol_latency: Arc<RwLock<HashMap<String, LatencyStats>>>,
    /// Profile metrics.
    profile: Arc<Mutex<ProtocolBusProfile>>,
}

impl ProtocolBus {
    /// Create a new `ProtocolBus` with default values.
    ///
    /// The default active transport is `"auto"`. Five known protocols are
    /// initialised with healthy status: `acp-stdio`, `acp-http`, `mcp-stdio`,
    /// `mcp-http`, and `auto`.
    pub fn new() -> Self {
        let health: HashMap<String, bool> = [
            ("acp-stdio".to_string(), true),
            ("acp-http".to_string(), true),
            ("mcp-stdio".to_string(), true),
            ("mcp-http".to_string(), true),
            ("auto".to_string(), true),
        ]
        .into();

        let latency: HashMap<String, LatencyStats> = [
            ("acp-stdio".to_string(), LatencyStats::new()),
            ("acp-http".to_string(), LatencyStats::new()),
            ("mcp-stdio".to_string(), LatencyStats::new()),
            ("mcp-http".to_string(), LatencyStats::new()),
            ("auto".to_string(), LatencyStats::new()),
        ]
        .into();

        Self {
            active_transport: Arc::new(RwLock::new("auto".to_string())),
            protocol_health: Arc::new(RwLock::new(health)),
            protocol_latency: Arc::new(RwLock::new(latency)),
            profile: Arc::new(Mutex::new(ProtocolBusProfile {
                enabled: true,
                active_transport: "auto".to_string(),
                healthy_protocols: 5,
                total_protocol_switches: 0,
            })),
        }
    }

    /// Set the currently active transport mode.
    ///
    /// Supported transports: `acp-stdio`, `acp-http`, `mcp-stdio`, `mcp-http`, `auto`.
    /// If the transport differs from the current one, the protocol switch counter
    /// is incremented.
    pub fn set_active_transport(&self, transport: &str) {
        {
            let mut current = self
                .active_transport
                .write()
                .expect("active_transport lock poisoned");
            if *current != transport {
                *current = transport.to_string();
            } else {
                // No change; nothing to update.
                return;
            }
        }

        // Update profile outside of the transport lock to avoid nested locking.
        let mut profile = self.profile.lock().expect("profile lock poisoned");
        profile.active_transport = transport.to_string();
        profile.total_protocol_switches += 1;
    }

    /// Return the currently active transport mode.
    pub fn active_transport(&self) -> String {
        self.active_transport
            .read()
            .expect("active_transport lock poisoned")
            .clone()
    }

    /// Recommend the best protocol for a task of the given type and payload size.
    ///
    /// The recommendation logic considers:
    /// - Current active transport (highest priority)
    /// - Protocol health (unhealthy protocols are never recommended)
    /// - Average latency (lower is better)
    ///
    /// Returns a `ProtocolRecommendation` with the preferred protocol, a
    /// human-readable reason, and a confidence score.
    pub fn recommend_protocol(&self, task_type: &str, payload_size: u64) -> ProtocolRecommendation {
        let transport = self
            .active_transport
            .read()
            .expect("active_transport lock poisoned")
            .clone();

        let health = self
            .protocol_health
            .read()
            .expect("protocol_health lock poisoned")
            .clone();

        let latency = self
            .protocol_latency
            .read()
            .expect("protocol_latency lock poisoned")
            .clone();

        // 1. If the active transport is healthy, prefer it.
        if health.get(&transport).copied().unwrap_or(false) {
            let avg_latency = latency
                .get(&transport)
                .map(LatencyStats::average_ms)
                .unwrap_or(0);

            let confidence = compute_confidence(avg_latency, payload_size, true);
            let reason = format!(
                "Active transport '{}' is healthy (avg latency {} ms, payload {} bytes, task '{}')",
                transport, avg_latency, payload_size, task_type
            );
            return ProtocolRecommendation {
                preferred_protocol: transport,
                reason,
                confidence,
            };
        }

        // 2. Fall back to the healthiest / lowest-latency protocol.
        let mut candidates: Vec<(&String, bool, u64)> = health
            .iter()
            .filter(|(_, healthy)| **healthy)
            .map(|(proto, _)| {
                let avg = latency
                    .get(proto)
                    .map(LatencyStats::average_ms)
                    .unwrap_or(u64::MAX);
                (proto, true, avg)
            })
            .collect();

        if candidates.is_empty() {
            // No healthy protocol available — return a low-confidence recommendation.
            return ProtocolRecommendation {
                preferred_protocol: "auto".to_string(),
                reason: "No healthy protocol available; defaulting to 'auto'".to_string(),
                confidence: 0.1,
            };
        }

        // Sort by average latency ascending, then pick the first.
        candidates.sort_by_key(|(_, _, avg)| *avg);
        let (best_proto, _, best_avg) = candidates[0].clone();

        let confidence = compute_confidence(best_avg, payload_size, false);
        let reason = format!(
            "Recommended '{}' (avg latency {} ms, payload {} bytes, task '{}')",
            best_proto, best_avg, payload_size, task_type
        );
        ProtocolRecommendation {
            preferred_protocol: best_proto.clone(),
            reason,
            confidence,
        }
    }

    /// Record a latency measurement for the given protocol.
    ///
    /// The measurement is stored in a ring buffer (last `LATENCY_RING_BUFFER_CAPACITY`
    /// entries per protocol). If the protocol is not yet tracked, a new entry is
    /// created automatically.
    pub fn record_protocol_latency(&self, protocol: &str, duration_ms: u64) {
        let mut latency = self
            .protocol_latency
            .write()
            .expect("protocol_latency lock poisoned");
        let stats = latency
            .entry(protocol.to_string())
            .or_insert_with(LatencyStats::new);
        stats.record(duration_ms);

        // If the protocol is not already in the health map, add it as healthy.
        let mut health = self
            .protocol_health
            .write()
            .expect("protocol_health lock poisoned");
        health.entry(protocol.to_string()).or_insert(true);
    }

    /// Check whether a protocol is currently considered healthy.
    ///
    /// Unknown protocols are assumed healthy by default.
    pub fn is_protocol_healthy(&self, protocol: &str) -> bool {
        self.protocol_health
            .read()
            .expect("protocol_health lock poisoned")
            .get(protocol)
            .copied()
            .unwrap_or(true)
    }

    /// Return a snapshot of the current `ProtocolBusProfile`.
    pub fn profile(&self) -> ProtocolBusProfile {
        let transport = self.active_transport();
        let health = self
            .protocol_health
            .read()
            .expect("protocol_health lock poisoned");
        let healthy_count = health.values().filter(|h| **h).count() as u32;

        let profile_guard = self.profile.lock().expect("profile lock poisoned");
        ProtocolBusProfile {
            enabled: profile_guard.enabled,
            active_transport: transport,
            healthy_protocols: healthy_count,
            total_protocol_switches: profile_guard.total_protocol_switches,
        }
    }
}

impl Default for ProtocolBus {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Compute a confidence score based on latency, payload size, and whether the
/// active transport is being used directly.
///
/// Returns a value in the range [0.1, 1.0].
fn compute_confidence(avg_latency_ms: u64, _payload_size: u64, is_active: bool) -> f64 {
    let mut confidence = 1.0_f64;

    // Penalise high latency.
    if avg_latency_ms > 0 {
        // Scale: at 5000 ms confidence drops to ~0.5; anything above that is poor.
        let latency_penalty = (avg_latency_ms as f64 / 5000.0).min(1.0);
        confidence -= latency_penalty * 0.4;
    }

    // Boost for the active transport.
    if is_active {
        confidence += 0.1;
    }

    confidence.clamp(0.1, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let bus = ProtocolBus::new();
        assert_eq!(bus.active_transport(), "auto");
        assert!(bus.is_protocol_healthy("auto"));
        assert!(bus.is_protocol_healthy("acp-stdio"));
        assert!(bus.is_protocol_healthy("unknown-protocol"));
    }

    #[test]
    fn test_set_active_transport() {
        let bus = ProtocolBus::new();
        bus.set_active_transport("acp-http");
        assert_eq!(bus.active_transport(), "acp-http");

        let profile = bus.profile();
        assert_eq!(profile.total_protocol_switches, 1);
        assert_eq!(profile.active_transport, "acp-http");
    }

    #[test]
    fn test_set_active_transport_same_value_no_switch() {
        let bus = ProtocolBus::new();
        bus.set_active_transport("auto");
        let profile = bus.profile();
        // Default is already "auto", so no switch should be counted.
        assert_eq!(profile.total_protocol_switches, 0);
    }

    #[test]
    fn test_record_latency_and_stats() {
        let bus = ProtocolBus::new();
        bus.record_protocol_latency("acp-stdio", 10);
        bus.record_protocol_latency("acp-stdio", 20);
        bus.record_protocol_latency("acp-stdio", 30);

        let latency = bus.protocol_latency.read().unwrap();
        let stats = latency.get("acp-stdio").unwrap();
        assert_eq!(stats.average_ms(), 20); // (10 + 20 + 30) / 3
        assert_eq!(stats.count, 3);
    }

    #[test]
    fn test_recommend_protocol_prefers_active_transport() {
        let bus = ProtocolBus::new();
        // Active transport is "auto" and healthy.
        let rec = bus.recommend_protocol("chat", 1024);
        assert_eq!(rec.preferred_protocol, "auto");
        assert!(rec.confidence > 0.5);
    }

    #[test]
    fn test_recommend_protocol_falls_back_when_active_unhealthy() {
        let bus = ProtocolBus::new();

        // Mark the active transport unhealthy.
        {
            let mut health = bus.protocol_health.write().unwrap();
            health.insert("auto".to_string(), false);
        }
        // Record some latency for another protocol so it gets preferred.
        bus.record_protocol_latency("acp-stdio", 5);

        let rec = bus.recommend_protocol("code-review", 4096);
        // Should now recommend "acp-stdio" because "auto" is unhealthy and
        // "acp-stdio" has low latency.
        assert_eq!(rec.preferred_protocol, "acp-stdio");
    }

    #[test]
    fn test_recommend_protocol_no_healthy_protocols() {
        let bus = ProtocolBus::new();
        {
            let mut health = bus.protocol_health.write().unwrap();
            for (_, v) in health.iter_mut() {
                *v = false;
            }
        }
        let rec = bus.recommend_protocol("chat", 256);
        assert_eq!(rec.preferred_protocol, "auto");
        assert!(rec.confidence < 0.5);
    }

    #[test]
    fn test_latency_ring_buffer_wraps() {
        let bus = ProtocolBus::new();
        // Fill the buffer beyond capacity.
        let n = LATENCY_RING_BUFFER_CAPACITY + 10;
        for i in 0..n {
            bus.record_protocol_latency("acp-http", i as u64);
        }

        let latency = bus.protocol_latency.read().unwrap();
        let stats = latency.get("acp-http").unwrap();
        // The ring buffer should still hold exactly CAPACITY items.
        assert_eq!(stats.measurements.len(), LATENCY_RING_BUFFER_CAPACITY);
        // Count should reflect all inserts.
        assert_eq!(stats.count, n as u64);
        // The first overwritten element should no longer be present.
        // After 110 inserts into a 100-slot buffer, the first 10 values are gone.
        // The oldest remaining value is 10, newest is 109.
        let min_val = *stats.measurements.iter().min().unwrap();
        assert_eq!(min_val, 10);
    }

    #[test]
    fn test_is_protocol_healthy_unknown() {
        let bus = ProtocolBus::new();
        // Unknown protocols are treated as healthy.
        assert!(bus.is_protocol_healthy("nonexistent-protocol"));
    }

    #[test]
    fn test_profile_snapshot() {
        let bus = ProtocolBus::new();
        bus.set_active_transport("mcp-stdio");
        bus.record_protocol_latency("mcp-stdio", 15);

        let profile = bus.profile();
        assert!(profile.enabled);
        assert_eq!(profile.active_transport, "mcp-stdio");
        assert_eq!(profile.healthy_protocols, 5);
        assert_eq!(profile.total_protocol_switches, 1);
    }

    #[test]
    fn test_default_trait() {
        let bus = ProtocolBus::default();
        assert_eq!(bus.active_transport(), "auto");
    }
}
