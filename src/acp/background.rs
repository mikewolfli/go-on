//! ACP Background Tasks - Background task management
//!
//! This module contains background task implementations for the ACP server,
//! including maintenance cycles, health checks, and periodic operations.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::Notify;
use tokio::task::spawn_blocking;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::cache::ResponseCache;
use crate::config::RuntimeConfig;
use crate::intelligence::fusion_evolution_bridge::init_fusion_evolution_bridge;
use crate::memory_module::MemoryStore;
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::metrics_exporter::bridge_metrics_recorder;
use crate::observability::telemetry_enhanced::global_metrics_recorder;
use crate::orchestration::self_evolution::evolution_loop::PubsubTriggerSource;
use crate::vector::VectorStore;

use super::prelude::{
    with_acp_lock_async, AcpLockMonitor, CircuitBreakerRegistry, InflightLimiter, LifecycleState,
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
    pub runtime_config: Arc<tokio::sync::Mutex<RuntimeConfig>>,
    pub memory_cache: Arc<tokio::sync::Mutex<MemoryResponseCache>>,
    pub memory_store: Arc<tokio::sync::Mutex<MemoryStore>>,
    pub cache: Arc<tokio::sync::Mutex<Option<Arc<ResponseCache>>>>,
    pub vector_store: Arc<tokio::sync::Mutex<Option<Arc<VectorStore>>>>,
    pub maintenance: Arc<tokio::sync::Mutex<MaintenanceTracker>>,
    pub lifecycle: Arc<tokio::sync::Mutex<LifecycleState>>,
    pub circuit_breakers: Arc<tokio::sync::Mutex<CircuitBreakerRegistry>>,
    pub phase_rate_limiter: Arc<tokio::sync::Mutex<PhaseRateLimiter>>,
    pub inflight_limiter: Arc<tokio::sync::Mutex<InflightLimiter>>,
    pub shutdown_notify: Arc<Notify>,
}

/// Run background maintenance loop
///
/// This function runs periodic maintenance tasks including cache cleanup,
/// health checks, and system monitoring.
pub async fn run_background_maintenance_loop(ctx: BackgroundContext) {
    let config = with_acp_lock_async(
        ctx.lock_monitor.as_ref(),
        ACP_LOCK_RUNTIME_CONFIG,
        ctx.runtime_config.as_ref(),
        |guard| guard.clone(),
    )
    .await;

    let mut maintenance_interval = interval(Duration::from_secs(
        config.maintenance_interval_seconds.max(1),
    ));
    let mut health_interval = interval(Duration::from_secs(config.health_interval_seconds.max(1)));

    maintenance_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    health_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ctx.shutdown_notify.notified() => break,
            _ = maintenance_interval.tick() => {
                if with_acp_lock_async(
                    ctx.lock_monitor.as_ref(),
                    ACP_LOCK_LIFECYCLE,
                    ctx.lifecycle.as_ref(),
                    |guard| guard.is_shutting_down(),
                ).await {
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
                if with_acp_lock_async(
                    ctx.lock_monitor.as_ref(),
                    ACP_LOCK_LIFECYCLE,
                    ctx.lifecycle.as_ref(),
                    |guard| guard.is_shutting_down(),
                ).await {
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
    memory_cache: Arc<TokioMutex<MemoryResponseCache>>,
    memory_store: Arc<TokioMutex<MemoryStore>>,
    cache: Arc<TokioMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<TokioMutex<Option<Arc<VectorStore>>>>,
    maintenance: Arc<TokioMutex<MaintenanceTracker>>,
    runtime_config: Arc<TokioMutex<RuntimeConfig>>,
    source: &str,
) -> Result<MaintenanceCycleResult> {
    let _ = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_MAINTENANCE,
        maintenance.as_ref(),
        |guard| guard.note_started(),
    )
    .await;

    let mut result = MaintenanceCycleResult {
        memory_expired_removed: with_acp_lock_async(
            lock_monitor.as_ref(),
            ACP_LOCK_MEMORY_CACHE,
            memory_cache.as_ref(),
            |guard| guard.purge_expired(),
        )
        .await,
        ..MaintenanceCycleResult::default()
    };

    // Clean memory cache
    debug!(
        "{}: cleaned {} expired entries from memory cache",
        source, result.memory_expired_removed
    );

    if with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_MEMORY_STORE,
        memory_store.as_ref(),
        |guard| {
            guard.gc();
            true
        },
    )
    .await
    {
        result.memory_store_gc_ran = true;
    }

    // Clean SQLite cache if available
    if let Some(cache_ref) = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_RESPONSE_CACHE,
        cache.as_ref(),
        |guard| guard.clone(),
    )
    .await
    {
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
    let config = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_RUNTIME_CONFIG,
        runtime_config.as_ref(),
        |guard| guard.clone(),
    )
    .await;
    let vacuum_interval_cycles = config.sqlite_vacuum_interval_cycles.max(1);
    let current_cycle = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_MAINTENANCE,
        maintenance.as_ref(),
        |guard| guard.snapshot().cycles_total,
    )
    .await;
    let should_vacuum = current_cycle.is_multiple_of(vacuum_interval_cycles);

    if should_vacuum {
        if let Some(cache_ref) = with_acp_lock_async(
            lock_monitor.as_ref(),
            ACP_LOCK_RESPONSE_CACHE,
            cache.as_ref(),
            |guard| guard.clone(),
        )
        .await
        {
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

    if let Some(vector_ref) = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_VECTOR_STORE,
        vector_store.as_ref(),
        |guard| guard.clone(),
    )
    .await
    {
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

    with_acp_lock_async(
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
    )
    .await;
    Ok(result)
}

/// Perform health check cycle
#[allow(clippy::too_many_arguments)]
pub async fn perform_health_check_cycle(
    lock_monitor: Arc<AcpLockMonitor>,
    memory_cache: Arc<TokioMutex<MemoryResponseCache>>,
    cache: Arc<TokioMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<TokioMutex<Option<Arc<VectorStore>>>>,
    circuit_breakers: Arc<TokioMutex<CircuitBreakerRegistry>>,
    phase_rate_limiter: Arc<TokioMutex<PhaseRateLimiter>>,
    inflight_limiter: Arc<TokioMutex<InflightLimiter>>,
    lifecycle: Arc<TokioMutex<LifecycleState>>,
    maintenance: Arc<TokioMutex<MaintenanceTracker>>,
) -> Result<()> {
    let memory_health = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_MEMORY_CACHE,
        memory_cache.as_ref(),
        |cache| {
            // active_entries returns the number of non-expired entries.
            // This also verifies the lock is accessible and not poisoned.
            let entries = cache.active_entries();
            debug!("memory cache health: {} active entries", entries);
            entries > 0
        },
    )
    .await;
    if !memory_health {
        debug!("memory cache is empty (0 active entries)");
    }

    let sqlite_health = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_RESPONSE_CACHE,
        cache.as_ref(),
        |guard| guard.clone(),
    )
    .await
    .map(|cache| cache.entry_count().is_ok())
    .unwrap_or_else(|| {
        warn!("sqlite health: lock returned None, assuming healthy");
        true
    });

    let vector_health = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_VECTOR_STORE,
        vector_store.as_ref(),
        |guard| guard.clone(),
    )
    .await
    .map(|store| store.memory_entry_count().is_ok() && store.summary_entry_count().is_ok())
    .unwrap_or_else(|| {
        warn!("vector health: lock returned None, assuming healthy");
        true
    });

    // Check circuit breakers
    let circuit_breaker_health = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_CIRCUIT_BREAKERS,
        circuit_breakers.as_ref(),
        |guard| guard.is_healthy(),
    )
    .await;
    if !circuit_breaker_health {
        warn!("circuit breaker health check failed");
    }

    // Check rate limiters
    let phase_healthy = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_PHASE_RATE_LIMITER,
        phase_rate_limiter.as_ref(),
        |guard| guard.is_healthy(),
    )
    .await;
    let inflight_healthy = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_INFLIGHT_LIMITER,
        inflight_limiter.as_ref(),
        |guard| guard.is_healthy(),
    )
    .await;
    let rate_limiter_health = phase_healthy && inflight_healthy;
    if !rate_limiter_health {
        warn!("rate limiter health check failed");
    }

    // Check lifecycle
    let lifecycle_health = with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_LIFECYCLE,
        lifecycle.as_ref(),
        |guard| guard.is_healthy(),
    )
    .await;
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
    with_acp_lock_async(
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
    )
    .await;

    // Update maintenance tracker
    with_acp_lock_async(
        lock_monitor.as_ref(),
        ACP_LOCK_MAINTENANCE,
        maintenance.as_ref(),
        |guard| guard.record_health_check(overall_health),
    )
    .await;

    Ok(())
}

/// Spawn a background task with panic detection.
///
/// Wraps `tokio::spawn` so that if the future panics, the error is
/// logged via `tracing::error!` instead of being silently swallowed.
/// Use this for all background tasks that should not go unobserved.
fn spawn_background_task<F>(future: F, task_name: &'static str)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let handle = tokio::spawn(future);
        if let Err(e) = handle.await {
            tracing::error!(
                target: "acp",
                task = task_name,
                "background task panicked: {:?}",
                e
            );
        }
    });
}

/// Start background tasks for an ACP server
pub async fn start_background_tasks(
    server: &super::server::AcpServer,
    shutdown_notify: Arc<Notify>,
) -> Result<()> {
    let lock_monitor = Arc::clone(&server.observability.lock_monitor);
    let runtime_config = Arc::new(tokio::sync::Mutex::new(server.runtime_config.clone()));
    let memory_cache = {
        let _inner = server
            .cache_deps
            .cache
            .memory_response_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // MemoryResponseCache doesn't impl Clone, but we can use Default
        // since the background loop only needs a fresh monitoring instance.
        let fresh = MemoryResponseCache::default();
        // _inner dropped here, releasing the lock
        Arc::new(tokio::sync::Mutex::new(fresh))
    };
    let memory_store = Arc::new(tokio::sync::Mutex::new(
        MemoryStore::new(Default::default()),
    ));

    let cache = Arc::new(tokio::sync::Mutex::new(
        server.cache_deps.cache.response_cache.clone(),
    ));
    let vector_store = Arc::new(tokio::sync::Mutex::new(
        server.cache_deps.cache.vector_store.clone(),
    ));

    let maintenance = Arc::new(tokio::sync::Mutex::new(MaintenanceTracker::new()));
    let lifecycle = Arc::new(tokio::sync::Mutex::new(LifecycleState::new()));
    let circuit_breakers = Arc::new(tokio::sync::Mutex::new(CircuitBreakerRegistry::default()));

    let phase_rate_limiter = Arc::new(tokio::sync::Mutex::new(PhaseRateLimiter::default()));
    let inflight_limiter = Arc::new(tokio::sync::Mutex::new(InflightLimiter::default()));

    let shutdown_notify_clone = shutdown_notify.clone();
    spawn_background_task(
        async move {
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
                shutdown_notify: shutdown_notify_clone,
            };
            run_background_maintenance_loop(bg_ctx).await;
        },
        "maintenance_loop",
    );

    // ── Metacognitive persistence (GAP-B53-56) ──────────────────────────
    // Save metacognitive state to disk every 60 seconds.
    if let Some(ref cb) = server.governance_deps.capability_bus {
        use crate::intelligence::metacognitive_persistence::MetacognitivePersistence;
        use std::path::PathBuf;
        let storage_dir = PathBuf::from(".goon/metacognitive");
        if let Ok(persistence) = MetacognitivePersistence::new(storage_dir) {
            // ── Cross-session state restoration (GAP-B53-56) ────────────
            // Restore any previously saved metacognitive state into the
            // controller so that corrective actions, observations, and
            // reflection reports survive process restarts.
            if persistence.has_saved_state() {
                match persistence.restore_into_controller(&cb.metacognitive) {
                    Ok(count) => {
                        tracing::info!(
                            target: "metacognitive_persistence",
                            restored_count = count,
                            "restored metacognitive state from previous session"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "metacognitive_persistence",
                            "failed to restore metacognitive state: {e}"
                        );
                    }
                }
            }

            let cb = Arc::clone(cb);
            let shutdown = shutdown_notify.clone();
            spawn_background_task(
                async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(60));
                    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    loop {
                        tokio::select! {
                            _ = shutdown.notified() => break,
                            _ = interval.tick() => {}
                        }
                        if let Err(e) = persistence.save(&cb.metacognitive) {
                            tracing::warn!(
                                target: "metacognitive_persistence",
                                "background save failed: {e}"
                            );
                        }
                    }
                },
                "metacognitive_persistence",
            );
        } else {
            tracing::warn!(
                target: "metacognitive_persistence",
                "failed to create persistence directory"
            );
        }
    }

    // ── mTLS certificate monitor (GAP-B52) ──────────────────────────────
    if server.runtime_config.mtls_enabled {
        let mtls_config = crate::security::mtls::MtlsConfig::new(
            server.runtime_config.mtls_ca_cert_path.clone(),
            server.runtime_config.mtls_server_cert_path.clone(),
            server.runtime_config.mtls_server_key_path.clone(),
        )
        .with_client_cert(server.runtime_config.mtls_require_client_cert)
        .with_allowed_cns(
            server
                .runtime_config
                .mtls_allowed_cns
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        );
        crate::security::mtls::spawn_cert_monitor_if_configured(Some(mtls_config));
    } else {
        crate::security::mtls::spawn_cert_monitor_if_configured(None);
    }

    // ── Security scanning background tasks (GAP-B52-24, GAP-B52-30) ─────

    // Schedule dependency vulnerability scan every 24 hours
    if let Some(ref scanner) = server.governance_deps.dependency_vulnerability_scanner {
        let scanner = Arc::clone(scanner);
        let advisor = server.governance_deps.security_advisor.clone();
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                // First tick fires immediately
                ticker.tick().await;

                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = ticker.tick() => {}
                    }

                    info!("Security scan: starting dependency vulnerability scan");
                    let result = scanner.scan(std::path::Path::new(".")).await;
                    if let Some(ref advisor) = advisor {
                        if let Err(e) = advisor.alert_from_dependency_scan(&result).await {
                            warn!("Failed to alert from dependency scan: {}", e);
                        }
                    }
                    info!(
                        "Security scan: dependency scan complete (vulnerabilities: {})",
                        result.total()
                    );
                }
            },
            "dependency_vulnerability_scan",
        );
    }

    // Schedule secret exposure scan every 1 hour
    if let Some(ref detector) = server.governance_deps.secret_exposure_detector {
        let detector = Arc::clone(detector);
        let advisor = server.governance_deps.security_advisor.clone();
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(60 * 60));
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                // Skip first tick, start after one interval
                ticker.tick().await;

                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = ticker.tick() => {}
                    }

                    info!("Security scan: starting secret exposure scan");
                    let result = detector.scan_directory(std::path::Path::new(".")).await;
                    if let Ok(ref scan_result) = result {
                        if let Some(ref advisor) = advisor {
                            if let Err(e) = advisor.alert_from_secret_scan(scan_result).await {
                                warn!("Failed to alert from secret scan: {}", e);
                            }
                        }
                        info!(
                            "Security scan: secret scan complete (matches: {})",
                            scan_result.total()
                        );
                    }
                }
            },
            "secret_exposure_scan",
        );
    }

    // Start security advisor daily digest schedule
    if let Some(ref advisor) = server.governance_deps.security_advisor {
        advisor.start_digest_schedule();
    }

    // BLUE56-D01: Policy reloader — check for policy file changes every 60 seconds (GAP-B58-D04)
    if let Some(ref reloader) = server.governance_deps.policy_reloader {
        let reloader = Arc::clone(reloader);
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(60));
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = ticker.tick() => {
                            if let Ok(mut guard) = reloader.lock() {
                                guard.reload_all();
                            }
                            debug!("Policy reloader: checked for policy updates");
                        }
                    }
                }
            },
            "policy_reloader",
        );
    } else {
        tracing::warn!("Policy reloader: no shared reloader available, skipping background task");
    }

    // BLUE56-D02: Process timeouts — spawn the full timeout loop
    // (which also runs timeout checks and processes approval engine timeouts)
    {
        let approval_engine = server.governance_deps.approval_engine.clone();
        crate::governance::runtime_controls::spawn_timeout_loop(
            shutdown_notify.clone(),
            approval_engine,
        );
    }

    // ── Code quality scan every 5 minutes (GAP-B53-57) ─────────────────
    {
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(300));
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                // Skip first tick, start after one interval
                interval.tick().await;
                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = interval.tick() => {}
                    }
                    let report = tokio::task::spawn_blocking(move || {
                        crate::intelligence::code_quality::run_code_quality_scan()
                    })
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("code quality scan task failed: {}", e);
                        crate::intelligence::code_quality::CodeQualityReport {
                            issues: Vec::new(),
                            health_score: 1.0,
                            modules_scanned: 0,
                            scanned_at_ms: crate::intelligence::now_ms(),
                        }
                    });
                    tracing::info!(
                        target: "intelligence",
                        health_score = report.health_score,
                        modules_scanned = report.modules_scanned,
                        issues = report.issues.len(),
                        "code quality scan complete"
                    );
                }
            },
            "code_quality_scan",
        );
    }

    // ── Metacognitive auto-reflexion every 30 seconds (BLUE56-B10) ───────
    {
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut interval = tokio::time::interval(Duration::from_millis(30_000));
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = interval.tick() => {}
                    }
                    let report_ids =
                        crate::intelligence::metacognitive::global_metacognitive_controller()
                            .autoreflect();
                    if !report_ids.is_empty() {
                        tracing::info!(
                            target: "intelligence",
                            count = report_ids.len(),
                            "Metacognitive auto-reflexion generated reports"
                        );
                    }
                }
            },
            "metacognitive_autoreflect",
        );
    }

    // ── SelfEvolutionAgent + EvolutionLoop (BLUE56-B03) ───────────────
    {
        let shutdown = shutdown_notify.clone();
        // Clone the real AlertManager reference so the evolution loop can
        // poll active alerts as evolution triggers (GAP-B52-02 I11).
        let alert_manager = Arc::clone(&server.observability.alert_manager);
        spawn_background_task(
            async move {
                // The evolution_agent binding lives for the entire async block scope,
                // so the agent is held alive until shutdown is notified (GAP-B58-C02/C04).
                let evolution_agent = Arc::new(
                    crate::agents::self_evolution_agent::SelfEvolutionAgent::new(
                        std::path::PathBuf::from("."),
                        Vec::new(),
                    )
                    .await,
                );

                let workdir = std::path::PathBuf::from(".goon/evolution");
                let mut evolution_loop =
                    crate::orchestration::self_evolution::evolution_loop::EvolutionLoop::new(workdir)
                        .with_default_trigger_sources()
                        .with_alert_manager(alert_manager)
                        .with_agent(evolution_agent)
                        .with_approval_mode(
                            crate::orchestration::self_evolution::evolution_loop::ApprovalMode::AutoApproval,
                        );

                tracing::info!(
                    target: "intelligence",
                    "SelfEvolutionAgent instantiated, EvolutionLoop starting"
                );

                // Bridge TripleFusion triggers into the EvolutionLoop via pubsub
                let rx = init_fusion_evolution_bridge();
                evolution_loop = evolution_loop.with_trigger_source(Box::new(
                    PubsubTriggerSource::new("fusion_evolution".to_string(), rx),
                ));

                // Run evolution loop until shutdown
                tokio::select! {
                    _ = shutdown.notified() => {
                        tracing::info!(target: "intelligence", "EvolutionLoop shutting down");
                    }
                    result = evolution_loop.run() => {
                        if let Err(e) = result {
                            tracing::warn!(
                                target: "intelligence",
                                error = %e,
                                "EvolutionLoop exited with error, agent will be dropped"
                            );
                        }
                    }
                }
            },
            "self_evolution_agent",
        );
    }

    // ── BLUE56-GAP-C04: Hyper-resilience health checks ─────────────────
    // Start background health checks for circuit breaker self-healing.
    // The health check interval is configured in ResilienceConfig.
    server
        .resilience
        .hyper_resilience
        .start_health_checks()
        .await;
    tracing::info!(
        target: "resilience",
        "HyperResilienceEngine health checks started"
    );

    // ── Fault tolerance recovery cycle (F-GAP-28) ───────────────────-
    // Periodically check heartbeats, detect failed nodes, create recovery
    // plans, and attempt automatic reintegration every 30 seconds.
    if let Some(ref harness_bus) = server.governance_deps.harness_bus {
        let ft = Arc::clone(&harness_bus.fault_tolerance);
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                // Skip first tick to give startup time
                interval.tick().await;
                loop {
                    tokio::select! {
                        _ = shutdown.notified() => {
                            tracing::info!(target: "fault_tolerance", "recovery cycle shutting down");
                            break;
                        }
                        _ = interval.tick() => {
                            let summary = ft.run_recovery_cycle().await;
                            if !summary.offenders.is_empty() || summary.plans_created > 0 {
                                tracing::info!(
                                    target: "fault_tolerance",
                                    offenders = summary.offenders.len(),
                                    plans_created = summary.plans_created,
                                    plans_activated = summary.plans_activated,
                                    cluster_health = ?summary.cluster_health,
                                    "fault tolerance recovery cycle complete"
                                );
                            }
                        }
                    }
                }
            },
            "fault_tolerance_recovery",
        );
        tracing::info!(
            target: "fault_tolerance",
            "FaultToleranceEngine recovery cycle started (interval=30s)"
        );
    } else {
        tracing::warn!(
            target: "fault_tolerance",
            "harness_bus is None — fault tolerance recovery cycle not started"
        );
    }

    // ── Metrics bridge (P5-6): periodically sync OTLP MetricsRecorder → RuntimeMetrics ──
    {
        let runtime_metrics = server.observability.metrics.clone();
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(15));
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = interval.tick() => {}
                    }
                    bridge_metrics_recorder(&runtime_metrics, global_metrics_recorder());
                }
            },
            "metrics_bridge",
        );
        tracing::debug!(
            target: "acp",
            "metrics bridge background task started (interval=15s)"
        );
    }

    Ok(())
}

/// Run a single maintenance cycle on demand
pub async fn run_maintenance_cycle(
    server: &super::server::AcpServer,
) -> Result<MaintenanceCycleResult> {
    let lock_monitor = Arc::clone(&server.observability.lock_monitor);
    let runtime_config = Arc::new(tokio::sync::Mutex::new(server.runtime_config.clone()));
    let memory_cache = {
        let _inner = server
            .cache_deps
            .cache
            .memory_response_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let fresh = MemoryResponseCache::default();
        // _inner dropped here, releasing the lock
        Arc::new(tokio::sync::Mutex::new(fresh))
    };
    let memory_store = Arc::new(tokio::sync::Mutex::new(
        MemoryStore::new(Default::default()),
    ));

    let cache = Arc::new(tokio::sync::Mutex::new(
        server.cache_deps.cache.response_cache.clone(),
    ));
    let vector_store = Arc::new(tokio::sync::Mutex::new(
        server.cache_deps.cache.vector_store.clone(),
    ));

    let maintenance = Arc::new(tokio::sync::Mutex::new(MaintenanceTracker::new()));

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
    let memory_cache = {
        let _inner = server
            .cache_deps
            .cache
            .memory_response_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let fresh = MemoryResponseCache::default();
        // _inner dropped here, releasing the lock
        Arc::new(tokio::sync::Mutex::new(fresh))
    };

    let cache = Arc::new(tokio::sync::Mutex::new(
        server.cache_deps.cache.response_cache.clone(),
    ));
    let vector_store = Arc::new(tokio::sync::Mutex::new(
        server.cache_deps.cache.vector_store.clone(),
    ));

    let circuit_breakers = Arc::new(tokio::sync::Mutex::new(CircuitBreakerRegistry::default()));
    let lifecycle = Arc::new(tokio::sync::Mutex::new(LifecycleState::new()));
    let maintenance = Arc::new(tokio::sync::Mutex::new(MaintenanceTracker::new()));

    let phase_rate_limiter = Arc::new(tokio::sync::Mutex::new(PhaseRateLimiter::default()));
    let inflight_limiter = Arc::new(tokio::sync::Mutex::new(InflightLimiter::default()));

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
