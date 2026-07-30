use super::*;

// ---------------------------------------------------------------------------
// Breaker Status
// ---------------------------------------------------------------------------

pub(super) async fn breaker_status_payload(server: &AcpServer) -> Result<Value> {
    let breakers = server
        .resilience
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();
    let open_count = breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count();
    let degraded_services = collect_degraded_services(server);
    Ok(json!({
        "ok": true,
        "open_count": open_count,
        "degraded_count": degraded_services.len(),
        "degraded_services": degraded_services,
        "breakers": breakers,
    }))
}

// ---------------------------------------------------------------------------
// Breaker Reset
// ---------------------------------------------------------------------------

pub(super) async fn breaker_reset_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let target = params
        .get("agent")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str);
    let reset_count = server
        .resilience
        .circuit_breakers
        .lock()
        .map(|mut guard| guard.reset(target))
        .unwrap_or(0);
    let breakers = server
        .resilience
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();

    Ok(json!({
        "ok": true,
        "removed": reset_count,
        "target": target,
        "breakers": breakers,
    }))
}

// ---------------------------------------------------------------------------
// Breaker Recovery
// ---------------------------------------------------------------------------

fn health_status_label(status: crate::failure_prevention::HealthStatus) -> &'static str {
    match status {
        crate::failure_prevention::HealthStatus::Healthy => "healthy",
        crate::failure_prevention::HealthStatus::Degraded => "degraded",
        crate::failure_prevention::HealthStatus::Unhealthy => "unhealthy",
    }
}

fn circuit_state_label(state: crate::failure_prevention::CircuitBreakerState) -> &'static str {
    match state {
        crate::failure_prevention::CircuitBreakerState::Closed => "closed",
        crate::failure_prevention::CircuitBreakerState::Open => "open",
        crate::failure_prevention::CircuitBreakerState::HalfOpen => "half-open",
    }
}

fn degradation_level_label(level: crate::failure_prevention::DegradationLevel) -> &'static str {
    match level {
        crate::failure_prevention::DegradationLevel::Normal => "normal",
        crate::failure_prevention::DegradationLevel::Degraded => "degraded",
        crate::failure_prevention::DegradationLevel::Constrained => "constrained",
        crate::failure_prevention::DegradationLevel::Emergency => "emergency",
    }
}

fn recovery_action(
    status: crate::failure_prevention::HealthStatus,
    level: crate::failure_prevention::DegradationLevel,
) -> &'static str {
    if matches!(status, crate::failure_prevention::HealthStatus::Unhealthy)
        || matches!(
            level,
            crate::failure_prevention::DegradationLevel::Emergency
        )
    {
        "reset_breaker_and_fallback"
    } else if matches!(status, crate::failure_prevention::HealthStatus::Degraded)
        || matches!(
            level,
            crate::failure_prevention::DegradationLevel::Constrained
        )
    {
        "degrade_to_secondary_agent"
    } else {
        "observe"
    }
}

pub(super) fn collect_degraded_services(server: &AcpServer) -> Vec<Value> {
    server
        .resilience
        .failure_prevention
        .lock()
        .map(|fp| {
            let mut services = fp.get_health_report();
            services.sort_by(|a, b| a.service_name.cmp(&b.service_name));
            services
                .into_iter()
                .filter_map(|health| {
                    let circuit = fp.get_circuit_state(&health.service_name);
                    let level = fp.get_degradation_strategy(&health.service_name);
                    let should_recover = !matches!(
                        health.status,
                        crate::failure_prevention::HealthStatus::Healthy
                    ) || !matches!(
                        circuit,
                        crate::failure_prevention::CircuitBreakerState::Closed
                    ) || fp.should_degrade(&health.service_name);
                    if !should_recover {
                        return None;
                    }

                    Some(json!({
                        "service": health.service_name,
                        "health_status": health_status_label(health.status),
                        "circuit_state": circuit_state_label(circuit),
                        "degradation_level": degradation_level_label(level),
                        "success_rate": health.success_rate,
                        "error_rate": health.error_rate,
                        "avg_latency_ms": health.avg_latency_ms,
                        "recommended_action": recovery_action(health.status, level),
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(super) async fn breaker_recovery_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let target = params
        .get("agent")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let dry_run = params
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let degraded_before = collect_degraded_services(server);
    let candidates = degraded_before
        .iter()
        .filter_map(|item| {
            item.get("service")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .filter(|service| target.map(|t| t == service).unwrap_or(true))
        .collect::<Vec<_>>();

    let (recovered_services, breaker_reset_count) = if dry_run {
        (Vec::new(), 0)
    } else {
        let recovered_services = server
            .resilience
            .failure_prevention
            .lock()
            .map(|mut fp| fp.recover(target))
            .unwrap_or_default();
        let breaker_reset_count = server
            .resilience
            .circuit_breakers
            .lock()
            .map(|mut guard| guard.reset(target))
            .unwrap_or(0);
        (recovered_services, breaker_reset_count)
    };
    let degraded_after = collect_degraded_services(server);

    Ok(json!({
        "ok": true,
        "dry_run": dry_run,
        "target": target,
        "candidates": candidates,
        "candidate_count": candidates.len(),
        "recovered_services": recovered_services,
        "recovered_count": recovered_services.len(),
        "breaker_reset_count": breaker_reset_count,
        "remaining_degraded_count": degraded_after.len(),
        "remaining_degraded_services": degraded_after,
    }))
}

// ---------------------------------------------------------------------------
// Cache Clear
// ---------------------------------------------------------------------------

pub(super) async fn cache_clear_payload(server: &AcpServer) -> Result<Value> {
    let memory_removed = server
        .cache_deps
        .cache
        .memory_response_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear_all();
    let persistent_removed = if let Some(cache) = server.cache_deps.cache.response_cache.clone() {
        cache.clear_all().await?
    } else {
        0
    };

    Ok(json!({
        "ok": true,
        "memory_removed": memory_removed,
        "sqlite_removed": persistent_removed,
        "total_removed": memory_removed + persistent_removed,
    }))
}

// ---------------------------------------------------------------------------
// Vector Clear
// ---------------------------------------------------------------------------

pub(super) async fn vector_clear_payload(server: &AcpServer) -> Result<Value> {
    let (memory_removed, summary_removed) =
        if let Some(store) = server.cache_deps.cache.vector_store.clone() {
            store.clear_all().await?
        } else {
            (0, 0)
        };

    Ok(json!({
        "ok": true,
        "vector_removed": memory_removed,
        "summary_removed": summary_removed,
    }))
}

// ---------------------------------------------------------------------------
// Maintenance GC
// ---------------------------------------------------------------------------

pub(super) async fn maintenance_gc_payload(server: &AcpServer) -> Result<Value> {
    let cycle = crate::acp::background::run_maintenance_cycle(server).await?;
    let maintenance = server
        .resilience
        .maintenance_tracker
        .read()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();

    Ok(json!({
        "ok": true,
        "memory_expired_removed": cycle.memory_expired_removed,
        "maintenance": maintenance,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── degradation_level_label ────────────────────────────────────────

    #[test]
    fn degradation_level_label_all_variants() {
        use crate::failure_prevention::DegradationLevel;
        assert_eq!(degradation_level_label(DegradationLevel::Normal), "normal");
        assert_eq!(
            degradation_level_label(DegradationLevel::Degraded),
            "degraded"
        );
        assert_eq!(
            degradation_level_label(DegradationLevel::Constrained),
            "constrained"
        );
        assert_eq!(
            degradation_level_label(DegradationLevel::Emergency),
            "emergency"
        );
    }

    // ── recovery_action ────────────────────────────────────────────────

    #[test]
    fn recovery_action_unhealthy_returns_reset() {
        use crate::failure_prevention::{DegradationLevel, HealthStatus};
        assert_eq!(
            recovery_action(HealthStatus::Unhealthy, DegradationLevel::Normal),
            "reset_breaker_and_fallback"
        );
    }

    #[test]
    fn recovery_action_critical_level_returns_reset() {
        use crate::failure_prevention::{DegradationLevel, HealthStatus};
        assert_eq!(
            recovery_action(HealthStatus::Degraded, DegradationLevel::Emergency),
            "reset_breaker_and_fallback"
        );
    }

    #[test]
    fn recovery_action_degraded_significant() {
        use crate::failure_prevention::{DegradationLevel, HealthStatus};
        assert_eq!(
            recovery_action(HealthStatus::Degraded, DegradationLevel::Constrained),
            "degrade_to_secondary_agent"
        );
    }

    #[test]
    fn recovery_action_healthy_none_observes() {
        use crate::failure_prevention::{DegradationLevel, HealthStatus};
        assert_eq!(
            recovery_action(HealthStatus::Healthy, DegradationLevel::Normal),
            "observe"
        );
    }
}
