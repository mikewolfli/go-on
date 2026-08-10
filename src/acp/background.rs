//! ACP Background Tasks - Background task management
//!
//! This module contains background task implementations for the ACP server,
//! including maintenance cycles, health checks, and periodic operations.

use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

use crate::intelligence::fusion_evolution_bridge::init_fusion_evolution_bridge;
use crate::memory::semantic_cache::SemanticResponseCache;
use crate::memory_module::MemoryStore;
use crate::orchestration::self_evolution::evolution_loop::PubsubTriggerSource;

use super::prelude::{with_acp_lock, with_acp_lock_async, MaintenanceTracker};

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
/// Groups all shared state handles needed by the on-demand maintenance cycle
/// into a single struct, eliminating the previous 12-parameter function signature.
#[derive(Debug)]
pub struct BackgroundContext {
    /// The server's live semantic cache — expired-entry purging runs against
    /// the same cache instance serving live requests.
    pub semantic_cache: Arc<std::sync::RwLock<SemanticResponseCache>>,
    /// The server's live memory store (GC must run on the store request paths
    /// actually write to, not on a fresh empty instance).
    pub memory_store: Arc<StdMutex<MemoryStore>>,
    pub maintenance: Arc<tokio::sync::Mutex<MaintenanceTracker>>,
}

/// Shared BackgroundContext populated once by `start_background_tasks`.
/// Used by one-shot `run_maintenance_cycle` to avoid creating duplicate lock
/// and state instances for each call.
static SHARED_BG_CTX: OnceLock<BackgroundContext> = OnceLock::new();

// NOTE: The 60-second background maintenance loop was removed.
// `MemoryResponseCache::purge_expired()` now delegates to the semantic cache
// (real expired-entry removal), and the loop GC'd a freshly-created empty
// MemoryStore that no request path writes to — the loop ran forever doing
// nothing but taking locks. The on-demand `run_maintenance_cycle()` below
// remains for the health/lifecycle APIs.

/// Perform maintenance cycle
pub async fn perform_maintenance_cycle(
    semantic_cache: Arc<std::sync::RwLock<SemanticResponseCache>>,
    memory_store: Arc<StdMutex<MemoryStore>>,
    maintenance: Arc<TokioMutex<MaintenanceTracker>>,
    source: &str,
) -> Result<MaintenanceCycleResult> {
    let _ = with_acp_lock_async(maintenance.as_ref(), |guard| guard.note_started()).await;

    let memory_expired_removed = semantic_cache
        .write()
        .map(|guard| guard.purge_expired())
        .unwrap_or(0);
    let mut result = MaintenanceCycleResult {
        memory_expired_removed,
        ..MaintenanceCycleResult::default()
    };

    debug!(
        "{}: cleaned {} expired entries from memory cache",
        source, result.memory_expired_removed
    );

    // GC the live memory store (the one request paths write to).
    result.memory_store_gc_ran = with_acp_lock(memory_store.as_ref(), |guard| {
        guard.gc();
        true
    });

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
    // Share the live semantic cache so background GC actually
    // purges expired entries from the cache serving live requests.
    let semantic_cache = server.cache_deps.cache.semantic_cache.clone();
    // GC the server's live memory store (previously a fresh empty MemoryStore
    // was created here and GC'd — the health/lifecycle APIs reported "GC ran"
    // while doing nothing).
    let memory_store = Arc::clone(&server.persistence.memory_store);

    let maintenance = Arc::new(tokio::sync::Mutex::new(MaintenanceTracker::new()));
    // Store shared context for reuse by one-shot maintenance/health-check calls.
    let ctx = BackgroundContext {
        semantic_cache,
        memory_store,
        maintenance,
    };
    let _ = SHARED_BG_CTX.set(ctx);

    // ── Metacognitive persistence (GAP-B53-56) ──────────────────────────
    // Save metacognitive state to disk every 60 seconds.
    if let Some(ref cb) = server.governance_deps.capability_bus {
        use crate::intelligence::metacognitive_persistence::MetacognitivePersistence;
        let storage_dir = crate::shared::goon_paths::goon_subdir("metacognitive");
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
                                // Preserve the snapshot on graceful shutdown so
                                // cross-session state survives clean restarts
                                // (GAP-B53-56). Previously clear() deleted the
                                // snapshot on clean shutdown, making restore
                                // only work after crashes — the opposite of
                                // intent. The periodic save above already
                                // persisted the latest state.
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
    // Certificate expiry monitoring is implemented and wired in
    // `security::wire_cert_monitor` (called from server_builder::wire_server):
    // initial check at startup + daily re-check of the server cert's
    // `not_after` with expiry/soon-to-expire warnings. The older
    // MtlsConfig/start_cert_monitor symbols were removed as dead code.

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

    // Policy hot-reload (BLUE56-D01) was removed in the 2026-08-06 cleanup:
    // `PolicyReloader`/`reloadable_policy` had no production caller — no reload
    // loop ran and the evaluator's `policy_reloader` was always None. See
    // log-20260730-18 for the original risk analysis.

    // BLUE56-D02 approval-timeout loop was removed in the 2026-08-06 cleanup:
    // the HITL ApprovalEngine (and its preference learner) had zero production
    // callers — ACP approvals flow through session/request_permission instead —
    // so the engine and its 5s `spawn_timeout_loop` were deleted together.

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
    // Gated on `runtime.evolution_enabled` (default false): the loop can
    // auto-apply LLM-generated patches to the project source, so it must not
    // run unless explicitly opted in. When enabled, the sandbox whitelist
    // (src/**/*.rs) and AutoApproval mode apply as before.
    // S6 startup optimization: delay 500ms to let server accept requests first.
    if server.runtime_config.evolution_enabled {
        let shutdown = shutdown_notify.clone();
        // Clone the real AlertManager reference so the evolution loop can
        // poll active alerts as evolution triggers (GAP-B52-02 I11).
        let alert_manager = Arc::clone(&server.observability.alert_manager);
        // Wire a real LLM agent (and registry handle) into the evolution agent
        // so patch generation and analysis use the LLM path instead of pure
        // heuristic synthesis (previously `llm_agent` was always None).
        let evolution_registry = server.agent_registry();
        spawn_background_task(
            async move {
                // S6: delay 500ms to let the server start accepting requests first
                tokio::time::sleep(Duration::from_millis(500)).await;

                let llm_agent = evolution_registry.as_ref().and_then(|registry| {
                    registry
                        .get("assistant")
                        .or_else(|| registry.get("summarizer"))
                        .or_else(|| {
                            let names = registry.names();
                            names.first().and_then(|n| registry.get(n))
                        })
                });

                // The evolution_agent binding lives for the entire async block scope,
                // so the agent is held alive until shutdown is notified (GAP-B58-C02/C04).
                let evolution_agent = Arc::new(
                    crate::agents::self_evolution_agent::SelfEvolutionAgent::with_llm(
                        std::path::PathBuf::from("."),
                        Vec::new(),
                        llm_agent,
                    )
                    .await,
                );

                // The evolution agent analyses the real project root (with_llm
                // uses "."), so the sandbox workdir must point at the same
                // root. Previously the workdir was ".goon/evolution": patches
                // were applied to a directory that never existed (every apply
                // failed with IoError), verify() ran `cargo build` in a
                // directory without its own Cargo.toml (so cargo walked up to
                // the real project — verifying the unpatched code), and
                // EvolutionHistory::new(workdir) produced the doubly-nested
                // path ".goon/evolution/.goon/evolution/history.ndjson".
                let workdir = std::path::PathBuf::from(".");
                // Wire sandbox + history so the verify/apply/record phases of
                // the evolution cycle are real (previously `sandbox: None` made
                // verify/apply fail and `history: None` silently skipped recording).
                let evolution_loop = crate::orchestration::self_evolution::evolution_loop::EvolutionLoop::new(
                    workdir.clone(),
                )
                .with_default_trigger_sources()
                .with_alert_manager(alert_manager)
                .with_agent(evolution_agent)
                .with_sandbox(
                    crate::orchestration::self_evolution::sandbox::SandboxExecutor::new(
                        workdir.clone(),
                        3,
                    )
                    // Only Rust source files under src/ are patchable. This is
                    // the safety whitelist for the auto-approval evolution loop
                    // running against the real project root: configs, scripts,
                    // generated files and non-Rust sources are never touched.
                    .with_allowed_targets(vec!["src/**/*.rs".to_string()]),
                )
                .with_history(
                    crate::orchestration::self_evolution::evolution_history::EvolutionHistory::new(
                        crate::shared::goon_paths::goon_data_dir(),
                    )
                    .await,
                )
                .with_approval_mode(
                    crate::orchestration::self_evolution::evolution_loop::ApprovalMode::AutoApproval,
                );

                let mut evolution_loop = evolution_loop;

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
    //
    // Engine boundary (dual-engine design, see the fault-tolerance loop
    // below): hyper_resilience owns circuit-breaker / degradation health
    // (probe → half-open → self-heal) on a 5s cycle; fault_tolerance owns
    // node heartbeat / recovery-plan orchestration on a 10s cycle. The two
    // engines are deliberately kept separate and do not drive each other.
    server
        .resilience
        .hyper_resilience
        .start_health_checks()
        .await;
    tracing::info!(
        target: "resilience",
        "HyperResilienceEngine health checks started"
    );

    // ── Fault-tolerance recovery cycle (detection + auto-recovery) ──
    // Schedule the engine's full recovery cycle on an interval so node
    // heartbeats, recovery plans, and auto-recovery are exercised in
    // production (previously the cycle only ran in tests). Real nodes
    // (e.g. hub workers) register via FaultToleranceEngine directly; the
    // evolve cycle no longer registers itself as a fake heartbeat node
    // (see CapabilityBus::evolve).
    //
    // Engine boundary (dual-engine design, see the hyper-resilience health
    // check above): fault_tolerance owns node heartbeats + recovery plans
    // (10s cycle); hyper_resilience owns circuit-breaker / degradation
    // health (5s cycle). Both run concurrently; neither drives the other.
    if let Some(ref harness) = server.governance_deps.harness_bus {
        let ft = Arc::clone(&harness.fault_tolerance);
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                // Skip the first tick to let the server start accepting requests.
                // 10s interval = 2/3 of the 15s heartbeat timeout. Liveness
                // signals come from real execution outcomes (task.rs reports
                // heartbeats on success, faults on failure); a node that was
                // registered but never reported is skipped by check_heartbeats
                // (has_reported=false), so idle agents are not falsely flagged
                // Offline.
                let mut interval = tokio::time::interval(Duration::from_secs(10));
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                interval.tick().await;
                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = interval.tick() => {}
                    }
                    // Detection + recovery only. Heartbeats are no longer
                    // self-reported here: the primary heartbeat/fault signal
                    // now comes from real execution (execute_single_subtask
                    // reports report_heartbeat on success and report_fault on
                    // failure; see src/acp/impl/request/exec_pack/task.rs).
                    // This loop runs the recovery cycle so a node whose real
                    // heartbeats stop (e.g. a hung worker) is detected Offline
                    // and auto-recovered.
                    let summary = ft.run_recovery_cycle().await;
                    if summary.offenders.is_empty() && summary.plans_created == 0 {
                        continue;
                    }
                    tracing::info!(
                        target: "fault_tolerance",
                        offenders = summary.offenders.len(),
                        plans_created = summary.plans_created,
                        plans_activated = summary.plans_activated,
                        "Fault-tolerance recovery cycle executed"
                    );
                }
            },
            "fault_tolerance_recovery",
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
    if server.get_or_init_memory_persistence().is_some() {
        let memory_store = Arc::clone(&server.persistence.memory_store);
        tokio::spawn(async move {
            // S6: defer initial promote by 500ms to let server accept requests first
            tokio::time::sleep(Duration::from_millis(500)).await;
            match crate::memory::memory_bridge::bridge_promote(&memory_store).await {
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

    // ── Alert rule producers (metrics-based rules) ───────────────────────
    // Evaluate the latency / error-rate / circuit-breaker / agent-timeout
    // alert rules on a 30s cadence. Previously these four rules had no
    // `evaluate` producer, so their alerts could never fire. Memory and
    // cache-hit rules are produced elsewhere (memory_health monitor and
    // chat/session cache-hit reporting).
    {
        let alert_manager = Arc::clone(&server.observability.alert_manager);
        let metrics = Arc::clone(&server.observability.metrics);
        let hyper_resilience = Arc::clone(&server.resilience.hyper_resilience);
        let lifecycle_state = Arc::clone(&server.resilience.lifecycle_state);
        let shutdown = shutdown_notify.clone();
        spawn_background_task(
            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = shutdown.notified() => break,
                        _ = interval.tick() => {}
                    }
                    let snapshot = metrics.snapshot();
                    // Runtime memory degradation: the memory alert rules were
                    // previously evaluated only once at startup, so a server
                    // that later leaked memory never triggered memory_* alerts.
                    // query_system_memory does blocking I/O (reads /proc/meminfo,
                    // spawns sysctl/vm_stat on macOS), so run it off the async
                    // worker on a blocking thread.
                    tokio::task::spawn_blocking(
                        crate::observability::memory_health::evaluate_memory_alerts,
                    )
                    .await
                    .ok();
                    // Circuit-breaker open count: the canonical source is
                    // HyperResilienceEngine (same one the Prometheus exporter
                    // reads); the RuntimeMetrics snapshot never owns this
                    // signal. Read it before taking the alert-manager lock so
                    // the non-Send guard is not held across the await.
                    let resilience_profile = hyper_resilience.profile().await;
                    // Real lifecycle health: open circuit breakers are a
                    // genuine degradation signal. Previously `is_healthy` was
                    // hard-coded true at construction and never updated.
                    let open_circuits = resilience_profile.open_circuits;
                    if let Ok(mut lc) = lifecycle_state.write() {
                        lc.set_healthy(open_circuits == 0);
                    }
                    let mut mgr = alert_manager.lock().unwrap_or_else(|poisoned| {
                        tracing::warn!("alert_manager lock poisoned");
                        poisoned.into_inner()
                    });
                    // Latency rule (avg request latency as the p95 proxy;
                    // the histogram would need a full percentile calculation).
                    mgr.evaluate("p95_latency_high", snapshot.avg_request_duration_ms);
                    // Error-rate rule (% of failed requests).
                    let error_rate = if snapshot.total_requests > 0 {
                        snapshot.failed_requests as f64 / snapshot.total_requests as f64 * 100.0
                    } else {
                        0.0
                    };
                    mgr.evaluate("error_rate_high", error_rate);
                    // Circuit-breaker open count rule.
                    mgr.evaluate("circuit_breaker_open", open_circuits as f64);
                    // Agent timeout-rate rule (% of requests that timed out).
                    let timeout_rate = if snapshot.total_requests > 0 {
                        snapshot.agent_timeout_failures_total as f64
                            / snapshot.total_requests as f64
                            * 100.0
                    } else {
                        0.0
                    };
                    mgr.evaluate("agent_timeout_rate", timeout_rate);
                }
            },
            "alert_metric_rules",
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
            ctx.semantic_cache.clone(),
            ctx.memory_store.clone(),
            ctx.maintenance.clone(),
            "manual",
        )
        .await;
    }

    // Fallback: create fresh state (before start_background_tasks is called).
    let semantic_cache = Arc::new(std::sync::RwLock::new(SemanticResponseCache::new(
        Default::default(),
    )));
    let memory_store = Arc::new(StdMutex::new(MemoryStore::new(Default::default())));
    let maintenance = Arc::new(tokio::sync::Mutex::new(MaintenanceTracker::new()));

    perform_maintenance_cycle(semantic_cache, memory_store, maintenance, "manual").await
}

/// Run a single health check on demand.
///
/// Verifies the server is responsive by checking basic subsystem health.
pub async fn run_health_check(server: &super::server::AcpServer) -> Result<()> {
    // Observable health signals. (pua_enforcement_plan and runtime_config
    // are always constructed — Arc<Mutex<..>> / plain struct — so merely
    // borrowing them would never fail; only checks that can actually fail
    // are worth running here.)
    if server.governance_deps.harness_bus.is_none() {
        anyhow::bail!("health.check: harness_bus is not configured");
    }
    if server
        .agent_registry()
        .map(|r| r.names().is_empty())
        .unwrap_or(true)
    {
        anyhow::bail!("health.check: agent registry is empty or not configured");
    }
    if server
        .orchestration_deps
        .skill_registry
        .read()
        .map(|r| r.list(false).len())
        .unwrap_or(0)
        == 0
    {
        tracing::warn!("health.check: skill registry is empty");
    }
    Ok(())
}
