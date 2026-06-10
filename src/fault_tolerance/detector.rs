//! Failure detection logic for the fault tolerance engine.
//!
//! Handles node registration, heartbeat tracking, fault reporting,
//! and isolation management.

use anyhow::{anyhow, Result};

use crate::fault_tolerance::{
    cluster_health_from_counts, now_millis, read_guard, write_guard, ClusterHealth,
    EscalationLevel, FaultEvent, FaultToleranceEngine, FaultType, IsolationLevel, NodeStatus,
    RecoveryState, MAX_FAULTS, MAX_GROUPS, MAX_HEARTBEATS,
};

impl FaultToleranceEngine {
    /// Register a node for heartbeat monitoring.
    pub fn register_node(&self, node_id: &str) -> Result<()> {
        let mut inner = write_guard(&self.inner);
        let node_id = node_id.to_string();
        if inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is already registered", node_id));
        }
        let now = now_millis();
        let record = crate::fault_tolerance::HeartbeatRecord {
            node_id: node_id.clone(),
            last_heartbeat_ms: now,
            missed_beats: 0,
            status: NodeStatus::Online,
        };
        inner.heartbeats.insert(node_id.clone(), record);

        // Evict node with the oldest heartbeat when at capacity.
        if inner.heartbeats.len() > MAX_HEARTBEATS {
            let oldest_id = inner
                .heartbeats
                .iter()
                .min_by_key(|(_, r)| r.last_heartbeat_ms)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest_id {
                inner.heartbeats.remove(&id);
            }
        }

        Ok(())
    }

    /// Unregister a node, removing it from monitoring entirely.
    pub fn unregister_node(&self, node_id: &str) -> Result<()> {
        let mut inner = write_guard(&self.inner);
        let node_id = node_id.to_string();
        if inner.heartbeats.remove(&node_id).is_none() {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }
        // Also clean up any active faults for this node
        inner.faults.retain(|_, f| f.node_id != node_id);
        // Clean up isolation groups that reference this node
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
        // Clean up recovery plans for this node
        inner.recovery_plans.retain(|_, p| p.node_id != node_id);
        Ok(())
    }

    /// Report a heartbeat from a node. Resets the missed-beat counter and
    /// moves the node back to Online if it was recovering.
    pub fn report_heartbeat(&self, node_id: &str) -> Result<()> {
        let mut inner = write_guard(&self.inner);
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
        let mut inner = write_guard(&self.inner);
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

        // Evict oldest resolved faults when the map grows too large.
        if inner.faults.len() > MAX_FAULTS {
            let mut resolved: Vec<(String, u64)> = inner
                .faults
                .iter()
                .filter(|(_, f)| f.recovered)
                .map(|(id, f)| (id.clone(), f.detected_ms))
                .collect();
            resolved.sort_unstable_by_key(|(_, ts)| *ts);
            let to_remove = inner.faults.len().saturating_sub(MAX_FAULTS);
            for (id, _) in resolved.into_iter().take(to_remove) {
                inner.faults.remove(&id);
            }
        }

        // Mark the node as degraded or offline based on severity
        // IMPORTANT: Only escalate status (Online→Degraded→Offline), never downgrade.
        // A node that is already Offline should not become Degraded from a lower-severity fault.
        if let Some(record) = inner.heartbeats.get_mut(&node_id) {
            if severity >= 8 && record.status != NodeStatus::Offline {
                record.status = NodeStatus::Offline;
            } else if severity >= 4 && record.status == NodeStatus::Online {
                record.status = NodeStatus::Degraded;
            }
        }

        Ok(fault_id)
    }

    /// Resolve an active fault by its id.
    pub fn resolve_fault(&self, fault_id: &str) -> Result<()> {
        let mut inner = write_guard(&self.inner);
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
        let mut inner = write_guard(&self.inner);
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
        let group = crate::fault_tolerance::IsolationGroup {
            group_id: group_id.clone(),
            nodes: vec![node_id],
            isolation_level: level,
            created_ms: now,
        };
        inner.isolation_groups.insert(group_id.clone(), group);

        // Evict oldest isolation group when at capacity.
        if inner.isolation_groups.len() > MAX_GROUPS {
            let oldest_id = inner
                .isolation_groups
                .iter()
                .min_by_key(|(_, g)| g.created_ms)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest_id {
                inner.isolation_groups.remove(&id);
            }
        }

        Ok(())
    }

    /// Check all heartbeats and return a list of node ids that have missed
    /// too many heartbeats (exceeded max_missed_beats).
    pub fn check_heartbeats(&self) -> Vec<String> {
        let mut inner = write_guard(&self.inner);
        let now = now_millis();
        let timeout = inner.config.heartbeat_timeout_ms;
        let max_missed = inner.config.max_missed_beats;

        let mut offenders = Vec::new();

        let node_ids: Vec<String> = inner.heartbeats.keys().cloned().collect();
        for node_id in node_ids {
            if let Some(record) = inner.heartbeats.get_mut(&node_id) {
                let elapsed = now.saturating_sub(record.last_heartbeat_ms);
                if elapsed >= timeout {
                    record.missed_beats = record.missed_beats.saturating_add(1).min(max_missed);
                } else {
                    // Node is responsive; reset miss counter
                    record.missed_beats = 0;
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
        let inner = read_guard(&self.inner);
        inner
            .faults
            .values()
            .filter(|f| !f.recovered)
            .cloned()
            .collect()
    }

    /// Assess the escalation level for a given node.
    pub fn escalation_level(&self, node_id: &str) -> EscalationLevel {
        let inner = read_guard(&self.inner);
        let node_id = node_id.to_string();
        let record = match inner.heartbeats.get(&node_id) {
            Some(r) => r,
            None => return EscalationLevel::Manual,
        };

        let active_node_faults: Vec<&FaultEvent> = inner
            .faults
            .values()
            .filter(|f| f.node_id == node_id && !f.recovered)
            .collect();

        if active_node_faults.is_empty() {
            return EscalationLevel::Auto;
        }

        let max_severity = active_node_faults
            .iter()
            .map(|f| f.severity)
            .max()
            .unwrap_or(0);
        let ongoing_recovery = inner
            .recovery_plans
            .values()
            .any(|p| p.node_id == node_id && p.state == RecoveryState::InProgress);

        match (record.status.clone(), max_severity, ongoing_recovery) {
            (NodeStatus::Online, _, _) => EscalationLevel::Auto,
            (NodeStatus::Degraded, s, _) if s < 7 => EscalationLevel::Auto,
            (NodeStatus::Degraded, _, _) => EscalationLevel::Coordinated,
            (NodeStatus::Offline, s, _) if s >= 9 => EscalationLevel::Manual,
            (NodeStatus::Offline, _, true) => EscalationLevel::Coordinated,
            (NodeStatus::Offline, _, _) => EscalationLevel::Coordinated,
            (NodeStatus::Recovering, _, _) => EscalationLevel::Coordinated,
        }
    }

    /// Get the overall cluster health.
    pub fn cluster_health(&self) -> ClusterHealth {
        let p = self.profile();
        if p.total_nodes == 0 {
            return ClusterHealth::Down;
        }
        cluster_health_from_counts(
            p.total_nodes,
            p.offline_nodes,
            p.degraded_nodes,
            p.active_faults,
        )
    }
}
