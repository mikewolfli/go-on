use crate::i18n::runtime::tf;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::bulkhead::Bulkhead;

// ── Submodules ──────────────────────────────────────────────────────────────

mod concurrency;
mod persistence;
mod priority;
mod queue;

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use concurrency::TaskPermitGuard;
pub use persistence::create_persistent_scheduler;
#[cfg(feature = "backend-sqlite")]
pub use persistence::SchedulerPersistence;
pub use priority::{Priority, ScheduledTask};

// ──────────────────────────────────────────────
// Types
// ──────────────────────────────────────────────

/// Configuration for the scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Global maximum concurrent tasks
    pub global_max_concurrent_tasks: usize,
    /// Maximum workers per role
    pub max_workers_per_role: usize,
    /// Aging bonus increment per second (applied to waiting tasks)
    pub aging_rate: f64,
    /// Maximum aging bonus cap
    pub max_aging_bonus: f64,
    /// Default max retries per task
    pub default_max_retries: u32,
    /// Aging check interval in seconds
    pub aging_interval_secs: u64,
    /// Queue depth at which backpressure is triggered (rejects with 429).
    /// When the pending queue exceeds this, new submissions are rejected.
    pub backpressure_queue_depth: usize,
    /// Enable automatic fault tolerance recovery cycle (runs every 30s).
    pub fault_tolerance_enabled: bool,
}

/// Default queue depth for backpressure: 500 pending tasks.
pub const DEFAULT_BACKPRESSURE_QUEUE_DEPTH: usize = 500;

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            global_max_concurrent_tasks: 100,
            max_workers_per_role: 3,
            aging_rate: 0.1,
            max_aging_bonus: 5.0,
            default_max_retries: 3,
            aging_interval_secs: 5,
            backpressure_queue_depth: DEFAULT_BACKPRESSURE_QUEUE_DEPTH,
            fault_tolerance_enabled: false,
        }
    }
}

/// Scheduler statistics snapshot for governance.status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerProfile {
    /// Level-1 queue depth
    pub l1_queue_depth: u32,
    /// Level-2 active worker count
    pub l2_active_workers: u32,
    /// Level-2 fan-out count (tasks split across workers)
    pub l2_fan_out_count: u32,
    /// Total tasks submitted
    pub total_submitted: u64,
    /// Total tasks completed
    pub total_completed: u64,
    /// Total tasks failed
    pub total_failed: u64,
    /// Starvation events prevented
    pub starvation_events_prevented: u64,
    /// Backpressure rejections (429s)
    pub backpressure_rejections: u64,
}

use queue::SchedulerState;

// ──────────────────────────────────────────────
// Level-1: TaskScheduler (Queue Manager)
// ──────────────────────────────────────────────

pub struct TaskScheduler {
    config: SchedulerConfig,
    /// Merged scheduler state (queues + task_map) protected by a single RwLock.
    /// Using std::sync::RwLock because all access is from synchronous code;
    /// tokio::sync::RwLock would require .blocking_*() and a runtime context.
    state: RwLock<SchedulerState>,
    /// Active task permits: task_id → (global_permit, role_permit).
    /// Holding these permits consumes semaphore capacity.
    /// Dropping the permits returns them to the semaphores.
    active: Mutex<HashMap<String, (OwnedSemaphorePermit, OwnedSemaphorePermit)>>,
    /// Statistics
    stats: RwLock<SchedulerProfile>,
    /// Last aging update timestamp
    last_aging: Mutex<Instant>,
    /// Global concurrency limiter using a semaphore.
    concurrency_limiter: Arc<Semaphore>,
    /// Per-role concurrency limiters (role name → semaphore).
    role_limiters: Mutex<HashMap<String, Arc<Semaphore>>>,
    /// Cancellation token for the aging background task.
    aging_cancel: Mutex<Option<CancellationToken>>,

    /// Bulkhead instance for per-provider concurrency isolation.
    bulkhead: Bulkhead,
    /// Cancellation token for the fault tolerance background task.
    ft_cancel: Mutex<Option<CancellationToken>>,
    /// Optional persistence for surviving restarts (SQLite-backed)
    #[cfg(feature = "backend-sqlite")]
    persistence: Option<Arc<SchedulerPersistence>>,
}

impl TaskScheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        let global_permits = config.global_max_concurrent_tasks;
        Self {
            state: RwLock::new(SchedulerState {
                queues: HashMap::new(),
                task_map: HashMap::new(),
            }),
            active: Mutex::new(HashMap::new()),
            stats: RwLock::new(SchedulerProfile {
                l1_queue_depth: 0,
                l2_active_workers: 0,
                l2_fan_out_count: 0,
                total_submitted: 0,
                total_completed: 0,
                total_failed: 0,
                starvation_events_prevented: 0,
                backpressure_rejections: 0,
            }),
            last_aging: Mutex::new(Instant::now()),
            concurrency_limiter: Arc::new(Semaphore::new(global_permits)),
            role_limiters: Mutex::new(HashMap::new()),
            aging_cancel: Mutex::new(None),
            bulkhead: Bulkhead::new(config.max_workers_per_role * 3),
            ft_cancel: Mutex::new(None),
            #[cfg(feature = "backend-sqlite")]
            persistence: None,
            config,
        }
    }

    /// Create a scheduler with SQLite-backed persistence.
    ///
    /// On creation, attempts to restore any tasks that were persisted
    /// in a previous session and re-enqueues them.
    #[cfg(feature = "backend-sqlite")]
    pub fn new_with_persistence(
        config: SchedulerConfig,
        persistence: SchedulerPersistence,
    ) -> Self {
        let global_permits = config.global_max_concurrent_tasks;
        let persistence = if persistence.is_enabled() {
            Some(Arc::new(persistence))
        } else {
            None
        };

        let scheduler = Self {
            state: RwLock::new(SchedulerState {
                queues: HashMap::new(),
                task_map: HashMap::new(),
            }),
            active: Mutex::new(HashMap::new()),
            stats: RwLock::new(SchedulerProfile {
                l1_queue_depth: 0,
                l2_active_workers: 0,
                l2_fan_out_count: 0,
                total_submitted: 0,
                total_completed: 0,
                total_failed: 0,
                starvation_events_prevented: 0,
                backpressure_rejections: 0,
            }),
            last_aging: Mutex::new(Instant::now()),
            concurrency_limiter: Arc::new(Semaphore::new(global_permits)),
            role_limiters: Mutex::new(HashMap::new()),
            aging_cancel: Mutex::new(None),
            bulkhead: Bulkhead::new(config.max_workers_per_role * 3),
            ft_cancel: Mutex::new(None),
            persistence,
            config,
        };

        // Restore previously persisted tasks
        if let Some(ref p) = scheduler.persistence {
            match p.restore_queue() {
                Ok(tasks) => {
                    let count = tasks.len();
                    for task in tasks {
                        if let Err(e) = scheduler.submit(task) {
                            warn!("Failed to restore persisted task: {}", e);
                        }
                    }
                    if count > 0 {
                        info!("Restored {} tasks from persistence", count);
                    }
                }
                Err(e) => {
                    warn!("Failed to restore scheduler queue: {}", e);
                }
            }
        }

        scheduler
    }

    /// Submit a task to the queue. Pushes into the role-specific BinaryHeap,
    /// stores in task_map.
    ///
    /// Returns `Err` if the queue depth exceeds the backpressure threshold
    /// (configured via `SchedulerConfig::backpressure_queue_depth`).
    pub fn submit(&self, task: ScheduledTask) -> Result<()> {
        let task_id = task.task_id.clone();
        let role = task.role.clone();

        {
            let state = self.state.read().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            if state.task_map.contains_key(&task_id) {
                return Err(anyhow!(tf(
                    "error.scheduler.task_already_submitted",
                    &[("task_id", &task_id)]
                )));
            }

            // Check backpressure: reject if total pending queue depth exceeds threshold.
            let total_pending: usize = state.queues.values().map(|q| q.len()).sum();
            if total_pending >= self.config.backpressure_queue_depth {
                drop(state);
                let mut stats = self.stats.write().unwrap_or_else(|poisoned| {
                    tracing::warn!("lock poisoned, recovering");
                    poisoned.into_inner()
                });
                stats.backpressure_rejections += 1;
                return Err(anyhow!(tf(
                    "error.scheduler.backpressure_rejected",
                    &[
                        ("total_pending", &total_pending.to_string()),
                        (
                            "threshold",
                            &self.config.backpressure_queue_depth.to_string()
                        ),
                        ("task_id", &task_id)
                    ]
                )));
            }
        } // Release read lock before acquiring write lock

        // Push into the role-specific heap and insert into task_map in a single write lock
        {
            let mut state = self.state.write().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            state.queues.entry(role).or_default().push(task.clone());
            state.task_map.insert(task_id.clone(), task);
        }

        {
            let mut stats = self.stats.write().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            stats.total_submitted += 1;
        }
        debug!("Submitted task {}", task_id);

        // Persist the task if persistence is enabled
        #[cfg(feature = "backend-sqlite")]
        if let Some(ref p) = self.persistence {
            let state = self.state.read().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            if let Some(saved) = state.task_map.get(&task_id) {
                if let Err(e) = p.save_task(saved) {
                    warn!("Failed to persist task {}: {}", task_id, e);
                }
            }
        }

        Ok(())
    }

    /// Acquire a global concurrency permit (async-safe).
    ///
    /// Returns a `SemaphorePermit` that must be held while the task
    /// is executing. Drop the permit to release the slot.
    #[allow(dead_code)] // F-GAP-51 — new API surface, not yet wired
    pub async fn acquire_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.concurrency_limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| anyhow!("Failed to acquire concurrency permit: {}", e))
    }

    /// Try to acquire a global concurrency permit without waiting.
    #[allow(dead_code)] // F-GAP-12 — reserved for task scheduling integration
    pub fn try_acquire_permit(&self) -> Result<Option<tokio::sync::OwnedSemaphorePermit>> {
        match self.concurrency_limiter.clone().try_acquire_owned() {
            Ok(permit) => Ok(Some(permit)),
            Err(_) => Ok(None),
        }
    }

    /// Acquire a per-role concurrency permit.
    ///
    /// If no per-role limiter exists for this role, one is created
    /// with the configured `max_workers_per_role` permits.
    #[allow(dead_code)] // F-GAP-12 — reserved for task scheduling integration
    pub async fn acquire_role_permit(
        &self,
        role: &str,
    ) -> Result<tokio::sync::OwnedSemaphorePermit> {
        let limiter = {
            let mut role_limiters = self
                .role_limiters
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            role_limiters
                .entry(role.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(self.config.max_workers_per_role)))
                .clone()
        };
        limiter
            .acquire_owned()
            .await
            .map_err(|e| anyhow!("Failed to acquire role permit for '{}': {}", role, e))
    }

    /// Dequeue the highest-priority task for the given role.
    ///
    /// O(log n) — pops directly from the role-specific BinaryHeap.
    /// Acquires global and per-role semaphore permits atomically; the caller
    /// must not separately call `acquire_permit` / `acquire_role_permit`.
    ///
    /// Returns a tuple `(task, guard)` where the guard holds the semaphore
    /// permits. The permits are automatically released when the guard is
    /// dropped, preventing resource leaks if the caller drops the task
    /// without calling `complete()` or `fail()`.
    ///
    /// Returns `None` if the queue is empty or capacity is exhausted.
    pub fn dequeue(&self, role: &str) -> Option<(ScheduledTask, TaskPermitGuard)> {
        // Note: apply_aging should be called periodically by a background
        // timer task, not synchronously on every dequeue call.

        // Check global concurrency capacity via the semaphore.
        if self.available_concurrency() == 0 {
            debug!("Semaphore exhausted, cannot dequeue");
            return None;
        }

        // Check per-role capacity via its semaphore.
        if self.is_role_at_capacity(role) {
            debug!("Role {} at capacity, cannot dequeue", role);
            return None;
        }

        // Pop the highest-priority task from the role-specific heap — O(log n).
        // Use recoverable lock: if the RwLock is poisoned, recover the data and
        // continue rather than silently discarding the error.
        let task = {
            let mut state = self.state.write().unwrap_or_else(|poisoned| {
                let guard = poisoned.into_inner();
                warn!(target: "scheduler", "state RwLock poisoned – recovered data");
                guard
            });
            state.queues.get_mut(role)?.pop()?
        };

        let role_str = task.role.clone();

        // Acquire the global semaphore permit.
        let global_permit = match self.concurrency_limiter.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                // Should not happen (we checked above), but roll back.
                let mut state = self.state.write().unwrap_or_else(|poisoned| {
                    let guard = poisoned.into_inner();
                    warn!(target: "scheduler", "state RwLock poisoned – recovered data");
                    guard
                });
                state.queues.entry(role_str).or_default().push(task);
                return None;
            }
        };

        // Acquire (or create) the per-role semaphore permit.
        let role_permit = {
            let mut limiters = self.role_limiters.lock().unwrap_or_else(|poisoned| {
                let guard = poisoned.into_inner();
                warn!(target: "scheduler", "role_limiters Mutex poisoned – recovered data");
                guard
            });
            let limiter = limiters
                .entry(role_str.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(self.config.max_workers_per_role)))
                .clone();
            match limiter.try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    // Roll back the global permit.
                    drop(global_permit);
                    let mut state = self.state.write().unwrap_or_else(|poisoned| {
                        let guard = poisoned.into_inner();
                        warn!(target: "scheduler", "state RwLock poisoned – recovered data");
                        guard
                    });
                    state.queues.entry(role_str).or_default().push(task);
                    return None;
                }
            }
        };

        // Acquire a per-provider bulkhead permit if the task specifies a provider.
        // `try_acquire` returns `Result<Option<OwnedSemaphorePermit>, &str>`.
        let guard = if let Some(ref provider) = task.provider {
            match self.bulkhead.try_acquire(provider) {
                Ok(Some(provider_permit)) => TaskPermitGuard::with_provider_permit(
                    global_permit,
                    role_permit,
                    provider_permit,
                ),
                Ok(None) | Err(_) => {
                    // Provider at capacity or lock poisoned — roll back.
                    drop(global_permit);
                    drop(role_permit);
                    let mut state = self.state.write().unwrap_or_else(|poisoned| {
                        let guard = poisoned.into_inner();
                        warn!(target: "scheduler", "state RwLock poisoned – recovered data");
                        guard
                    });
                    state.queues.entry(role_str).or_default().push(task);
                    return None;
                }
            }
        } else {
            TaskPermitGuard::new(global_permit, role_permit)
        };

        // Return the permits as a TaskPermitGuard so they auto-release on drop.

        debug!("Dequeued task {} for role {}", task.task_id, role);
        Some((task, guard))
    }

    /// Mark task as completed.
    ///
    /// The caller should drop the `TaskPermitGuard` (which releases the
    /// semaphore permits automatically). Removes from `task_map` and
    /// updates stats.
    ///
    /// Recovers from Mutex poison rather than silently discarding errors,
    /// so that the task is always removed from the map and stats are updated.
    pub fn complete(&self, task_id: &str) -> Result<()> {
        // Remove completed task from task_map — recover from poison instead of
        // propagating the error, so the task is never leaked.
        {
            let mut state = self.state.write().unwrap_or_else(|poisoned| {
                let guard = poisoned.into_inner();
                warn!(target: "scheduler", "state RwLock poisoned – recovered data");
                guard
            });
            state.task_map.remove(task_id);
        }

        // Update stats — recover from poison.
        {
            let mut stats = self.stats.write().unwrap_or_else(|poisoned| {
                let guard = poisoned.into_inner();
                warn!(target: "scheduler", "stats RwLock poisoned – recovered data");
                guard
            });
            stats.total_completed += 1;
        }

        info!("Task {} completed", task_id);

        // Remove from persistence on completion
        #[cfg(feature = "backend-sqlite")]
        if let Some(ref p) = self.persistence {
            if let Err(e) = p.remove_task(task_id) {
                warn!("Failed to remove task {} from persistence: {}", task_id, e);
            }
        }

        Ok(())
    }

    /// Mark task as failed. If requeue and retries < max_retries, increments retries
    /// and pushes back to the role-specific queue. Otherwise removes permanently.
    ///
    /// The caller should drop the `TaskPermitGuard` (which releases the
    /// semaphore permits automatically). On requeue the permits are released
    /// (via drop) and the task is re-enqueued.
    ///
    /// # TOCTOU fix (GAP-B50-22)
    /// Merges task_map read + queues write into a single lock scope to prevent
    /// a concurrent `complete()` from removing the task between the read and
    /// the re-enqueue.
    pub fn fail(&self, task_id: &str, requeue: bool) -> Result<()> {
        // Note: The TaskPermitGuard should be dropped by the caller to
        // release semaphore permits. This method only handles task_map
        // and stat bookkeeping.

        if requeue {
            // ── Single lock scope: state write ─────────────────────────────
            // Holding the state write lock across task_map read + queues write
            // prevents a concurrent complete() from removing the task between
            // the retry check and the re-enqueue.
            let mut state = self.state.write().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            if let Some(task) = state.task_map.get_mut(task_id) {
                if task.retries < task.max_retries {
                    task.retries += 1;
                    let updated_task = task.clone();
                    let role = task.role.clone();
                    let retries = task.retries;
                    let max_retries = task.max_retries;
                    state.queues.entry(role).or_default().push(updated_task);
                    drop(state);
                    let mut stats = self.stats.write().unwrap_or_else(|poisoned| {
                        tracing::warn!("lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                    stats.total_failed += 1;
                    warn!(
                        "{}",
                        tf(
                            "status.scheduler.task_requeued",
                            &[
                                ("task_id", task_id),
                                ("retries", &retries.to_string()),
                                ("max_retries", &max_retries.to_string()),
                            ]
                        )
                    );
                    return Ok(());
                }
            }
            // Drop state lock before the permanent-removal path below
            drop(state);
        }

        // ── Single lock scope: read + remove from task_map ─────────────
        // Prevents a concurrent modification between the read and remove.
        let max_retries = {
            let mut state = self
                .state
                .write()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            let max_r = state
                .task_map
                .get(task_id)
                .map(|t| t.max_retries)
                .unwrap_or(0);
            state.task_map.remove(task_id);
            max_r
        };
        self.stats
            .write()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .total_failed += 1;
        error!(
            "{}",
            tf(
                "status.scheduler.task_failed_permanently",
                &[
                    ("task_id", task_id),
                    ("max_retries", &max_retries.to_string()),
                ]
            )
        );
        Ok(())
    }

    /// Public aging method — called from dequeue() with throttling or from a
    /// background timer. Increments aging_bonus for all waiting tasks to prevent
    /// starvation of low-priority items.
    ///
    /// Should be called periodically (e.g. every 1-2 seconds) by a background
    /// timer task, not synchronously on every dequeue call.
    ///
    /// Start a background timer that periodically applies aging to all
    /// queued tasks, preventing starvation by boosting the priority of
    /// long-waiting tasks.
    ///
    /// Returns a `JoinHandle` that can be aborted. The task will also stop
    /// when the stored `CancellationToken` is cancelled via `shutdown()`.
    /// Errors from `apply_aging` are logged as warnings.
    pub fn start_aging_timer(self: &Arc<Self>, interval: Duration) -> JoinHandle<()> {
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        // Store the token so shutdown() can cancel this task.
        if let Ok(mut stored) = self.aging_cancel.lock() {
            *stored = Some(cancel);
        }
        let sched = self.clone();
        // Also start the fault tolerance background timer if enabled.
        let _ft_handle = self.start_fault_tolerance_timer();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = timer.tick() => {
                        sched.apply_aging();
                    }
                    _ = cancel_clone.cancelled() => {
                        info!("Aging timer cancelled");
                        break;
                    }
                }
            }
        })
    }

    /// Start a background timer for the fault tolerance recovery cycle.
    ///
    /// Spawns a periodic task that runs every 30 seconds.  On each tick,
    /// it calls [`FaultToleranceEngine::run_recovery_cycle`] to detect faults,
    /// create recovery plans, and execute pending plans.
    ///
    /// Returns `None` when `fault_tolerance_enabled` is `false`.
    /// The task is cancelled when [`shutdown`](Self::shutdown) is called.
    ///
    /// Note: This method is called from `start_aging_timer()` which also
    /// starts the fault tolerance loop.  The `pub` visibility is reserved
    /// for external callers who want independent lifecycle control.
    #[allow(dead_code)]
    pub fn start_fault_tolerance_timer(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        if !self.config.fault_tolerance_enabled {
            return None;
        }

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        if let Ok(mut stored) = self.ft_cancel.lock() {
            *stored = Some(cancel);
        }

        let engine = crate::fault_tolerance::FaultToleranceEngine::new(
            crate::fault_tolerance::FaultToleranceConfig::default(),
        );

        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            // Skip the first immediate tick so the loop waits 30s before
            // the first recovery cycle.
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let summary = engine.run_recovery_cycle().await;
                        info!(
                            "FaultTolerance: recovery cycle — {} offenders, {} plans created, {:?}",
                            summary.offenders.len(),
                            summary.plans_created,
                            summary.cluster_health,
                        );
                    }
                    _ = cancel_clone.cancelled() => {
                        info!("Fault tolerance timer cancelled");
                        break;
                    }
                }
            }
        }))
    }

    /// Gracefully stop the aging background task and fault tolerance timer.
    ///
    /// Cancels the `CancellationToken` instances stored by
    /// `start_aging_timer()` and `start_fault_tolerance_timer()`.  The
    /// background tasks will exit on their next tick after cancellation.
    /// This is safe to call multiple times; subsequent calls are no-ops.
    #[allow(dead_code)] // Reserved for graceful server shutdown
    pub fn shutdown(&self) {
        if let Ok(mut stored) = self.aging_cancel.lock() {
            if let Some(token) = stored.take() {
                token.cancel();
            }
        }
        if let Ok(mut stored) = self.ft_cancel.lock() {
            if let Some(token) = stored.take() {
                token.cancel();
            }
        }
        info!("Scheduler shutdown initiated");
    }

    /// Apply aging bonus to all pending (non-active) tasks.
    ///
    /// Uses snapshot-then-rebuild pattern: reads all tasks and the active set
    /// in a single lock scope, then rebuilds queues in a single lock scope.
    /// Eliminates the double-lock of task_map (update → release → re-read).
    pub fn apply_aging(&self) {
        let now = Instant::now();
        let elapsed = {
            let mut last = self.last_aging.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            let dur = now.duration_since(*last);
            *last = now;
            dur
        };

        let elapsed_secs = elapsed.as_secs_f64();
        if elapsed_secs <= 0.0 {
            return;
        }

        let aging_rate = self.config.aging_rate;
        let max_bonus = self.config.max_aging_bonus;

        // Aging threshold for starvation prevention tracking
        let starvation_threshold = 2.0;

        // ── Snapshot phase: single state + active lock scope ───────────
        //
        // ⚠️  LOCK ORDERING: state → active (both acquired here, state
        //     first).  Never acquire `active` then `state` — doing so
        //     creates a potential deadlock if another code path takes those
        //     locks in the opposite order.
        //
        // Update aging_bonus in place AND snapshot pending tasks atomically.
        let (pending_tasks, starvation_events) = {
            let mut state = self.state.write().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            let active = self.active.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });

            let mut starvation_events = 0u64;
            for task in state.task_map.values_mut() {
                let old_bonus = task.aging_bonus;
                let bonus = (task.aging_bonus + aging_rate * elapsed_secs).min(max_bonus);
                task.aging_bonus = bonus;
                // Check if aging crossed the starvation threshold
                if old_bonus < starvation_threshold && bonus >= starvation_threshold {
                    starvation_events += 1;
                }
            }

            // Snapshot non-active tasks while still holding the locks
            let pending: Vec<ScheduledTask> = state
                .task_map
                .values()
                .filter(|t| !active.contains_key(&t.task_id))
                .cloned()
                .collect();

            (pending, starvation_events)
        }; // state and active locks released here

        // Update stats
        if starvation_events > 0 {
            let mut stats = self.stats.write().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            stats.starvation_events_prevented += starvation_events;
            debug!(
                "Aging triggered {} starvation prevention events",
                starvation_events
            );
        }

        // ── Rebuild phase: single state write scope ───────────────────
        let mut state = self.state.write().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        state.queues.clear();
        for task in &pending_tasks {
            state
                .queues
                .entry(task.role.clone())
                .or_default()
                .push(task.clone());
        }
        debug!(
            "Aging applied (elapsed={:.2}s), queues rebuilt with {} tasks",
            elapsed_secs,
            pending_tasks.len()
        );
    }

    /// Return a snapshot of current stats
    pub fn profile(&self) -> SchedulerProfile {
        let mut profile = self
            .stats
            .read()
            .map(|s| s.clone())
            .unwrap_or(SchedulerProfile {
                l1_queue_depth: 0,
                l2_active_workers: 0,
                l2_fan_out_count: 0,
                total_submitted: 0,
                total_completed: 0,
                total_failed: 0,
                starvation_events_prevented: 0,
                backpressure_rejections: 0,
            });
        let state = self.state.read().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        profile.l1_queue_depth = state.queues.values().map(|q| q.len() as u32).sum();
        // active workers are now tracked via outstanding TaskPermitGuard instances.
        // Since those are dropped on complete/fail, we can approximate active count
        // from the task_map minus pending queue entries.
        let pending_count: u32 = state.queues.values().map(|h| h.len() as u32).sum();
        let total_in_map: u32 = state.task_map.len() as u32;
        profile.l2_active_workers = total_in_map.saturating_sub(pending_count);
        profile
    }

    /// Check if a role has reached its max_workers limit via the per-role semaphore.
    pub fn is_role_at_capacity(&self, role: &str) -> bool {
        let limiters = self.role_limiters.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Some(limiter) = limiters.get(role) {
            return limiter.available_permits() == 0;
        }
        false
    }

    /// Returns the number of permits currently available in the global semaphore.
    pub fn available_concurrency(&self) -> usize {
        self.concurrency_limiter.available_permits()
    }

    /// Returns a reference to the global concurrency limiter, for external
    /// consumers that need to acquire permits manually.
    #[allow(dead_code)] // F-GAP-12 — reserved for task scheduling diagnostics
    pub fn concurrency_limiter(&self) -> &Arc<Semaphore> {
        &self.concurrency_limiter
    }

    /// Returns a reference to a per-role concurrency limiter, creating one
    /// lazily if it doesn't exist.
    #[allow(dead_code)] // F-GAP-12 — reserved for task scheduling diagnostics
    pub fn role_limiter(&self, role: &str) -> Result<Arc<Semaphore>> {
        let mut limiters = self
            .role_limiters
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        let limiter = limiters
            .entry(role.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.config.max_workers_per_role)));
        Ok(Arc::clone(limiter))
    }

    /// Persist the entire queue to storage (for graceful shutdown).
    ///
    /// This is a no-op unless a `SchedulerPersistence` was provided
    /// via `new_with_persistence`.
    #[cfg(feature = "backend-sqlite")]
    pub fn persist_all(&self) -> Result<()> {
        if let Some(ref p) = self.persistence {
            let state = self
                .state
                .read()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            let tasks: Vec<ScheduledTask> = state.task_map.values().cloned().collect();
            p.snapshot_queue(&tasks)?;
            info!("Persisted {} tasks to storage", tasks.len());
        }
        Ok(())
    }
}

// ──────────────────────────────────────────────
// Level-2: AgentWorkerScheduler (Worker Pool)
// ──────────────────────────────────────────────

pub struct AgentWorkerScheduler {
    pub(crate) level1: Arc<TaskScheduler>,
    /// Worker registrations: role -> set of worker_ids
    workers: Mutex<HashMap<String, HashSet<String>>>,
    /// Active assignments: worker_id -> task_id
    assignments: Mutex<HashMap<String, String>>,
    /// Fan-out groups: group_id -> set of task_ids
    fan_out_groups: Mutex<HashMap<String, Vec<String>>>,
}

impl AgentWorkerScheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        Self {
            level1: Arc::new(TaskScheduler::new(config)),
            workers: Mutex::new(HashMap::new()),
            assignments: Mutex::new(HashMap::new()),
            fan_out_groups: Mutex::new(HashMap::new()),
        }
    }

    /// Register a worker for a role
    pub fn register_worker(&self, worker_id: &str, role: &str) -> Result<()> {
        let mut workers = self
            .workers
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        workers
            .entry(role.to_string())
            .or_default()
            .insert(worker_id.to_string());
        info!(
            "{}",
            tf(
                "status.scheduler.worker_registered",
                &[("worker_id", worker_id), ("role", role)]
            )
        );
        Ok(())
    }

    /// Remove a worker
    pub fn unregister_worker(&self, worker_id: &str, role: &str) -> Result<()> {
        let mut workers = self
            .workers
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        if let Some(worker_set) = workers.get_mut(role) {
            worker_set.remove(worker_id);
            // Clean up empty sets
            if worker_set.is_empty() {
                workers.remove(role);
            }
            // Remove any active assignment for this worker
            let mut assignments = self.assignments.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            if let Some(task_id) = assignments.remove(worker_id) {
                // Mark the task as failed since the worker left
                let _ = self.level1.fail(&task_id, true);
            }
            info!(
                "{}",
                tf(
                    "status.scheduler.worker_unregistered",
                    &[("worker_id", worker_id), ("role", role)]
                )
            );
            Ok(())
        } else {
            Err(anyhow!(tf(
                "error.scheduler.worker_not_found",
                &[("worker_id", worker_id), ("role", role)]
            )))
        }
    }

    /// Find an idle worker for the role, dequeue from level-1, assign the task.
    /// Returns (worker_id, task, permit_guard) on success.
    /// The permit guard must be held for the task's lifetime and dropped
    /// when the task completes or fails.
    pub fn assign_next(&self, role: &str) -> Option<(String, ScheduledTask, TaskPermitGuard)> {
        // Find an idle worker for this role
        let idle_worker = {
            let workers = match self.workers.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::error!("scheduler workers mutex poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            let assignments = match self.assignments.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::error!("scheduler assignments mutex poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            workers
                .get(role)?
                .iter()
                .find(|wid| !assignments.contains_key(*wid))
                .cloned()
        };

        let worker_id = idle_worker?;

        // Dequeue a task from level-1
        let (task, guard) = self.level1.dequeue(role)?;

        // Assign
        let mut assignments = self.assignments.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        assignments.insert(worker_id.clone(), task.task_id.clone());

        debug!(
            "Assigned task {} to worker {} (role {})",
            task.task_id, worker_id, role
        );
        Some((worker_id, task, guard))
    }

    /// Complete the task assigned to worker
    pub fn complete_task(&self, worker_id: &str) -> Result<()> {
        let task_id = {
            let mut assignments = self
                .assignments
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            assignments.remove(worker_id).ok_or_else(|| {
                anyhow!(tf(
                    "error.scheduler.no_active_assignment",
                    &[("worker_id", worker_id)]
                ))
            })?
        };
        self.level1.complete(&task_id)
    }

    /// Submit multiple tasks as a fan-out group. All tasks share the same role
    /// and are submitted to level-1. Returns group_id.
    pub fn submit_fan_out(&self, base_name: &str, tasks: Vec<ScheduledTask>) -> Result<String> {
        let group_id = format!("fanout-{}-{}", base_name, chrono_now_ms());
        let task_ids: Vec<String> = tasks.iter().map(|t| t.task_id.clone()).collect();

        // Submit all tasks to level-1
        for task in tasks {
            self.level1.submit(task)?;
        }

        // Track the fan-out group
        let count = task_ids.len();
        self.fan_out_groups
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .insert(group_id.clone(), task_ids);

        // Update stats
        let mut stats = self.level1.stats.write().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        stats.l2_fan_out_count += 1;

        info!(
            "{}",
            tf(
                "status.scheduler.fan_out_created",
                &[("group_id", &group_id), ("count", &count.to_string())]
            )
        );
        Ok(group_id)
    }

    /// Return (completed_count, total_count) for a fan-out group
    pub fn fan_out_progress(&self, group_id: &str) -> Result<(usize, usize)> {
        let groups = self
            .fan_out_groups
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        let task_ids = groups.get(group_id).ok_or_else(|| {
            anyhow!(tf(
                "error.scheduler.fan_out_group_not_found",
                &[("group_id", group_id)]
            ))
        })?;
        let total = task_ids.len();

        let state = self
            .level1
            .state
            .read()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        let task_map = &state.task_map;
        // Count tasks that are no longer in task_map (completed) or are not active
        let completed = task_ids
            .iter()
            .filter(|id| !task_map.contains_key(*id))
            .count();

        Ok((completed, total))
    }

    /// Aggregate profile from level-1 + worker stats
    pub fn profile(&self) -> SchedulerProfile {
        let mut profile = self.level1.profile();
        let assignments = self.assignments.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        profile.l2_active_workers = assignments.len() as u32;
        let groups = self.fan_out_groups.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        profile.l2_fan_out_count = groups.len() as u32;
        profile
    }
}

/// Helper: current timestamp in milliseconds for unique fan-out group IDs.
fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ──────────────────────────────────────────────
// Unit Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_task(id: &str, role: &str, priority_value: i64, base_score: f64) -> ScheduledTask {
        ScheduledTask {
            task_id: id.to_string(),
            role: role.to_string(),
            priority: Priority(priority_value),
            base_score,
            urgency: 0.5,
            cost_efficiency: 0.5,
            deadline_pressure: 0.0,
            aging_bonus: 0.0,
            submitted_at: chrono_now_ms(),
            retries: 0,
            max_retries: 3,
            provider: None,
        }
    }

    #[test]
    fn test_submit_and_dequeue() {
        let config = SchedulerConfig::default();
        let scheduler = TaskScheduler::new(config);

        let task_low = make_task("task-low", "worker", 1, 10.0);
        let task_high = make_task("task-high", "worker", 10, 100.0);
        let task_medium = make_task("task-med", "worker", 5, 50.0);

        scheduler.submit(task_low).unwrap();
        scheduler.submit(task_high).unwrap();
        scheduler.submit(task_medium).unwrap();

        // Highest priority (effective_priority) should come out first
        let (first, _g1) = scheduler.dequeue("worker").unwrap();
        assert_eq!(first.task_id, "task-high");

        let (second, _g2) = scheduler.dequeue("worker").unwrap();
        assert_eq!(second.task_id, "task-med");

        let (third, _g3) = scheduler.dequeue("worker").unwrap();
        assert_eq!(third.task_id, "task-low");

        // Queue should be empty now
        assert!(scheduler.dequeue("worker").is_none());
    }

    #[test]
    fn test_global_concurrency_cap() {
        let config = SchedulerConfig {
            global_max_concurrent_tasks: 2,
            ..Default::default()
        };
        let scheduler = TaskScheduler::new(config);

        scheduler
            .submit(make_task("t1", "role-a", 1, 10.0))
            .unwrap();
        scheduler
            .submit(make_task("t2", "role-b", 2, 20.0))
            .unwrap();
        scheduler
            .submit(make_task("t3", "role-c", 3, 30.0))
            .unwrap();

        // Dequeue two tasks — hold the guards to keep permits consumed
        let (_t1, g1) = scheduler.dequeue("role-a").unwrap();
        let (_t2, _g2) = scheduler.dequeue("role-b").unwrap();

        // Global cap should prevent third dequeue
        assert!(scheduler.dequeue("role-c").is_none());

        // Drop one guard (releasing its permits), then dequeue again
        drop(g1);
        let (_t3, _g3) = scheduler.dequeue("role-c").unwrap();
        assert_eq!(_t3.task_id, "t3");
    }

    #[test]
    fn test_role_capacity() {
        let config = SchedulerConfig {
            max_workers_per_role: 2,
            ..Default::default()
        };
        let scheduler = TaskScheduler::new(config);

        scheduler
            .submit(make_task("t1", "same-role", 1, 10.0))
            .unwrap();
        scheduler
            .submit(make_task("t2", "same-role", 2, 20.0))
            .unwrap();
        scheduler
            .submit(make_task("t3", "same-role", 3, 30.0))
            .unwrap();

        // Dequeue returns highest priority first: t3 (30), then t2 (20)
        let (task_a, _g1) = scheduler.dequeue("same-role").unwrap();
        let (_task_b, _g2) = scheduler.dequeue("same-role").unwrap();
        // Role cap should prevent third dequeue (role permits consumed by the guards)
        assert!(scheduler.dequeue("same-role").is_none());

        // Drop one guard (releasing the role permit), then dequeue should work
        drop(_g1);
        scheduler.complete(&task_a.task_id).unwrap();
        let (task_c, _g3) = scheduler.dequeue("same-role").unwrap();
        assert_eq!(task_c.task_id, "t1");
    }

    #[test]
    fn test_aging_bonus() {
        let config = SchedulerConfig {
            aging_rate: 1.0,
            max_aging_bonus: 10.0,
            ..Default::default()
        };
        let scheduler = TaskScheduler::new(config);

        scheduler.submit(make_task("t1", "role", 1, 10.0)).unwrap();
        assert_eq!(
            scheduler
                .state
                .read()
                .unwrap()
                .task_map
                .get("t1")
                .unwrap()
                .aging_bonus,
            0.0
        );

        // Manually set last_aging to simulate elapsed time
        {
            let mut last = scheduler.last_aging.lock().unwrap();
            *last = Instant::now() - Duration::from_secs(2);
        }

        scheduler.apply_aging();

        let bonus = scheduler
            .state
            .read()
            .unwrap()
            .task_map
            .get("t1")
            .unwrap()
            .aging_bonus;
        assert!(
            bonus > 0.0,
            "Aging bonus should have increased, got {}",
            bonus
        );
        assert!(bonus <= 10.0, "Aging bonus should be capped at max");
    }

    #[test]
    fn test_anti_starvation() {
        let config = SchedulerConfig {
            aging_rate: 100.0, // Very fast aging for test
            max_aging_bonus: 500.0,
            global_max_concurrent_tasks: 1, // Only one at a time to force queue buildup
            ..Default::default()
        };
        let scheduler = TaskScheduler::new(config);

        // Submit a low-priority task first (so it has time to age)
        let low_task = make_task("low-prio", "role", 1, 10.0);
        scheduler.submit(low_task).unwrap();

        // Simulate aging (0.5 seconds to keep bonus below 99 so high-prio still wins)
        {
            let mut last = scheduler.last_aging.lock().unwrap();
            *last = Instant::now() - Duration::from_millis(500);
        }
        scheduler.apply_aging();

        // Now submit a high-priority task
        let high_task = make_task("high-prio", "role", 100, 1000.0);
        scheduler.submit(high_task).unwrap();

        // Without aging, high-prio would be dequeued first.
        // With aging, the aged low-prio task may surpass it.
        let aged_bonus = {
            let state = scheduler.state.read().unwrap();
            state.task_map.get("low-prio").unwrap().aging_bonus
        };

        // Since 10 + aged_bonus < 1000 (aging_rate * 10s = 1000, capped at 500),
        // high-prio should still be dequeued first by default.
        // Let's verify that aging has been applied.
        assert!(aged_bonus > 0.0, "Aging should have provided a bonus");

        // Now dequeue and check order. High priority should still win.
        let (first, _g) = scheduler.dequeue("role").unwrap();
        assert_eq!(first.task_id, "high-prio", "High priority should still win");

        // But the low-prio task should have a significant aging bonus
        let bonus = scheduler
            .state
            .read()
            .unwrap()
            .task_map
            .get("low-prio")
            .unwrap()
            .aging_bonus;
        assert!(
            bonus > 1.0,
            "Low-priority task should have accumulated aging bonus"
        );

        // Verify starvation events tracking
        let profile = scheduler.profile();
        assert!(profile.starvation_events_prevented > 0);
    }

    #[test]
    fn test_complete_and_fail() {
        let config = SchedulerConfig::default();
        let scheduler = TaskScheduler::new(config);

        // Submit and dequeue a task
        scheduler.submit(make_task("t1", "role", 1, 10.0)).unwrap();
        let (task, _g1) = scheduler.dequeue("role").unwrap();
        assert_eq!(task.task_id, "t1");

        // Complete it
        drop(_g1);
        scheduler.complete("t1").unwrap();
        let profile = scheduler.profile();
        assert_eq!(profile.total_completed, 1);

        // Test fail + requeue
        scheduler.submit(make_task("t2", "role", 2, 20.0)).unwrap();
        let (_t2, _g2) = scheduler.dequeue("role").unwrap();

        // Fail with requeue
        drop(_g2);
        scheduler.fail("t2", true).unwrap();
        assert!(scheduler.state.read().unwrap().task_map.contains_key("t2"));

        // Verify retry count incremented
        let retries = scheduler
            .state
            .read()
            .unwrap()
            .task_map
            .get("t2")
            .unwrap()
            .retries;
        assert_eq!(retries, 1);

        // Dequeue again, complete, verify stats
        let (_t2b, _g2b) = scheduler.dequeue("role").unwrap();
        drop(_g2b);
        scheduler.complete("t2").unwrap();
        assert!(!scheduler.state.read().unwrap().task_map.contains_key("t2"));

        // Test fail without requeue
        scheduler.submit(make_task("t3", "role", 3, 30.0)).unwrap();
        let (_t3, _g3) = scheduler.dequeue("role").unwrap();
        drop(_g3);
        scheduler.fail("t3", false).unwrap();
        assert!(!scheduler.state.read().unwrap().task_map.contains_key("t3"));
        let profile = scheduler.profile();
        assert_eq!(profile.total_failed, 2);
    }

    #[test]
    fn test_fan_out() {
        let config = SchedulerConfig {
            max_workers_per_role: 10,
            ..Default::default()
        };
        let l2 = AgentWorkerScheduler::new(config);

        // Register workers (3 workers for 3 fan-out tasks)
        l2.register_worker("w1", "role").unwrap();
        l2.register_worker("w2", "role").unwrap();
        l2.register_worker("w3", "role").unwrap();

        // Create fan-out tasks
        let tasks: Vec<ScheduledTask> = (0..3)
            .map(|i| make_task(&format!("fan-{}", i), "role", i, (i as f64) * 10.0))
            .collect();

        let group_id = l2.submit_fan_out("test-group", tasks).unwrap();
        assert!(group_id.starts_with("fanout-test-group-"));

        // Assign all tasks
        for _ in 0..3 {
            let assigned = l2.assign_next("role");
            assert!(assigned.is_some(), "Should assign a task");
        }

        // No more tasks to assign
        assert!(l2.assign_next("role").is_none());

        // Fan-out progress: 0 completed, 3 total
        let (completed, total) = l2.fan_out_progress(&group_id).unwrap();
        assert_eq!(completed, 0);
        assert_eq!(total, 3);

        // Complete two tasks (leave third for progress check)
        l2.complete_task("w1").unwrap();
        l2.complete_task("w2").unwrap();

        let (completed, total) = l2.fan_out_progress(&group_id).unwrap();
        assert_eq!(completed, 2);
        assert_eq!(total, 3);
    }

    #[test]
    fn test_worker_assign_next() {
        let config = SchedulerConfig::default();
        let l2 = AgentWorkerScheduler::new(config);

        // Register workers
        l2.register_worker("worker-alpha", "coder").unwrap();
        l2.register_worker("worker-beta", "coder").unwrap();
        l2.register_worker("worker-gamma", "reviewer").unwrap();

        // Submit tasks
        let task1 = make_task("code-task-1", "coder", 1, 100.0);
        let task2 = make_task("code-task-2", "coder", 2, 50.0);
        let task3 = make_task("review-task", "reviewer", 3, 200.0);

        l2.level1.submit(task1).unwrap();
        l2.level1.submit(task2).unwrap();
        l2.level1.submit(task3).unwrap();

        // Assign for coder role
        let (worker_id, task, _guard) = l2.assign_next("coder").unwrap();
        assert_eq!(task.role, "coder");
        assert!(worker_id == "worker-alpha" || worker_id == "worker-beta");

        let (worker_id2, task2, _guard2) = l2.assign_next("coder").unwrap();
        assert_eq!(task2.role, "coder");
        assert_ne!(
            worker_id, worker_id2,
            "Two different workers should be assigned"
        );

        // All coder workers busy, next assign should be None
        assert!(l2.assign_next("coder").is_none());

        // Assign for reviewer role
        let (worker_id3, task3, _guard3) = l2.assign_next("reviewer").unwrap();
        assert_eq!(worker_id3, "worker-gamma");
        assert_eq!(task3.task_id, "review-task");

        // Complete one coder task
        drop(_guard);
        l2.complete_task(&worker_id).unwrap();

        // Submit a new task so we have something to assign
        let task4 = make_task("code-task-3", "coder", 4, 150.0);
        l2.level1.submit(task4).unwrap();

        // Now we can assign again for coder
        let (worker_id4, _, _guard4) = l2.assign_next("coder").unwrap();
        assert!(worker_id4 == "worker-alpha" || worker_id4 == "worker-beta");
    }

    #[test]
    fn test_backpressure_rejects_when_queue_full() {
        let config = SchedulerConfig {
            backpressure_queue_depth: 2,
            ..Default::default()
        };
        let scheduler = TaskScheduler::new(config);

        scheduler.submit(make_task("t1", "w", 1, 10.0)).unwrap();
        scheduler.submit(make_task("t2", "w", 2, 20.0)).unwrap();
        // Third task should be rejected since queue depth >= 2.
        let result = scheduler.submit(make_task("t3", "w", 3, 30.0));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("error.scheduler.backpressure_rejected"));

        let profile = scheduler.profile();
        assert_eq!(profile.backpressure_rejections, 1);
    }

    #[test]
    fn test_available_concurrency_reflects_semaphore() {
        let config = SchedulerConfig::default();
        let scheduler = TaskScheduler::new(config);
        assert_eq!(scheduler.available_concurrency(), 100);
    }

    #[test]
    fn test_concurrency_limiter_access() {
        let config = SchedulerConfig::default();
        let scheduler = TaskScheduler::new(config);
        let limiter = scheduler.concurrency_limiter();
        assert_eq!(limiter.available_permits(), 100);
    }

    #[tokio::test]
    async fn test_acquire_permit_reduces_available() {
        let config = SchedulerConfig::default();
        let scheduler = TaskScheduler::new(config);
        let initial = scheduler.available_concurrency();
        let permit = scheduler.acquire_permit().await.unwrap();
        assert_eq!(scheduler.available_concurrency(), initial - 1);
        drop(permit);
        assert_eq!(scheduler.available_concurrency(), initial);
    }

    #[tokio::test]
    async fn test_acquire_role_permit_works() {
        let config = SchedulerConfig {
            max_workers_per_role: 2,
            ..Default::default()
        };
        let scheduler = TaskScheduler::new(config);

        let p1 = scheduler.acquire_role_permit("coder").await.unwrap();
        let p2 = scheduler.acquire_role_permit("coder").await.unwrap();

        // Third acquisition for same role would block; we test with try.
        let limiter = scheduler.role_limiter("coder").unwrap();
        assert_eq!(limiter.available_permits(), 0);

        drop(p1);
        assert_eq!(limiter.available_permits(), 1);
        drop(p2);
        assert_eq!(limiter.available_permits(), 2);
    }

    #[test]
    fn test_try_acquire_permit_non_blocking() {
        let config = SchedulerConfig {
            global_max_concurrent_tasks: 1,
            ..Default::default()
        };
        let scheduler = TaskScheduler::new(config);

        let permit = scheduler.try_acquire_permit().unwrap();
        assert!(permit.is_some());
        // Second attempt should fail since only 1 permit.
        let none = scheduler.try_acquire_permit().unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn test_semaphore_integration_with_dequeue() {
        let config = SchedulerConfig {
            global_max_concurrent_tasks: 2,
            ..Default::default()
        };
        let scheduler = TaskScheduler::new(config);

        // Submit tasks.
        scheduler.submit(make_task("t1", "w", 1, 100.0)).unwrap();
        scheduler.submit(make_task("t2", "w", 2, 50.0)).unwrap();

        // Dequeue should work when under capacity.
        let (_task1, _g1) = scheduler.dequeue("w").unwrap();

        let (_task2, _g2) = scheduler.dequeue("w").unwrap();

        // Complete to free up capacity.
        drop(_g1);
        scheduler.complete("t1").unwrap();

        // Submit and dequeue again.
        scheduler.submit(make_task("t3", "w", 3, 30.0)).unwrap();
        let (task3, _g3) = scheduler.dequeue("w").unwrap();
        assert_eq!(task3.task_id, "t3");
    }

    // ── persistence smoke tests (backend-sqlite) ───────────────────────

    #[test]
    #[cfg(feature = "backend-sqlite")]
    fn persistence_smoke_new_and_is_enabled() {
        use tempfile::NamedTempFile;
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let p = SchedulerPersistence::new(Some(path));
        assert!(p.is_enabled());

        let p_disabled = SchedulerPersistence::new(None);
        assert!(!p_disabled.is_enabled());
    }

    #[test]
    #[cfg(feature = "backend-sqlite")]
    fn persistence_smoke_snapshot_and_restore() {
        use tempfile::NamedTempFile;
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let p = SchedulerPersistence::new(Some(path));

        let tasks = vec![make_task("persist-1", "worker", 5, 10.0)];
        p.snapshot_queue(&tasks).unwrap();
        let restored = p.restore_queue().unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].task_id, "persist-1");
    }

    #[test]
    #[cfg(feature = "backend-sqlite")]
    fn scheduler_with_persistence_smoke() {
        use std::sync::Arc;
        use tempfile::NamedTempFile;

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let config = SchedulerConfig::default();
        let persistence = SchedulerPersistence::new(Some(path));
        let scheduler = TaskScheduler::new_with_persistence(config, persistence);

        // Submit a task and persist.
        scheduler
            .submit(make_task("p-task", "worker", 3, 20.0))
            .unwrap();
        let arc_scheduler = Arc::new(scheduler);
        // Verify persist_all does not panic.
        arc_scheduler.persist_all().unwrap();
    }
}
