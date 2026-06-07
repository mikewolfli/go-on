//! Discovery subsystem — pattern discovery and knowledge abstraction
//!
//! Extracted from `core.rs` to isolate DiscoveryCenter integration
//! within the evolve pipeline.

use super::core::CapabilityBus;
use tracing::warn;

impl CapabilityBus {
    /// Record successful patterns in DiscoveryCenter.
    pub(crate) fn evolve_discovery(
        &self,
        state: &(String, String),
        action: &str,
        reward: f64,
        quality_score: f64,
        success: bool,
        now: u64,
    ) {
        if success && quality_score > 0.7 {
            if let Err(e) = self.discovery.record_solution(
                crate::intelligence::discovery::DiscoveryEntry {
                    id: String::new(),
                    problem_pattern: format!("state_{}", state.0),
                    solution_summary: format!("action_{}", action),
                    solution_detail: serde_json::json!({"reward": reward, "quality": quality_score}),
                    applicability_tags: vec![state.0.clone(), state.1.clone()],
                    success_rate: quality_score,
                    total_attempts: 1,
                    successful_attempts: if success { 1 } else { 0 },
                    discovered_by: "capability_bus_evolve".to_string(),
                    created_ms: now,
                    last_used_ms: now,
                }
            ) {
                warn!("evolve: discovery.record_solution failed: {}", e);
            }
        }
    }
}
