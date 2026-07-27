//! ACP Background Tasks - Background task management
//!
//! This module contains background task implementations for the ACP server,
//! including maintenance cycles, health checks, and periodic operations.

use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::Notify;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::intelligence::fusion_evolution_bridge::init_fusion_evolution_bridge;
use crate::memory_module::MemoryStore;
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::metrics_exporter::bridge_metrics_recorder;
use crate::observability::telemetry_enhanced::global_metrics_recorder;
use crate::orchestration::self_evolution::evolution_loop::PubsubTriggerSource;

use super::prelude::{with_acp_lock_async, MaintenanceTracker};

/// Maintenance cycle result
#[derive(Debug, Default, Clone, Copy)]
pub struct MaintenanceCycleResult {
    /// Number of expired entries removed from memory cache
    pub memory_expired_removed: usize,
    /// Whether runtime memory store GC was executed
    pub memory_store_gc_ran: bool,
}

/// Shared context for background maintenance operations.
///
/// Groups all shared state handles needed by the background maintenance loop
/// into a single struct, eliminating the previous 12-parameter function signature.
#[derive(Debug)]
pub struct BackgroundContext {
    pub memory_cache: Arc<StdMutex<MemoryResponseCache>>,
    pub memory_store: Arc<tokio::sync::Mutex<MemoryStore>>,
    pub maintenance: Arc<tokio::sync::Mutex<MaintenanceTracker>>,
    pub shutdown_notify: Arc<Notify>,
}

/// Shared BackgroundContext populated once by `start_background_tasks`.
/// Used by one-shot `run_maintenance_cycle` and `run_health_check` to avoid
/// creating duplicate lock and state instances for each call.
static SHARED_BG_CTX: OnceLock<BackgroundContext> = OnceLock::new();

/// Run background maintenance loop
///
/// This function runs periodic memory maintenance tasks.
pub async fn run_background_maintenance_loop(ctx: BackgroundContext) {
    // Run maintenance every 60 seconds to purge expired cache entries
    let mut maintenance_interval = interval(Duration::from_secs(60));

    maintenance_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ctx.shutdown_notify.notified() => break,
            _ = maintenance_interval.tick() => {
                if let Err(err) = perform_maintenance_cycle(
                    ctx.memory_cache.clone(),
                    ctx.memory_store.clone(),
                    ctx.maintenance.clone(),
                    "background",
                ).await {
                    warn!("background maintenance cycle failed: {}", err);
                }
            }
        }
    }

    info!("background maintenance loop stopped");
}

/// Perform maintenance cycle
pub async fn perform_maintenance_cycle(
    memory_cache: Arc<StdMutex<MemoryResponseCache>>,
    memory_store: Arc<TokioMutex<MemoryStore>>,
    maintenance: Arc<TokioMutex<MaintenanceTracker>>,
    source: &str,
) -> Result<MaintenanceCycleResult> {
    let _ = with_acp_lock_async(maintenance.as_ref(), |guard| guard.note_started()).await;

    let mut result = MaintenanceCycleResult {
        memory_expired_removed: {
            let guard = memory_cache.lock().unwrap_or_else(|e| e.into_inner());
            guard.purge_expired()
        },
        ..MaintenanceCycleResult::default()
    };

    debug!(
        "{}: cleaned {} expired entries from memory cache",
        source, result.memory_expired_removed
    );

    if with_acp_lock_async(memory_store.as_ref(), |guard| {
        guard.gc();
        true
    })
    .await
    {
        result.memory_store_gc_ran = true;
    }

    with_acp_lock_async(maintenance.as_ref(), |guard| {
        guard.note_completed(result.memory_expired_removed);
    })
    .await;
    Ok(result)
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
        // We must spawn future in a separate task to catch panics.
        // JoinHandle<Result<()>> from spawn captures panics as Err.
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
    // Share the live memory_response_cache so background GC actually
    // purges expired entries from the cache serving live requests.
    let memory_cache = server.cache_deps.cache.memory_response_cache.clone();
    let memory_store = Arc::new(tokio::sync::Mutex::new(
        MemoryStore::new(Default::default()),
    ));

    let maintenance = Arc::new(tokio::sync::Mutex::new(MaintenanceTracker::new()));
    let shutdown_notify_clone = shutdown_notify.clone();

    // Store shared context for reuse by one-shot maintenance/health-check calls.
    let ctx = BackgroundContext {
        memory_cache: memory_cache.clone(),
        memory_store: memory_store.clone(),
        maintenance: maintenance.clone(),
        shutdown_notify: shutdown_notify.clone(),
    };
    let _ = SHARED_BG_CTX.set(ctx);

    spawn_background_task(
        async move {
            let bg_ctx = BackgroundContext {
                memory_cache,
                memory_store,
                maintenance,
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
                            _ = shutdown.notified() => {
                                // Clean up snapshot on graceful shutdown.
                                if let Err(e) = persistence.clear() {
                                    tracing::warn!(
                                        target: "metacognitive_persistence",
                                        "failed to clear metacognitive snapshot: {e}"
                                    );
                                }
                                break;
                            }
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
    // Certificate monitor task removed — MtlsConfig, start_cert_monitor, and
    // spawn_cert_monitor_if_configured were dead code (F-GAP-49 reserved mTLS).
    // TODO: Wire mTLS certificate monitoring (planned feature). The mtls_enabled
    // config flag exists but the cert-monitoring background task has not yet been
    // implemented. When implementing, consider polling the cert file's mtime and
    // triggering a graceful TLS acceptor reload on change.

    // ── Security scanning background tasks (GAP-B52-24, GAP-B52-30) ─────
    // S6 startup optimization: delay security scans by 500ms so the server
    // can start accepting requests first. These run every 24h/1h anyway.

    // Schedule dependency vulnerability scan every 24 hours
    if let Some(ref scanner) = server.governance_deps.dependency_vulnerability_scanner {
        let scanner = Arc::clone(scanner);
        let advisor = server.governance_deps.security_advisor.clone();
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                // S6: delay 500ms to let the server start accepting requests first
                tokio::time::sleep(Duration::from_millis(500)).await;

                let mut ticker = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                // First tick fires immediately (after the 500ms sleep)
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
                // S6: delay 500ms to let the server start accepting requests first
                tokio::time::sleep(Duration::from_millis(500)).await;

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
        // Pull timeout config from harness_bus evaluator if available
        let (pending_count, timeout_secs) = server
            .governance_deps
            .harness_bus
            .as_ref()
            .map(|hb| {
                let secs = hb.evaluator.dispatch.timeout_policy.max_timeout.as_secs();
                (Some(1usize), Some(secs))
            })
            .unwrap_or_else(|| (Some(1usize), Some(300u64)));

        crate::governance::runtime_controls::spawn_timeout_loop(
            shutdown_notify.clone(),
            approval_engine,
            pending_count,
            timeout_secs,
        );
    }

    // ── Code quality scan every 5 minutes (GAP-B53-57) ─────────────────
    // S6 startup optimization: delay 500ms to let server accept requests first.
    {
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                // S6: delay 500ms to let the server start accepting requests first
                tokio::time::sleep(Duration::from_millis(500)).await;

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
    // S6 startup optimization: delay 500ms to let server accept requests first.
    {
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                // S6: delay 500ms to let the server start accepting requests first
                tokio::time::sleep(Duration::from_millis(500)).await;

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
    // S6 startup optimization: delay 500ms to let server accept requests first.
    {
        let shutdown = shutdown_notify.clone();
        // Clone the real AlertManager reference so the evolution loop can
        // poll active alerts as evolution triggers (GAP-B52-02 I11).
        let alert_manager = Arc::clone(&server.observability.alert_manager);
        spawn_background_task(
            async move {
                // S6: delay 500ms to let the server start accepting requests first
                tokio::time::sleep(Duration::from_millis(500)).await;

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
                match init_fusion_evolution_bridge() {
                    Ok(rx) => {
                        evolution_loop = evolution_loop.with_trigger_source(Box::new(
                            PubsubTriggerSource::new("fusion_evolution".to_string(), rx),
                        ));
                    }
                    Err(e) => {
                        warn!("fusion_evolution_bridge: {}", e);
                    }
                }

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

    // ── Memory bridge: auto-migrate every 5 minutes (PERF-FIX: moved from new_acp_server) ──
    // Uses the server's lazy MemoryPersistence (S1 startup optimization) instead of
    // creating a redundant third SQLite connection during the critical startup path.
    if let Some(mp) = server.get_or_init_memory_persistence() {
        let mp = Arc::clone(&mp);
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 min
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                // Skip first tick to give startup time
                interval.tick().await;
                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = interval.tick() => {}
                    }
                    match mp.auto_migrate().await {
                        Ok(report) => {
                            let total = report.promoted_hot_to_warm
                                + report.promoted_warm_to_cold
                                + report.demoted_hot_to_cold
                                + report.evicted_warm;
                            if total > 0 {
                                tracing::debug!(
                                    target = "memory_bridge",
                                    promoted_hot_to_warm = report.promoted_hot_to_warm,
                                    promoted_warm_to_cold = report.promoted_warm_to_cold,
                                    demoted_hot_to_cold = report.demoted_hot_to_cold,
                                    evicted_warm = report.evicted_warm,
                                    "memory_persistence auto_migrate cycle complete"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                target = "memory_bridge",
                                "memory_persistence auto_migrate failed: {e}"
                            );
                        }
                    }
                }
            },
            "memory_auto_migrate",
        );
        tracing::info!(
            target = "memory_bridge",
            "memory persistence auto-migrate background task started (interval=300s)"
        );
    }

    // ── Memory bridge: initial promote on startup (PERF-FIX: was only in stdio mode) ──
    // GAP-B58-B13: Wire memory bridge — run initial promotion on startup.
    // This was previously only called in run_acp_server (stdio mode).
    // Now it runs for ALL protocol modes (HTTP, WebSocket, etc.).
    // S6 startup optimization: delay 500ms so the server can accept requests first.
    if let Some(mp) = server.get_or_init_memory_persistence() {
        let memory_store = Arc::clone(&server.persistence.memory_store);
        let mp = Arc::clone(&mp);
        tokio::spawn(async move {
            // S6: defer initial promote by 500ms to let server accept requests first
            tokio::time::sleep(Duration::from_millis(500)).await;
            match crate::memory::memory_bridge::bridge_promote(&memory_store, &mp).await {
                Ok(report) => {
                    if report.promoted_count > 0 {
                        tracing::info!(
                            "memory bridge: initial promote moved {} entries",
                            report.promoted_count
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("memory bridge: initial bridge_promote failed: {e}");
                }
            }
        });
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

/// Run a single maintenance cycle on demand.
///
/// Uses the shared BackgroundContext (populated by `start_background_tasks`)
/// to avoid creating duplicate lock and state instances.
pub async fn run_maintenance_cycle(
    _server: &super::server::AcpServer,
) -> Result<MaintenanceCycleResult> {
    // Use shared context if available, otherwise fall back to creating fresh state.
    if let Some(ctx) = SHARED_BG_CTX.get() {
        return perform_maintenance_cycle(
            ctx.memory_cache.clone(),
            ctx.memory_store.clone(),
            ctx.maintenance.clone(),
            "manual",
        )
        .await;
    }

    // Fallback: create fresh state (before start_background_tasks is called).
    let memory_cache = Arc::new(StdMutex::new(MemoryResponseCache::default()));
    let memory_store = Arc::new(tokio::sync::Mutex::new(
        MemoryStore::new(Default::default()),
    ));
    let maintenance = Arc::new(tokio::sync::Mutex::new(MaintenanceTracker::new()));

    perform_maintenance_cycle(memory_cache, memory_store, maintenance, "manual").await
}

/// Run a single health check on demand.
///
/// Verifies the server is responsive by checking basic subsystem health.
pub async fn run_health_check(server: &super::server::AcpServer) -> Result<()> {
    // Verify governance deps are accessible
    let _pua = &server.governance_deps.pua_enforcement_plan;
    // Verify runtime config is accessible
    let _config = &server.runtime_config;
    // Verify agent registry is accessible
    let _agents = &server.model_deps.agent_registry;
    // Verify core deps
    if server.governance_deps.harness_bus.is_none() {
        anyhow::bail!("health.check: harness_bus is not configured");
    }
    if server
        .orchestration_deps
        .skill_registry
        .read()
        .map(|r| r.list().len())
        .unwrap_or(0)
        == 0
    {
        tracing::warn!("health.check: skill registry is empty");
    }
    Ok(())
}
