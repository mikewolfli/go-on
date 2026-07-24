//! Recovery plan execution for the fault tolerance engine.
//!
//! Handles recovery plan creation, execution, consistency checking,
//! completion, failure handling, and node reintegration.

use anyhow::{anyhow, Result};

use crate::fault_tolerance::{
    now_millis, ConsistencyCheckEvent, FaultEvent, FaultToleranceEngine, FaultType, NodeStatus,
    RecoveryAction, RecoveryPlan, RecoveryState, MAX_RECOVERY_PLANS,
};

impl FaultToleranceEngine {
    /// Reintegrate a previously isolated node back into the cluster.
    pub async fn reintegrate_node(&self, node_id: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
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

        // Resolve all active faults for this node (they were recovered)
        let fault_ids: Vec<String> = inner
            .faults
            .values()
            .filter(|f| f.node_id == node_id && !f.recovered)
            .map(|f| f.id.clone())
            .collect();
        let now = now_millis();
        for fault_id in fault_ids {
            if let Some(event) = inner.faults.get_mut(&fault_id) {
                event.resolved_ms = Some(now);
                event.recovered = true;
            }
        }

        // Complete all active (Pending/InProgress) recovery plans for this node
        let active_plan_ids: Vec<String> = inner
            .recovery_plans
            .values()
            .filter(|p| {
                p.node_id == node_id
                    && (p.state == RecoveryState::Pending || p.state == RecoveryState::InProgress)
            })
            .map(|p| p.plan_id.clone())
            .collect();
        for plan_id in active_plan_ids {
            if let Some(plan) = inner.recovery_plans.get_mut(&plan_id) {
                plan.state = RecoveryState::Completed;
                plan.completed_ms = Some(now);
            }
        }

        Ok(())
    }

    /// Create a recovery plan for a failed node.
    /// Determines appropriate recovery actions based on fault type and severity.
    pub async fn create_recovery_plan(&self, node_id: &str) -> Result<String> {
        let mut inner = self.inner.write().await;
        let node_id = node_id.to_string();
        if !inner.heartbeats.contains_key(&node_id) {
            return Err(anyhow!("node '{}' is not registered", node_id));
        }

        // Collect active faults for this node
        let node_faults: Vec<&FaultEvent> = inner
            .faults
            .values()
            .filter(|f| f.node_id == node_id && !f.recovered)
            .collect();

        // Determine actions based on faults
        let mut actions = Vec::new();
        let max_severity = node_faults.iter().map(|f| f.severity).max().unwrap_or(0);

        for fault in &node_faults {
            match fault.fault_type {
                FaultType::Crash | FaultType::Oom => {
                    if !actions.contains(&RecoveryAction::RestartNode) {
                        actions.push(RecoveryAction::RestartNode);
                    }
                }
                FaultType::Hang | FaultType::ResourceExhaustion => {
                    if !actions.contains(&RecoveryAction::ScaleUp) {
                        actions.push(RecoveryAction::ScaleUp);
                    }
                }
                FaultType::NetworkSplit
                | FaultType::NetworkTimeout
                | FaultType::NetworkPartition => {
                    if !actions.contains(&RecoveryAction::FailoverToBackup) {
                        actions.push(RecoveryAction::FailoverToBackup);
                    }
                }
                FaultType::ProcessCrash => {
                    if !actions.contains(&RecoveryAction::RestartNode) {
                        actions.push(RecoveryAction::RestartNode);
                    }
                }
                FaultType::RateLimit | FaultType::LatencySpike { .. } => {
                    if !actions.contains(&RecoveryAction::ScaleUp) {
                        actions.push(RecoveryAction::ScaleUp);
                    }
                }
                FaultType::DataCorruption => {
                    if !actions.contains(&RecoveryAction::Rebalance) {
                        actions.push(RecoveryAction::Rebalance);
                    }
                }
                FaultType::FileIOError | FaultType::AuthFailure | FaultType::PartialWrite => {
                    if !actions.contains(&RecoveryAction::NotifyOperator) {
                        actions.push(RecoveryAction::NotifyOperator);
                    }
                }
            }
        }

        // Add operator notification for high severity
        if max_severity >= 9 {
            actions.push(RecoveryAction::NotifyOperator);
        }

        // If no specific actions, add a default
        if actions.is_empty() {
            actions.push(RecoveryAction::NotifyOperator);
        }

        inner.plan_counter += 1;
        let plan_id = format!("plan-{}", inner.plan_counter);
        let now = now_millis();
        let plan = RecoveryPlan {
            plan_id: plan_id.clone(),
            node_id: node_id.clone(),
            actions,
            state: RecoveryState::Pending,
            created_ms: now,
            completed_ms: None,
            result: None,
        };
        inner.recovery_plans.insert(plan_id.clone(), plan);

        // Evict oldest completed/failed plans when the map grows too large.
        if inner.recovery_plans.len() > MAX_RECOVERY_PLANS {
            let mut done: Vec<(String, u64)> = inner
                .recovery_plans
                .iter()
                .filter(|(_, p)| {
                    p.state == RecoveryState::Completed || p.state == RecoveryState::Failed
                })
                .map(|(id, p)| (id.clone(), p.created_ms))
                .collect();
            done.sort_unstable_by_key(|(_, ts)| *ts);
            let to_remove = inner
                .recovery_plans
                .len()
                .saturating_sub(MAX_RECOVERY_PLANS);
            for (id, _) in done.into_iter().take(to_remove) {
                inner.recovery_plans.remove(&id);
            }
        }

        Ok(plan_id)
    }

    /// Execute a recovery plan — transitions it to InProgress.
    pub async fn execute_recovery_plan(&self, plan_id: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
        let plan = inner
            .recovery_plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow!("recovery plan '{}' not found", plan_id))?;
        if plan.state != RecoveryState::Pending {
            return Err(anyhow!(
                "recovery plan '{}' is not in Pending state",
                plan_id
            ));
        }
        plan.state = RecoveryState::InProgress;
        Ok(())
    }

    /// Run a post-recovery consistency check and return the result.
    pub async fn post_recovery_consistency_check(&self, plan_id: &str) -> ConsistencyCheckEvent {
        let inner = self.inner.read().await;
        let plan = inner.recovery_plans.get(plan_id);

        let now = now_millis();
        let check_id = format!("cc-{}-{}", plan_id, now);

        let (passed, details) = if let Some(p) = plan {
            // Verify the plan's node still has a heartbeat record
            let has_heartbeat = inner.heartbeats.contains_key(&p.node_id);
            // Check that all active faults for this node have been resolved
            let unresolved_faults: Vec<&str> = inner
                .faults
                .values()
                .filter(|f| f.node_id == p.node_id && !f.recovered)
                .map(|f| f.id.as_str())
                .collect();
            // Plan should be in progress or completed (not pending or failed)
            let plan_active = matches!(
                p.state,
                RecoveryState::InProgress | RecoveryState::Completed
            );

            let all_ok = has_heartbeat && unresolved_faults.is_empty() && plan_active;
            let detail = if all_ok {
                format!(
                    "node '{}' heartbeat present, {} unresolved faults cleared, plan completed",
                    p.node_id,
                    unresolved_faults.len()
                )
            } else {
                let mut issues: Vec<String> = Vec::new();
                if !has_heartbeat {
                    issues.push("missing heartbeat record".to_string());
                }
                if !unresolved_faults.is_empty() {
                    issues.push(format!(
                        "{} unresolved faults remain",
                        unresolved_faults.len()
                    ));
                }
                if !plan_active {
                    issues.push("plan not active (pending or failed)".to_string());
                }
                format!("inconsistencies detected: {}", issues.join(", "))
            };
            (all_ok, detail)
        } else {
            (false, format!("recovery plan '{}' not found", plan_id))
        };

        ConsistencyCheckEvent {
            check_id,
            check_type: "post_recovery".to_string(),
            passed,
            details,
            timestamp_ms: now,
        }
    }

    /// Complete a recovery plan with a result. Runs a post-recovery
    /// consistency check and returns it alongside the completion outcome.
    pub async fn complete_recovery_plan(
        &self,
        plan_id: &str,
        result: &str,
    ) -> Result<ConsistencyCheckEvent> {
        let mut inner = self.inner.write().await;
        let plan = inner
            .recovery_plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow!("recovery plan '{}' not found", plan_id))?;
        if plan.state != RecoveryState::InProgress {
            return Err(anyhow!(
                "recovery plan '{}' is not in InProgress state",
                plan_id
            ));
        }
        let node_id_clone = plan.node_id.clone();
        plan.state = RecoveryState::Completed;
        plan.completed_ms = Some(now_millis());
        plan.result = Some(result.to_string());

        // Restore the node status if completing a recovery plan
        if let Some(record) = inner.heartbeats.get_mut(&node_id_clone) {
            record.status = NodeStatus::Recovering;
        }
        drop(inner);

        // Run post-recovery consistency check
        let check = self.post_recovery_consistency_check(plan_id).await;
        if !check.passed {
            tracing::warn!(
                "consistency check failed after recovery plan '{}': {}",
                plan_id,
                check.details
            );
        }
        Ok(check)
    }

    /// Fail a recovery plan.
    pub async fn fail_recovery_plan(&self, plan_id: &str, error: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
        let plan = inner
            .recovery_plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow!("recovery plan '{}' not found", plan_id))?;
        plan.state = RecoveryState::Failed;
        plan.completed_ms = Some(now_millis());
        plan.result = Some(format!("failed: {}", error));
        Ok(())
    }

    /// Get active recovery plans.
    pub async fn active_recovery_plans(&self) -> Vec<RecoveryPlan> {
        let inner = self.inner.read().await;
        inner
            .recovery_plans
            .values()
            .filter(|p| p.state == RecoveryState::Pending || p.state == RecoveryState::InProgress)
            .cloned()
            .collect()
    }
}
