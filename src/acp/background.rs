//! ACP Background Tasks - Background task management
//!
//! This module contains background task implementations for the ACP server,
//! including maintenance cycles, health checks, and periodic operations.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Notify;
use tokio::task::spawn_blocking;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::cache::ResponseCache;
use crate::config::RuntimeConfig;
use crate::memory_module::MemoryStore;
use crate::memory_response_cache::MemoryResponseCache;
use crate::vector::VectorStore;

use super::prelude::{
    with_acp_lock, AcpLockMonitor, CircuitBreakerRegistry, InflightLimiter, LifecycleState,
    MaintenanceTracker, PhaseRateLimiter, ACP_LOCK_CIRCUIT_BREAKERS, ACP_LOCK_INFLIGHT_LIMITER,
    ACP_LOCK_LIFECYCLE, ACP_LOCK_MAINTENANCE, ACP_LOCK_MEMORY_CACHE, ACP_LOCK_MEMORY_STORE,
    ACP_LOCK_PHASE_RATE_LIMITER, ACP_LOCK_RESPONSE_CACHE, ACP_LOCK_RUNTIME_CONFIG,
    ACP_LOCK_VECTOR_STORE,
};

/// Maintenance cycle result
#[derive(Debug, Default, Clone, Copy)]
pub struct MaintenanceCycleResult {
    /// Number of expired entries removed from memory cache
    pub memory_expired_removed: usize,
    /// Whether runtime memory store GC was executed
    pub memory_store_gc_ran: bool,
    /// Number of expired entries removed from SQLite cache
    pub sqlite_expired_removed: usize,
    /// Whether cache vacuum was performed
    pub cache_vacuumed: bool,
    /// Whether vector store vacuum was performed
    pub vector_vacuumed: bool,
}

/// Shared context for background maintenance operations.
///
/// Groups all shared state handles needed by the background maintenance loop
/// into a single struct, eliminating the previous 12-parameter function signature.
#[derive(Debug)]
pub struct BackgroundContext {
    pub lock_monitor: Arc<AcpLockMonitor>,
    pub runtime_config: Arc<std::sync::Mutex<RuntimeConfig>>,
    pub memory_cache: Arc<std::sync::Mutex<MemoryResponseCache>>,
    pub memory_store: Arc<std::sync::Mutex<MemoryStore>>,
    pub cache: Arc<std::sync::Mutex<Option<Arc<ResponseCache>>>>,
    pub vector_store: Arc<std::sync::Mutex<Option<Arc<VectorStore>>>>,
    pub maintenance: Arc<std::sync::Mutex<MaintenanceTracker>>,
    pub lifecycle: Arc<std::sync::Mutex<LifecycleState>>,
    pub circuit_breakers: Arc<std::sync::Mutex<CircuitBreakerRegistry>>,
    pub phase_rate_limiter: Arc<std::sync::Mutex<PhaseRateLimiter>>,
    pub inflight_limiter: Arc<std::sync::Mutex<InflightLimiter>>,
    pub shutdown_notify: Arc<Notify>,
}

/// Run background maintenance loop
///
/// This function runs periodic maintenance tasks including cache cleanup,
/// health checks, and system monitoring.
pub async fn run_background_maintenance_loop(ctx: BackgroundContext) {
    let config = with_acp_lock(
        ctx.lock_monitor.as_ref(),
        ACP_LOCK_RUNTIME_CONFIG,
        ctx.runtime_config.as_ref(),
        |guard| guard.clone(),
    );

    let mut maintenance_interval = interval(Duration::from_secs(
        config.maintenance_interval_seconds.max(1),
    ));
    let mut health_interval = interval(Duration::from_secs(config.health_interval_seconds.max(1)));

    maintenance_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    health_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let max_iterations: u64 = 1000;
    let mut iteration_count: u64 = 0;

    loop {
        if iteration_count >= max_iterations {
            info!(
                "background maintenance loop reached max iterations ({})",
                max_iterations
            );
            break;
        }
        iteration_count += 1;

        tokio::select! {
            _ = ctx.shutdown_notify.notified() => break,
            _ = maintenance_interval.tick() => {
                if with_acp_lock(
                    ctx.lock_monitor.as_ref(),
                    ACP_LOCK_LIFECYCLE,
                    ctx.lifecycle.as_ref(),
                    |guard| guard.is_shutting_down(),
                ) {
                    break;
                }

                if let Err(err) = perform_maintenance_cycle(
                    ctx.lock_monitor.clone(),
                    ctx.memory_cache.clone(),
                    ctx.memory_store.clone(),
                    ctx.cache.clone(),
                    ctx.vector_store.clone(),
                    ctx.maintenance.clone(),
                    ctx.runtime_config.clone(),
                    "background",
                ).await {
                    warn!("background maintenance cycle failed: {}", err);
                }
            }
            _ = health_interval.tick() => {
                if with_acp_lock(
                    ctx.lock_monitor.as_ref(),
                    ACP_LOCK_LIFECYCLE,
                    ctx.lifecycle.as_ref(),
                    |guard| guard.is_shutting_down(),
                ) {
                    break;
                }

                if let Err(err) = perform_health_check_cycle(
                    ctx.lock_monitor.clone(),
                    ctx.memory_cache.clone(),
                    ctx.cache.clone(),
                    ctx.vector_store.clone(),
                    ctx.circuit_breakers.clone(),
                    ctx.phase_rate_limiter.clone(),
                    ctx.inflight_limiter.clone(),
                    ctx.lifecycle.clone(),
                    ctx.maintenance.clone(),
                ).await {
                    warn!("health check cycle failed: {}", err);
                }
            }
        }
    }

    info!("background maintenance loop stopped");
}

/// Perform maintenance cycle
#[allow(clippy::too_many_arguments)]
pub async fn perform_maintenance_cycle(
    lock_monitor: Arc<AcpLockMonitor>,
    memory_cache: Arc<std::sync::Mutex<MemoryResponseCache>>,
    memory_store: Arc<std::sync::Mutex<MemoryStore>>,
    cache: Arc<std::sync::Mutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<std::sync::Mutex<Option<Arc<VectorStore>>>>,
    maintenance: Arc<std::sync::Mutex<MaintenanceTracker>>,
    runtime_config: Arc<std::sync::Mutex<RuntimeConfig>>,
    source: &str,
) -> Result<MaintenanceCycleResult> {
    with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_MAINTENANCE,
        maintenance.as_ref(),
        |guard| guard.note_started(),
    );

    let mut result = MaintenanceCycleResult {
        memory_expired_removed: with_acp_lock(
            lock_monitor.as_ref(),
            ACP_LOCK_MEMORY_CACHE,
            memory_cache.as_ref(),
            |guard| guard.purge_expired(),
        ),
        ..MaintenanceCycleResult::default()
    };

    // Clean memory cache
    debug!(
        "{}: cleaned {} expired entries from memory cache",
        source, result.memory_expired_removed
    );

    if with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_MEMORY_STORE,
        memory_store.as_ref(),
        |guard| {
            guard.gc();
            true
        },
    ) {
        result.memory_store_gc_ran = true;
    }

    // Clean SQLite cache if available
    if let Some(cache_ref) = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_RESPONSE_CACHE,
        cache.as_ref(),
        |guard| guard.clone(),
    ) {
        match spawn_blocking(move || cache_ref.purge_expired()).await {
            Ok(Ok(removed)) => {
                result.sqlite_expired_removed = removed;
                debug!(
                    "{}: cleaned {} expired entries from SQLite cache",
                    source, removed
                );
            }
            Ok(Err(err)) => {
                warn!("{}: failed to clean SQLite cache: {}", source, err);
            }
            Err(err) => {
                warn!("{}: failed to join cache purge task: {}", source, err);
            }
        }
    }

    // Vacuum caches if configured
    let config = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_RUNTIME_CONFIG,
        runtime_config.as_ref(),
        |guard| guard.clone(),
    );
    let vacuum_interval_cycles = config.sqlite_vacuum_interval_cycles.max(1);
    let current_cycle = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_MAINTENANCE,
        maintenance.as_ref(),
        |guard| guard.snapshot().cycles_total,
    );
    let should_vacuum = current_cycle.is_multiple_of(vacuum_interval_cycles);

    if should_vacuum {
        if let Some(cache_ref) = with_acp_lock(
            lock_monitor.as_ref(),
            ACP_LOCK_RESPONSE_CACHE,
            cache.as_ref(),
            |guard| guard.clone(),
        ) {
            match spawn_blocking(move || cache_ref.vacuum()).await {
                Ok(Ok(_)) => {
                    result.cache_vacuumed = true;
                    debug!("{}: vacuumed SQLite cache", source);
                }
                Ok(Err(err)) => {
                    warn!("{}: failed to vacuum SQLite cache: {}", source, err);
                }
                Err(err) => {
                    warn!("{}: failed to join cache vacuum task: {}", source, err);
                }
            }
        }
    }

    if let Some(vector_ref) = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_VECTOR_STORE,
        vector_store.as_ref(),
        |guard| guard.clone(),
    ) {
        match spawn_blocking(move || vector_ref.vacuum()).await {
            Ok(Ok(_)) => {
                result.vector_vacuumed = true;
                debug!("{}: vacuumed vector store", source);
            }
            Ok(Err(err)) => {
                warn!("{}: failed to vacuum vector store: {}", source, err);
            }
            Err(err) => {
                warn!("{}: failed to join vector vacuum task: {}", source, err);
            }
        }
    }

    with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_MAINTENANCE,
        maintenance.as_ref(),
        |guard| {
            guard.note_completed(
                result.memory_expired_removed,
                result.sqlite_expired_removed,
                result.cache_vacuumed,
                result.vector_vacuumed,
            );
        },
    );
    Ok(result)
}

/// Perform health check cycle
#[allow(clippy::too_many_arguments)]
pub async fn perform_health_check_cycle(
    lock_monitor: Arc<AcpLockMonitor>,
    memory_cache: Arc<std::sync::Mutex<MemoryResponseCache>>,
    cache: Arc<std::sync::Mutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<std::sync::Mutex<Option<Arc<VectorStore>>>>,
    circuit_breakers: Arc<std::sync::Mutex<CircuitBreakerRegistry>>,
    phase_rate_limiter: Arc<std::sync::Mutex<PhaseRateLimiter>>,
    inflight_limiter: Arc<std::sync::Mutex<InflightLimiter>>,
    lifecycle: Arc<std::sync::Mutex<LifecycleState>>,
    maintenance: Arc<std::sync::Mutex<MaintenanceTracker>>,
) -> Result<()> {
    let memory_health = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_MEMORY_CACHE,
        memory_cache.as_ref(),
        |cache| {
            cache.active_entries();
            true
        },
    );

    let sqlite_health = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_RESPONSE_CACHE,
        cache.as_ref(),
        |guard| guard.clone(),
    )
    .map(|cache| cache.entry_count().is_ok())
    .unwrap_or(true);

    let vector_health = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_VECTOR_STORE,
        vector_store.as_ref(),
        |guard| guard.clone(),
    )
    .map(|store| store.memory_entry_count().is_ok() && store.summary_entry_count().is_ok())
    .unwrap_or(true);

    // Check circuit breakers
    let circuit_breaker_health = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_CIRCUIT_BREAKERS,
        circuit_breakers.as_ref(),
        |guard| guard.is_healthy(),
    );
    if !circuit_breaker_health {
        warn!("circuit breaker health check failed");
    }

    // Check rate limiters
    let phase_healthy = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_PHASE_RATE_LIMITER,
        phase_rate_limiter.as_ref(),
        |guard| guard.is_healthy(),
    );
    let inflight_healthy = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_INFLIGHT_LIMITER,
        inflight_limiter.as_ref(),
        |guard| guard.is_healthy(),
    );
    let rate_limiter_health = phase_healthy && inflight_healthy;
    if !rate_limiter_health {
        warn!("rate limiter health check failed");
    }

    // Check lifecycle
    let lifecycle_health = with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_LIFECYCLE,
        lifecycle.as_ref(),
        |guard| guard.is_healthy(),
    );
    if !lifecycle_health {
        warn!("lifecycle health check failed");
    }

    // Overall health
    let overall_health = memory_health
        && sqlite_health
        && vector_health
        && circuit_breaker_health
        && rate_limiter_health
        && lifecycle_health;

    // Update health status in lifecycle state.
    with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_LIFECYCLE,
        lifecycle.as_ref(),
        |guard| {
            if overall_health {
                guard.mark_healthy();
                info!("Health check passed");
            } else {
                guard.mark_unhealthy();
                warn!("Health check failed");
            }
            guard.update_health_check();
        },
    );

    // Update maintenance tracker
    with_acp_lock(
        lock_monitor.as_ref(),
        ACP_LOCK_MAINTENANCE,
        maintenance.as_ref(),
        |guard| guard.record_health_check(overall_health),
    );

    Ok(())
}

/// Start background tasks for an ACP server
pub async fn start_background_tasks(
    server: &super::server::AcpServer,
    shutdown_notify: Arc<Notify>,
) -> Result<()> {
    let lock_monitor = Arc::clone(&server.observability.lock_monitor);
    let runtime_config = Arc::new(std::sync::Mutex::new(server.runtime_config.clone()));
    let memory_cache = Arc::clone(&server.cache.memory_response_cache);
    let memory_store = Arc::clone(&server.memory_store);

    let cache = Arc::new(std::sync::Mutex::new(server.cache.response_cache.clone()));
    let vector_store = Arc::new(std::sync::Mutex::new(server.cache.vector_store.clone()));

    let maintenance = Arc::clone(&server.maintenance_tracker);
    let lifecycle = Arc::clone(&server.lifecycle_state);
    let circuit_breakers = Arc::clone(&server.circuit_breakers);

    let phase_rate_limiter = Arc::clone(&server.phase_rate_limiter);
    let inflight_limiter = Arc::clone(&server.inflight_limiter);

    tokio::spawn(async move {
        let bg_ctx = BackgroundContext {
            lock_monitor,
            runtime_config,
            memory_cache,
            memory_store,
            cache,
            vector_store,
            maintenance,
            lifecycle,
            circuit_breakers,
            phase_rate_limiter,
            inflight_limiter,
            shutdown_notify,
        };
        run_background_maintenance_loop(bg_ctx).await;
    });

    Ok(())
}

/// Stop all background tasks
#[allow(dead_code)] // F-GAP-03 — planned wiring: lifecycle/background task orchestration
pub fn stop_background_tasks(shutdown_notify: Arc<Notify>) {
    shutdown_notify.notify_waiters();
}

/// Run a single maintenance cycle on demand
pub async fn run_maintenance_cycle(
    server: &super::server::AcpServer,
) -> Result<MaintenanceCycleResult> {
    let lock_monitor = Arc::clone(&server.observability.lock_monitor);
    let runtime_config = Arc::new(std::sync::Mutex::new(server.runtime_config.clone()));
    let memory_cache = Arc::clone(&server.cache.memory_response_cache);
    let memory_store = Arc::clone(&server.memory_store);

    let cache = Arc::new(std::sync::Mutex::new(server.cache.response_cache.clone()));
    let vector_store = Arc::new(std::sync::Mutex::new(server.cache.vector_store.clone()));

    let maintenance = Arc::clone(&server.maintenance_tracker);

    perform_maintenance_cycle(
        lock_monitor,
        memory_cache,
        memory_store,
        cache,
        vector_store,
        maintenance,
        runtime_config,
        "manual",
    )
    .await
}

/// Run a single health check on demand
pub async fn run_health_check(server: &super::server::AcpServer) -> Result<()> {
    let lock_monitor = Arc::clone(&server.observability.lock_monitor);
    let memory_cache = Arc::clone(&server.cache.memory_response_cache);

    let cache = Arc::new(std::sync::Mutex::new(server.cache.response_cache.clone()));
    let vector_store = Arc::new(std::sync::Mutex::new(server.cache.vector_store.clone()));

    let circuit_breakers = Arc::clone(&server.circuit_breakers);
    let lifecycle = Arc::clone(&server.lifecycle_state);
    let maintenance = Arc::clone(&server.maintenance_tracker);

    let phase_rate_limiter = Arc::clone(&server.phase_rate_limiter);
    let inflight_limiter = Arc::clone(&server.inflight_limiter);

    perform_health_check_cycle(
        lock_monitor,
        memory_cache,
        cache,
        vector_store,
        circuit_breakers,
        phase_rate_limiter,
        inflight_limiter,
        lifecycle,
        maintenance,
    )
    .await
}
