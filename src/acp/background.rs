#[derive(Debug, Default, Clone, Copy)]
struct MaintenanceCycleResult {
    memory_expired_removed: usize,
    sqlite_expired_removed: usize,
    cache_vacuumed: bool,
    vector_vacuumed: bool,
}

#[allow(clippy::too_many_arguments)]
async fn run_background_maintenance_loop(
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    maintenance: Arc<MaintenanceTracker>,
    lifecycle: Arc<LifecycleState>,
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    inflight_limiter: Arc<InflightLimiter>,
    shutdown_notify: Arc<Notify>,
) {
    let config = runtime_config
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let mut maintenance_interval = tokio::time::interval(Duration::from_secs(
        config.maintenance_interval_seconds.max(1),
    ));
    let mut health_interval =
        tokio::time::interval(Duration::from_secs(config.health_interval_seconds.max(1)));
    maintenance_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    health_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_notify.notified() => break,
            _ = maintenance_interval.tick() => {
                if lifecycle.is_shutting_down() {
                    break;
                }

                if let Err(err) = perform_maintenance_cycle(
                    Arc::clone(&memory_cache),
                    Arc::clone(&cache),
                    Arc::clone(&vector_store),
                    Arc::clone(&runtime_config),
                    Arc::clone(&maintenance),
                    "background",
                ).await {
                    warn!("background maintenance cycle failed: {}", err);
                }
            }
            _ = health_interval.tick() => {
                if lifecycle.is_shutting_down() {
                    break;
                }

                log_background_health(
                    Arc::clone(&memory_cache),
                    Arc::clone(&cache),
                    Arc::clone(&vector_store),
                    Arc::clone(&circuit_breakers),
                    Arc::clone(&phase_rate_limiter),
                    Arc::clone(&inflight_limiter),
                    Arc::clone(&lifecycle),
                    Arc::clone(&maintenance),
                ).await;
            }
        }
    }
}

async fn perform_maintenance_cycle(
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    maintenance: Arc<MaintenanceTracker>,
    source: &str,
) -> Result<MaintenanceCycleResult> {
    maintenance.note_started();
    let vacuum_interval_cycles = runtime_config
        .lock()
        .map(|guard| guard.sqlite_vacuum_interval_cycles.max(1))
        .unwrap_or(60);
    let current_cycle = maintenance.snapshot().cycles_total;
    let should_vacuum = current_cycle.is_multiple_of(vacuum_interval_cycles);

    let memory_expired_removed = memory_cache.purge_expired();
    let cache_handle = cache.lock().ok().and_then(|guard| guard.clone());
    let sqlite_expired_removed_result = if let Some(cache) = cache_handle.clone() {
        spawn_blocking(move || cache.purge_expired())
            .await
            .map_err(|e| anyhow::anyhow!("cache purge task join error: {}", e))?
    } else {
        Ok(0)
    };
    let sqlite_expired_removed = match sqlite_expired_removed_result {
        Ok(value) => value,
        Err(err) => {
            maintenance.note_failed(&err.to_string());
            return Err(err);
        }
    };

    let cache_vacuumed = if should_vacuum {
        if let Some(cache) = cache_handle.clone() {
            spawn_blocking(move || cache.vacuum())
                .await
                .map_err(|e| anyhow::anyhow!("cache vacuum task join error: {}", e))??;
            true
        } else {
            false
        }
    } else {
        false
    };

    let vector_vacuumed = if should_vacuum {
        if let Some(store) = vector_store.lock().ok().and_then(|guard| guard.clone()) {
            spawn_blocking(move || store.vacuum())
                .await
                .map_err(|e| anyhow::anyhow!("vector vacuum task join error: {}", e))??;
            true
        } else {
            false
        }
    } else {
        false
    };

    let result = MaintenanceCycleResult {
        memory_expired_removed,
        sqlite_expired_removed,
        cache_vacuumed,
        vector_vacuumed,
    };

    maintenance.note_completed(
        memory_expired_removed,
        sqlite_expired_removed,
        cache_vacuumed,
        vector_vacuumed,
    );
    info!(
        "maintenance cycle '{}' completed (memory_removed={}, sqlite_removed={}, cache_vacuumed={}, vector_vacuumed={})",
        source,
        result.memory_expired_removed,
        result.sqlite_expired_removed,
        result.cache_vacuumed,
        result.vector_vacuumed
    );
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn log_background_health(
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    inflight_limiter: Arc<InflightLimiter>,
    lifecycle: Arc<LifecycleState>,
    maintenance: Arc<MaintenanceTracker>,
) {
    let sqlite_cache_entries =
        if let Some(cache) = cache.lock().ok().and_then(|guard| guard.clone()) {
            match spawn_blocking(move || cache.entry_count()).await {
                Ok(Ok(count)) => Some(count),
                Ok(Err(err)) => {
                    warn!(
                        "background health failed to read sqlite cache entries: {}",
                        err
                    );
                    None
                }
                Err(err) => {
                    warn!("background health cache count task failed: {}", err);
                    None
                }
            }
        } else {
            None
        };

    let vector_counts =
        if let Some(store) = vector_store.lock().ok().and_then(|guard| guard.clone()) {
            match spawn_blocking(move || {
                Ok::<(u64, u64), anyhow::Error>((
                    store.memory_entry_count()?,
                    store.summary_entry_count()?,
                ))
            })
            .await
            {
                Ok(Ok(counts)) => Some(counts),
                Ok(Err(err)) => {
                    warn!("background health failed to read vector counts: {}", err);
                    None
                }
                Err(err) => {
                    warn!("background health vector count task failed: {}", err);
                    None
                }
            }
        } else {
            None
        };

    let (global_inflight, phase_inflight) = inflight_limiter.snapshot();
    let lifecycle_snapshot = lifecycle.snapshot();
    let maintenance_snapshot = maintenance.snapshot();

    info!(
        "runtime health: shutting_down={}, inflight_global={}, inflight_phases={}, memory_cache_entries={}, sqlite_cache_entries={:?}, vector_counts={:?}, breaker_open={}, breaker_half_open={}, rate_limiter_tracked={}, maintenance_running={}, maintenance_cycles={}",
        lifecycle_snapshot.shutting_down,
        global_inflight,
        phase_inflight.len(),
        memory_cache.active_entries(),
        sqlite_cache_entries,
        vector_counts,
        circuit_breakers.open_count(),
        circuit_breakers.half_open_count(),
        phase_rate_limiter.tracked_phases(),
        maintenance_snapshot.running,
        maintenance_snapshot.cycles_total,
    );
}
