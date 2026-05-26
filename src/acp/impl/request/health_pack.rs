use super::*;

// ---------------------------------------------------------------------------
// Breaker Status
// ---------------------------------------------------------------------------

pub(super) async fn handle_breaker_status(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let breakers = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();
    let open_count = breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count();
    let degraded_services = collect_degraded_services(server);
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "open_count": open_count,
            "degraded_count": degraded_services.len(),
            "degraded_services": degraded_services,
            "breakers": breakers,
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// Breaker Reset
// ---------------------------------------------------------------------------

pub(super) async fn handle_breaker_reset(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let target = params
        .get("agent")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str);
    let reset_count = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.reset(target))
        .unwrap_or(0);
    let breakers = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "removed": reset_count,
            "target": target,
            "breakers": breakers,
        }),
    )
    .await
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
        crate::failure_prevention::DegradationLevel::None => "none",
        crate::failure_prevention::DegradationLevel::Minimal => "minimal",
        crate::failure_prevention::DegradationLevel::Moderate => "moderate",
        crate::failure_prevention::DegradationLevel::Significant => "significant",
        crate::failure_prevention::DegradationLevel::Critical => "critical",
    }
}

fn recovery_action(
    status: crate::failure_prevention::HealthStatus,
    level: crate::failure_prevention::DegradationLevel,
) -> &'static str {
    if matches!(status, crate::failure_prevention::HealthStatus::Unhealthy)
        || matches!(level, crate::failure_prevention::DegradationLevel::Critical)
    {
        "reset_breaker_and_fallback"
    } else if matches!(status, crate::failure_prevention::HealthStatus::Degraded)
        || matches!(
            level,
            crate::failure_prevention::DegradationLevel::Significant
        )
    {
        "degrade_to_secondary_agent"
    } else {
        "observe"
    }
}

pub(super) fn collect_degraded_services(server: &AcpServer) -> Vec<Value> {
    server
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

pub(super) async fn handle_breaker_recovery(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
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
            .failure_prevention
            .lock()
            .map(|mut fp| fp.recover(target))
            .unwrap_or_default();
        let breaker_reset_count = server
            .circuit_breakers
            .lock()
            .map(|guard| guard.reset(target))
            .unwrap_or(0);
        (recovered_services, breaker_reset_count)
    };
    let degraded_after = collect_degraded_services(server);

    send_result(
        server,
        request_id,
        json!({
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
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// Cache Clear
// ---------------------------------------------------------------------------

pub(super) async fn handle_cache_clear(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let memory_removed = server
        .cache
        .memory_response_cache
        .lock()
        .map(|cache| cache.clear_all())
        .unwrap_or(0);
    let persistent_removed =
        if let Some(cache) = server.cache.response_cache.clone() {
            crate::acp::r#impl::storage::cache_clear(server, cache).await?
        } else {
            0
        };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "memory_removed": memory_removed,
            "sqlite_removed": persistent_removed,
            "total_removed": memory_removed + persistent_removed,
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// Vector Clear
// ---------------------------------------------------------------------------

pub(super) async fn handle_vector_clear(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let (memory_removed, summary_removed) =
        if let Some(store) = server.cache.vector_store.clone() {
            store.clear_all()?
        } else {
            (0, 0)
        };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "vector_removed": memory_removed,
            "summary_removed": summary_removed,
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// Maintenance GC
// ---------------------------------------------------------------------------

pub(super) async fn handle_maintenance_gc(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let cycle = crate::acp::background::run_maintenance_cycle(server).await?;
    let maintenance = server
        .maintenance_tracker
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "memory_expired_removed": cycle.memory_expired_removed,
            "sqlite_expired_removed": cycle.sqlite_expired_removed,
            "cache_vacuumed": cycle.cache_vacuumed,
            "vector_vacuumed": cycle.vector_vacuumed,
            "maintenance": maintenance,
        }),
    )
    .await
}
