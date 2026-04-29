//! Cross-node Fault Tolerance module — F-GAP-28
//!
//! Provides node-level fault isolation, heartbeat-based failure detection,
//! and automatic recovery coordination across a distributed cluster.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Core data types
// ---------------------------------------------------------------------------

/// Current status of a monitored node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Degraded,
    Offline,
    Recovering,
}

/// The type of fault detected on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaultType {
    Crash,
    Hang,
    OOM,
    NetworkSplit,
    DataCorruption,
    ResourceExhaustion,
}

/// A recorded fault event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultEvent {
    pub id: String,
    pub node_id: String,
    pub fault_type: FaultType,
    pub severity: u8,
    pub description: String,
    pub detected_ms: u64,
    pub resolved_ms: Option<u64>,
    pub recovered: bool,
}

/// The level of isolation applied to a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IsolationLevel {
    Monitor,
    Quarantine,
    Shutdown,
}

/// A group of isolated nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsolationGroup {
    pub group_id: String,
    pub nodes: Vec<String>,
    pub isolation_level: IsolationLevel,
    pub created_ms: u64,
}

/// Record of a single heartbeat received from a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRecord {
    pub node_id: String,
    pub last_heartbeat_ms: u64,
    pub missed_beats: u32,
    pub status: NodeStatus,
}

/// Configuration for the fault tolerance engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultToleranceConfig {
    pub heartbeat_timeout_ms: u64,
    pub max_missed_beats: u32,
    pub recovery_check_interval_ms: u64,
}

impl Default for FaultToleranceConfig {
    fn default() -> Self {
        Self {
            heartbeat_timeout_ms: 15000,
            max_missed_beats: 3,
            recovery_check_interval_ms: 5000,
        }
    }
}

/// A snapshot profile of the cluster's health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultToleranceProfile {
    pub total_nodes: usize,
    pub online_nodes: usize,
    pub degraded_nodes: usize,
    pub offline_nodes: usize,
    pub active_faults: usize,
    pub isolated_groups: usize,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct Inner {
    config: FaultToleranceConfig,
    /// node_id -> HeartbeatRecord
    heartbeats: HashMap<String, HeartbeatRecord>,
    /// fault_id -> FaultEvent
    faults: HashMap<String, FaultEvent>,
    /// group_id -> IsolationGroup
    isolation_groups: HashMap<String, IsolationGroup>,
    /// monotonic counter for generating unique fault ids
    fault_counter: u64,
    /// monotonic counter for generating unique group ids
    group_counter: u64,
}

// ---------------------------------------------------------------------------
// FaultToleranceEngine
// ---------------------------------------------------------------------------

/// Thread-safe engine that monitors node health, detects faults, and manages
/// isolation / recovery.
#[derive(Clone)]
pub struct FaultToleranceEngine {
    inner: Arc<Mutex<Inner>>,
}

impl FaultToleranceEngine {
    /// Create a new engine with the given configuration.
    pub fn new(config: FaultToleranceConfig) -> Self {
        let inner = Inner {
            config,
            heartbeats: HashMap::new(),
            faults: HashMap::new(),
            isolation_groups: HashMap::new(),
            fault_counter: 0,
            group_counter: 0,
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Register a node for heartbeat monitoring.
    pub fn register_node(&self, node_id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let node_id = node_id.to_string();
        if inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is already registered", node_id));
        }
        let now = now_millis();
        let record = HeartbeatRecord {
            node_id: node_id.clone(),
            last_heartbeat_ms: now,
            missed_beats: 0,
            status: NodeStatus::Online,
        };
        inner.heartbeats.insert(node_id, record);
        Ok(())
    }

    /// Unregister a node, removing it from monitoring entirely.
    pub fn unregister_node(&self, node_id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let node_id = node_id.to_string();
        if inner.heartbeats.remove(&node_id).is_none() {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }
        // Also clean up any active faults for this node
        inner.faults.retain(|_, f| f.node_id != node_id);
        Ok(())
    }

    /// Report a heartbeat from a node. Resets the missed-beat counter and
    /// moves the node back to Online if it was recovering.
    pub fn report_heartbeat(&self, node_id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let node_id = node_id.to_string();
        let record = inner
            .heartbeats
            .get_mut(&node_id)
            .ok_or_else(|| anyhow!("node '{}' is not registered", node_id))?;
        let now = now_millis();
        record.last_heartbeat_ms = now;
        record.missed_beats = 0;
        if record.status == NodeStatus::Offline || record.status == NodeStatus::Recovering {
            record.status = NodeStatus::Online;
        }
        Ok(())
    }

    /// Report a fault on a node. Returns the generated fault id.
    pub fn report_fault(
        &self,
        node_id: &str,
        fault_type: FaultType,
        severity: u8,
        description: &str,
    ) -> Result<String> {
        let mut inner = self.inner.lock().unwrap();
        let node_id = node_id.to_string();
        if !inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }
        let now = now_millis();
        inner.fault_counter += 1;
        let fault_id = format!("fault-{}", inner.fault_counter);
        let event = FaultEvent {
            id: fault_id.clone(),
            node_id: node_id.clone(),
            fault_type,
            severity,
            description: description.to_string(),
            detected_ms: now,
            resolved_ms: None,
            recovered: false,
        };
        inner.faults.insert(fault_id.clone(), event);

        // Mark the node as degraded or offline based on severity
        if let Some(record) = inner.heartbeats.get_mut(&node_id) {
            if severity >= 8 {
                record.status = NodeStatus::Offline;
            } else if severity >= 4 {
                record.status = NodeStatus::Degraded;
            }
        }

        Ok(fault_id)
    }

    /// Resolve an active fault by its id.
    pub fn resolve_fault(&self, fault_id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let fault_id = fault_id.to_string();
        let event = inner
            .faults
            .get_mut(&fault_id)
            .ok_or_else(|| anyhow!("fault '{}' not found", fault_id))?;
        if event.recovered {
            return Err(anyhow!("fault '{}' is already resolved", fault_id));
        }
        let now = now_millis();
        event.resolved_ms = Some(now);
        event.recovered = true;
        Ok(())
    }

    /// Isolate a node under a specific isolation level. Creates or updates
    /// an isolation group containing the node.
    pub fn isolate_node(&self, node_id: &str, level: IsolationLevel) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let node_id = node_id.to_string();
        if !inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }

        // Mark the node offline if shutdown level
        if level == IsolationLevel::Shutdown {
            if let Some(record) = inner.heartbeats.get_mut(&node_id) {
                record.status = NodeStatus::Offline;
            }
        } else if level == IsolationLevel::Quarantine {
            if let Some(record) = inner.heartbeats.get_mut(&node_id) {
                record.status = NodeStatus::Degraded;
            }
        }

        // Check if this node already belongs to a group
        for group in inner.isolation_groups.values_mut() {
            if group.nodes.contains(&node_id) {
                group.isolation_level = level.clone();
                return Ok(());
            }
        }

        // Create a new isolation group
        inner.group_counter += 1;
        let group_id = format!("group-{}", inner.group_counter);
        let now = now_millis();
        let group = IsolationGroup {
            group_id: group_id.clone(),
            nodes: vec![node_id],
            isolation_level: level,
            created_ms: now,
        };
        inner.isolation_groups.insert(group_id, group);
        Ok(())
    }

    /// Reintegrate a previously isolated node back into the cluster.
    pub fn reintegrate_node(&self, node_id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let node_id = node_id.to_string();
        if !inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }

        // Remove node from all isolation groups
        let groups_to_remove: Vec<String> = inner
            .isolation_groups
            .iter()
            .filter(|(_, g)| g.nodes.contains(&node_id))
            .map(|(id, _)| id.clone())
            .collect();

        for group_id in groups_to_remove {
            let mut empty = false;
            if let Some(group) = inner.isolation_groups.get_mut(&group_id) {
                group.nodes.retain(|n| n != &node_id);
                empty = group.nodes.is_empty();
            }
            if empty {
                inner.isolation_groups.remove(&group_id);
            }
        }

        // Restore node to online
        if let Some(record) = inner.heartbeats.get_mut(&node_id) {
            record.status = NodeStatus::Online;
            record.missed_beats = 0;
        }

        Ok(())
    }

    /// Check all heartbeats and return a list of node ids that have missed
    /// too many heartbeats (exceeded max_missed_beats).
    pub fn check_heartbeats(&self) -> Vec<String> {
        let mut inner = self.inner.lock().unwrap();
        let now = now_millis();
        let timeout = inner.config.heartbeat_timeout_ms;
        let max_missed = inner.config.max_missed_beats;

        let mut offenders = Vec::new();

        let node_ids: Vec<String> = inner.heartbeats.keys().cloned().collect();
        for node_id in node_ids {
            if let Some(record) = inner.heartbeats.get_mut(&node_id) {
                let elapsed = now.saturating_sub(record.last_heartbeat_ms);
                if elapsed >= timeout {
                    record.missed_beats = record.missed_beats.saturating_add(1);
                } else {
                    // Node is responsive; reset miss counter if not offline
                    if record.status != NodeStatus::Offline {
                        record.missed_beats = 0;
                    }
                }

                // Update status based on missed beats
                if record.missed_beats >= max_missed {
                    record.status = NodeStatus::Offline;
                    offenders.push(node_id.clone());
                } else if record.missed_beats > 0 {
                    record.status = NodeStatus::Degraded;
                } else if record.status != NodeStatus::Recovering {
                    record.status = NodeStatus::Online;
                }
            }
        }

        offenders
    }

    /// Return all active (unresolved) faults.
    pub fn active_faults(&self) -> Vec<FaultEvent> {
        let inner = self.inner.lock().unwrap();
        inner
            .faults
            .values()
            .filter(|f| !f.recovered)
            .cloned()
            .collect()
    }

    /// Return a snapshot profile of the cluster state.
    pub fn profile(&self) -> FaultToleranceProfile {
        let inner = self.inner.lock().unwrap();
        let total_nodes = inner.heartbeats.len();
        let online_nodes = inner
            .heartbeats
            .values()
            .filter(|r| r.status == NodeStatus::Online)
            .count();
        let degraded_nodes = inner
            .heartbeats
            .values()
            .filter(|r| r.status == NodeStatus::Degraded)
            .count();
        let offline_nodes = inner
            .heartbeats
            .values()
            .filter(|r| r.status == NodeStatus::Offline)
            .count();
        // Recovering nodes are counted separately; they are none of the above
        let active_faults = inner.faults.values().filter(|f| !f.recovered).count();
        let isolated_groups = inner.isolation_groups.len();

        FaultToleranceProfile {
            total_nodes,
            online_nodes,
            degraded_nodes,
            offline_nodes,
            active_faults,
            isolated_groups,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> FaultToleranceConfig {
        FaultToleranceConfig {
            heartbeat_timeout_ms: 100, // 100 ms for fast test
            max_missed_beats: 3,
            recovery_check_interval_ms: 50,
        }
    }

    #[test]
    fn test_new_engine_empty() {
        let config = make_config();
        let engine = FaultToleranceEngine::new(config);
        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 0);
        assert_eq!(profile.online_nodes, 0);
        assert_eq!(profile.degraded_nodes, 0);
        assert_eq!(profile.offline_nodes, 0);
        assert_eq!(profile.active_faults, 0);
        assert_eq!(profile.isolated_groups, 0);
    }

    #[test]
    fn test_register_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 1);
        assert_eq!(profile.online_nodes, 1);
    }

    #[test]
    fn test_register_duplicate_node_fails() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        let result = engine.register_node("node-1");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already registered"));
    }

    #[test]
    fn test_unregister_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        engine.unregister_node("node-1").unwrap();
        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 0);
        // unregistering an unknown node should fail
        let result = engine.unregister_node("node-1");
        assert!(result.is_err());
    }

    #[test]
    fn test_report_heartbeat() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        // report a heartbeat (should succeed)
        engine.report_heartbeat("node-1").unwrap();
        // reporting heartbeat for unknown node should fail
        let result = engine.report_heartbeat("node-unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_missed_heartbeat_detection() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        // Immediately after registration, no missed beats
        let offenders = engine.check_heartbeats();
        assert!(offenders.is_empty());

        // Wait longer than the heartbeat timeout (100ms)
        std::thread::sleep(std::time::Duration::from_millis(150));

        // First check: one missed -> degraded
        let offenders = engine.check_heartbeats();
        // missed_beats == 1, < max_missed (3), so not an offender yet
        assert!(offenders.is_empty());

        // Wait again and check multiple times to exceed max_missed_beats
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(110));
            engine.check_heartbeats();
        }

        let offenders = engine.check_heartbeats();
        assert!(
            offenders.contains(&"node-1".to_string()),
            "node-1 should be marked as offender after many missed beats, got: {:?}",
            offenders
        );
    }

    #[test]
    fn test_report_fault() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        let fault_id = engine
            .report_fault("node-1", FaultType::Crash, 7, "Node crashed unexpectedly")
            .unwrap();
        assert!(fault_id.starts_with("fault-"));
        // Verify fault is active
        let active = engine.active_faults();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, fault_id);
        assert_eq!(active[0].node_id, "node-1");
        assert_eq!(active[0].fault_type, FaultType::Crash);
        assert!(!active[0].recovered);

        // Reporting fault on unknown node should fail
        let result = engine.report_fault("unknown", FaultType::Crash, 5, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_fault() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        let fault_id = engine
            .report_fault("node-1", FaultType::OOM, 9, "Out of memory")
            .unwrap();

        // Resolve the fault
        engine.resolve_fault(&fault_id).unwrap();
        let active = engine.active_faults();
        assert_eq!(active.len(), 0);

        // Resolving again should fail
        let result = engine.resolve_fault(&fault_id);
        assert!(result.is_err());

        // Resolving unknown fault should fail
        let result = engine.resolve_fault("does-not-exist");
        assert!(result.is_err());
    }

    #[test]
    fn test_isolate_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();

        // Isolate at Monitor level
        engine
            .isolate_node("node-1", IsolationLevel::Monitor)
            .unwrap();
        let profile = engine.profile();
        assert_eq!(profile.isolated_groups, 1);

        // Isolate again at a different level (should update existing group)
        engine
            .isolate_node("node-1", IsolationLevel::Quarantine)
            .unwrap();
        let profile = engine.profile();
        assert_eq!(profile.isolated_groups, 1);
        assert_eq!(profile.degraded_nodes, 1);

        // Isolate unknown node
        let result = engine.isolate_node("unknown", IsolationLevel::Shutdown);
        assert!(result.is_err());
    }

    #[test]
    fn test_reintegrate_node() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        engine
            .isolate_node("node-1", IsolationLevel::Quarantine)
            .unwrap();

        // Reintegrate
        engine.reintegrate_node("node-1").unwrap();
        let profile = engine.profile();
        assert_eq!(profile.isolated_groups, 0);
        assert_eq!(profile.online_nodes, 1);

        // Reintegrate unknown node should fail
        let result = engine.reintegrate_node("unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_active_faults() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        engine.register_node("node-2").unwrap();

        let id1 = engine
            .report_fault("node-1", FaultType::Crash, 8, "crash")
            .unwrap();
        let id2 = engine
            .report_fault("node-2", FaultType::Hang, 5, "hang")
            .unwrap();

        let active = engine.active_faults();
        assert_eq!(active.len(), 2);

        // Resolve one
        engine.resolve_fault(&id1).unwrap();
        let active = engine.active_faults();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id2);
    }

    #[test]
    fn test_profile_reflects_state() {
        let engine = FaultToleranceEngine::new(make_config());
        engine.register_node("node-1").unwrap();
        engine.register_node("node-2").unwrap();
        engine.register_node("node-3").unwrap();

        engine
            .report_fault("node-1", FaultType::NetworkSplit, 9, "split")
            .unwrap();
        engine
            .report_fault("node-2", FaultType::ResourceExhaustion, 5, "exhausted")
            .unwrap();

        // node-1 severity 9 -> offline
        // node-2 severity 5 -> degraded
        // node-3 -> online

        let profile = engine.profile();
        assert_eq!(profile.total_nodes, 3);
        assert_eq!(profile.online_nodes, 1);
        assert_eq!(profile.degraded_nodes, 1);
        assert_eq!(profile.offline_nodes, 1);
        assert_eq!(profile.active_faults, 2);
        assert_eq!(profile.isolated_groups, 0);
    }
}
