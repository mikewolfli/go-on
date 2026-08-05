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
    // Reset the REAL per-agent breakers + health through the unified
    // hyper-resilience engine.
    let recovered = server.resilience.hyper_resilience.recover_services(target);
    let reset_count = recovered.len();
    let breakers = server
        .resilience
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();

    Ok(json!({
        "ok": true,
        "removed": reset_count,
        "recovered": recovered,
        "target": target,
        "breakers": breakers,
    }))
}

// ---------------------------------------------------------------------------
// Breaker Recovery
// ---------------------------------------------------------------------------

fn health_status_label(status: crate::resilience::hyper_resilience::HealthStatus) -> &'static str {
    match status {
        crate::resilience::hyper_resilience::HealthStatus::Healthy => "healthy",
        crate::resilience::hyper_resilience::HealthStatus::Degraded => "degraded",
        crate::resilience::hyper_resilience::HealthStatus::Unhealthy => "unhealthy",
    }
}

fn circuit_state_label(
    state: crate::resilience::hyper_resilience::CircuitBreakerState,
) -> &'static str {
    match state {
        crate::resilience::hyper_resilience::CircuitBreakerState::Closed => "closed",
        crate::resilience::hyper_resilience::CircuitBreakerState::Open => "open",
        crate::resilience::hyper_resilience::CircuitBreakerState::HalfOpen => "half-open",
    }
}

fn degradation_level_label(
    level: crate::resilience::hyper_resilience::DegradationLevel,
) -> &'static str {
    match level {
        crate::resilience::hyper_resilience::DegradationLevel::Normal => "normal",
        crate::resilience::hyper_resilience::DegradationLevel::Degraded => "degraded",
        crate::resilience::hyper_resilience::DegradationLevel::Constrained => "constrained",
        crate::resilience::hyper_resilience::DegradationLevel::Emergency => "emergency",
    }
}

fn recovery_action(
    status: crate::resilience::hyper_resilience::HealthStatus,
    level: crate::resilience::hyper_resilience::DegradationLevel,
) -> &'static str {
    if matches!(
        status,
        crate::resilience::hyper_resilience::HealthStatus::Unhealthy
    ) || matches!(
        level,
        crate::resilience::hyper_resilience::DegradationLevel::Emergency
    ) {
        "reset_breaker_and_fallback"
    } else if matches!(
        status,
        crate::resilience::hyper_resilience::HealthStatus::Degraded
    ) || matches!(
        level,
        crate::resilience::hyper_resilience::DegradationLevel::Constrained
    ) {
        "degrade_to_secondary_agent"
    } else {
        "observe"
    }
}

pub(super) fn collect_degraded_services(server: &AcpServer) -> Vec<Value> {
    let hre = &server.resilience.hyper_resilience;
    let mut services = hre.health_report();
    services.sort_by(|a, b| a.service_name.cmp(&b.service_name));
    let mut out = Vec::new();
    for health in services {
        let circuit = hre.breaker_state(&health.service_name);
        let level = hre.degradation_level(&health.service_name);
        let should_recover = !matches!(
            health.status,
            crate::resilience::hyper_resilience::HealthStatus::Healthy
        ) || !matches!(
            circuit,
            crate::resilience::hyper_resilience::CircuitState::Closed
        ) || hre.should_degrade(&health.service_name);
        if !should_recover {
            continue;
        }
        out.push(json!({
            "service": health.service_name,
            "health_status": health_status_label(health.status),
            "circuit_state": circuit_state_label(circuit),
            "degradation_level": degradation_level_label(level),
            "success_rate": health.success_rate,
            "error_rate": health.error_rate,
            "avg_latency_ms": health.avg_latency_ms,
            "recommended_action": recovery_action(health.status, level),
        }));
    }
    out
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
        // Recover through the unified engine (breaker + health + counters).
        let recovered_services = server.resilience.hyper_resilience.recover_services(target);
        let breaker_reset_count = recovered_services.len();
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
    let memory_removed = {
        let cache = &server.cache_deps.cache.semantic_cache;
        let removed = cache.write().map(|guard| guard.len()).unwrap_or(0);
        if let Ok(guard) = cache.write() {
            guard.clear();
        }
        removed
    };
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
        use crate::resilience::hyper_resilience::DegradationLevel;
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
        use crate::resilience::hyper_resilience::{DegradationLevel, HealthStatus};
        assert_eq!(
            recovery_action(HealthStatus::Unhealthy, DegradationLevel::Normal),
            "reset_breaker_and_fallback"
        );
    }

    #[test]
    fn recovery_action_critical_level_returns_reset() {
        use crate::resilience::hyper_resilience::{DegradationLevel, HealthStatus};
        assert_eq!(
            recovery_action(HealthStatus::Degraded, DegradationLevel::Emergency),
            "reset_breaker_and_fallback"
        );
    }

    #[test]
    fn recovery_action_degraded_significant() {
        use crate::resilience::hyper_resilience::{DegradationLevel, HealthStatus};
        assert_eq!(
            recovery_action(HealthStatus::Degraded, DegradationLevel::Constrained),
            "degrade_to_secondary_agent"
        );
    }

    #[test]
    fn recovery_action_healthy_none_observes() {
        use crate::resilience::hyper_resilience::{DegradationLevel, HealthStatus};
        assert_eq!(
            recovery_action(HealthStatus::Healthy, DegradationLevel::Normal),
            "observe"
        );
    }
}
