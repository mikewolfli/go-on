use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;
use tracing::{debug, info, warn};

// ──────────────────────────────────────────────
// Types
// ──────────────────────────────────────────────

/// Scheduling priority (higher = more urgent)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Priority(pub i64);

/// Task priority with anti-starvation boost
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ScheduledTask {
    pub task_id: String,
    pub role: String,
    pub priority: Priority,
    /// Base priority score (before aging boost)
    pub base_score: f64,
    /// Urgency component (0.0–1.0)
    pub urgency: f64,
    /// Cost efficiency component (0.0–1.0)
    pub cost_efficiency: f64,
    /// Deadline pressure (0.0–1.0), 0 = no deadline
    pub deadline_pressure: f64,
    /// Aging bonus (increments over time)
    pub aging_bonus: f64,
    /// Submission timestamp (epoch ms)
    pub submitted_at: i64,
    /// Number of retries so far
    pub retries: u32,
    /// Max allowed retries
    pub max_retries: u32,
}

impl ScheduledTask {
    #[allow(dead_code)]
    pub fn effective_priority(&self) -> f64 {
        self.base_score + self.aging_bonus
    }
}

impl Eq for ScheduledTask {}

impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        self.task_id == other.task_id
    }
}

impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        self.effective_priority()
            .partial_cmp(&other.effective_priority())
            .unwrap_or(Ordering::Equal)
        // BinaryHeap is a max-heap: the "greatest" element per Ord is at the top.
        // Higher effective_priority should be "greater", so we do NOT reverse.
    }
}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Configuration for the scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
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
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            global_max_concurrent_tasks: 10,
            max_workers_per_role: 3,
            aging_rate: 0.1,
            max_aging_bonus: 5.0,
            default_max_retries: 3,
            aging_interval_secs: 5,
        }
    }
}

/// Scheduler statistics snapshot for governance.status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
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
}

// ──────────────────────────────────────────────
// Level-1: TaskScheduler (Queue Manager)
// ──────────────────────────────────────────────

pub struct TaskScheduler {
    config: SchedulerConfig,
    /// Priority queue of pending tasks
    queue: Mutex<BinaryHeap<ScheduledTask>>,
    /// Active task IDs (currently being executed)
    active_tasks: Mutex<HashSet<String>>,
    /// Task lookup by ID
    task_map: Mutex<HashMap<String, ScheduledTask>>,
    /// Per-role active count
    role_active_count: Mutex<HashMap<String, usize>>,
    /// Statistics
    stats: Mutex<SchedulerProfile>,
    /// Last aging update timestamp
    last_aging: Mutex<Instant>,
}

impl TaskScheduler {
    #[allow(dead_code)]
    pub fn new(config: SchedulerConfig) -> Self {
        Self {
            queue: Mutex::new(BinaryHeap::new()),
            active_tasks: Mutex::new(HashSet::new()),
            task_map: Mutex::new(HashMap::new()),
            role_active_count: Mutex::new(HashMap::new()),
            stats: Mutex::new(SchedulerProfile {
                l1_queue_depth: 0,
                l2_active_workers: 0,
                l2_fan_out_count: 0,
                total_submitted: 0,
                total_completed: 0,
                total_failed: 0,
                starvation_events_prevented: 0,
            }),
            last_aging: Mutex::new(Instant::now()),
            config,
        }
    }

    /// Submit a task to the queue. Pushes into BinaryHeap, stores in task_map.
    #[allow(dead_code)]
    pub fn submit(&self, task: ScheduledTask) -> Result<()> {
        let task_id = task.task_id.clone();
        if self
            .task_map
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .contains_key(&task_id)
        {
            return Err(anyhow!("Task {} already submitted", task_id));
        }
        self.queue
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .push(task.clone());
        self.task_map
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .insert(task_id.clone(), task);
        self.stats
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .total_submitted += 1;
        debug!("Submitted task {}", task_id);
        Ok(())
    }

    /// Dequeue the highest-priority task matching the role.
    /// Checks global concurrency cap and per-role cap.
    /// On success, adds task_id to active_tasks and increments role counter.
    #[allow(dead_code)]
    pub fn dequeue(&self, role: &str) -> Option<ScheduledTask> {
        if self.is_global_at_capacity() {
            debug!("Global concurrency cap reached, cannot dequeue");
            return None;
        }
        if self.is_role_at_capacity(role) {
            debug!("Role {} at capacity, cannot dequeue", role);
            return None;
        }

        // We need to find the highest-priority task matching the role.
        // BinaryHeap only gives us the top, so we pop and re-push non-matching ones.
        let mut queue = self.queue.lock().ok()?;
        let mut buffer: Vec<ScheduledTask> = Vec::new();
        let mut result: Option<ScheduledTask> = None;

        while let Some(task) = queue.pop() {
            if self.active_tasks.lock().ok()?.contains(&task.task_id) {
                // Should not happen, but skip if already active
                buffer.push(task);
                continue;
            }
            if task.role == role {
                result = Some(task);
                break;
            }
            buffer.push(task);
        }

        // Re-push all non-matching tasks back
        for task in buffer {
            queue.push(task);
        }

        if let Some(ref task) = result {
            let task_id = task.task_id.clone();
            let role_key = task.role.clone();
            if let Ok(mut active) = self.active_tasks.lock() {
                active.insert(task_id);
            }
            if let Ok(mut counts) = self.role_active_count.lock() {
                *counts.entry(role_key).or_insert(0) += 1;
            }
            debug!("Dequeued task {} for role {}", task.task_id, role);
        }

        result
    }

    /// Mark task as completed. Removes from active_tasks and task_map, decrements role counter,
    /// updates stats.
    #[allow(dead_code)]
    pub fn complete(&self, task_id: &str) -> Result<()> {
        {
            let mut active = self
                .active_tasks
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            if !active.remove(task_id) {
                return Err(anyhow!("Task {} not found in active tasks", task_id));
            }
        }
        // Decrement role counter by looking up the task's role from task_map
        if let Ok(task_map) = self.task_map.lock() {
            if let Some(task) = task_map.get(task_id) {
                if let Ok(mut counts) = self.role_active_count.lock() {
                    if let Some(count) = counts.get_mut(&task.role) {
                        if *count > 0 {
                            *count -= 1;
                        }
                    }
                }
            }
        }
        // Remove completed task from task_map
        self.task_map
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .remove(task_id);
        self.stats
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .total_completed += 1;
        info!("Task {} completed", task_id);
        Ok(())
    }

    /// Mark task as failed. If requeue and retries < max_retries, increments retries
    /// and pushes back to queue. Otherwise removes permanently.
    #[allow(dead_code)]
    pub fn fail(&self, task_id: &str, requeue: bool) -> Result<()> {
        // Remove from active first
        {
            let mut active = self
                .active_tasks
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            active.remove(task_id);
        }

        // Decrement role counter
        if let Ok(task_map) = self.task_map.lock() {
            if let Some(task) = task_map.get(task_id) {
                if let Ok(mut counts) = self.role_active_count.lock() {
                    if let Some(count) = counts.get_mut(&task.role) {
                        if *count > 0 {
                            *count -= 1;
                        }
                    }
                }
            }
        }

        if requeue {
            if let Ok(mut task_map) = self.task_map.lock() {
                if let Some(task) = task_map.get_mut(task_id) {
                    if task.retries < task.max_retries {
                        task.retries += 1;
                        let updated_task = task.clone();
                        // Re-push into queue
                        self.queue
                            .lock()
                            .map_err(|e| anyhow!("Lock error: {}", e))?
                            .push(updated_task);
                        self.stats
                            .lock()
                            .map_err(|e| anyhow!("Lock error: {}", e))?
                            .total_failed += 1;
                        warn!(
                            "Task {} failed, requeueing (retry {}/{})",
                            task_id, task.retries, task.max_retries
                        );
                        return Ok(());
                    }
                }
            }
        }

        // Permanently remove
        self.task_map
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .remove(task_id);
        self.stats
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?
            .total_failed += 1;
        warn!("Task {} failed permanently", task_id);
        Ok(())
    }

    /// Apply aging to all waiting tasks. Iterates task_map, increments aging_bonus
    /// by aging_rate * elapsed_seconds, capped at max_aging_bonus. Rebuilds BinaryHeap.
    /// Tracks starvation_events_prevented when aging bonus crosses a threshold.
    #[allow(dead_code)]
    pub fn apply_aging(&self) {
        let now = Instant::now();
        let elapsed = {
            if let Ok(mut last) = self.last_aging.lock() {
                let dur = now.duration_since(*last);
                *last = now;
                dur
            } else {
                return;
            }
        };

        let elapsed_secs = elapsed.as_secs_f64();
        if elapsed_secs <= 0.0 {
            return;
        }

        let aging_rate = self.config.aging_rate;
        let max_bonus = self.config.max_aging_bonus;

        // Aging threshold for starvation prevention tracking
        let starvation_threshold = 2.0;

        if let Ok(mut task_map) = self.task_map.lock() {
            let mut starvation_events = 0u64;
            for task in task_map.values_mut() {
                // Only age tasks that are not currently active
                // (We age all tasks; active ones will eventually get their bonus too,
                //  but dequeue filters on active check.)
                let old_bonus = task.aging_bonus;
                let bonus = (task.aging_bonus + aging_rate * elapsed_secs).min(max_bonus);
                task.aging_bonus = bonus;
                // Check if aging crossed the starvation threshold
                if old_bonus < starvation_threshold && bonus >= starvation_threshold {
                    starvation_events += 1;
                }
            }

            // Update stats
            if starvation_events > 0 {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.starvation_events_prevented += starvation_events;
                }
                debug!(
                    "Aging triggered {} starvation prevention events",
                    starvation_events
                );
            }
        }

        // Rebuild the BinaryHeap from updated task_map
        if let (Ok(mut queue), Ok(task_map)) = (self.queue.lock(), self.task_map.lock()) {
            queue.clear();
            for task in task_map.values() {
                // Only queue tasks not currently active
                if let Ok(active) = self.active_tasks.lock() {
                    if !active.contains(&task.task_id) {
                        queue.push(task.clone());
                    }
                }
            }
            debug!(
                "Aging applied (elapsed={:.2}s), queue rebuilt with {} tasks",
                elapsed_secs,
                queue.len()
            );
        }
    }

    /// Return a snapshot of current stats
    #[allow(dead_code)]
    pub fn profile(&self) -> SchedulerProfile {
        let mut profile = self
            .stats
            .lock()
            .map(|s| s.clone())
            .unwrap_or(SchedulerProfile {
                l1_queue_depth: 0,
                l2_active_workers: 0,
                l2_fan_out_count: 0,
                total_submitted: 0,
                total_completed: 0,
                total_failed: 0,
                starvation_events_prevented: 0,
            });
        if let Ok(queue) = self.queue.lock() {
            profile.l1_queue_depth = queue.len() as u32;
        }
        if let Ok(active) = self.active_tasks.lock() {
            profile.l2_active_workers = active.len() as u32;
        }
        profile
    }

    /// Check if a role has reached max_workers
    #[allow(dead_code)]
    pub fn is_role_at_capacity(&self, role: &str) -> bool {
        if let Ok(counts) = self.role_active_count.lock() {
            let count = counts.get(role).copied().unwrap_or(0);
            count >= self.config.max_workers_per_role
        } else {
            true // Err on the side of caution
        }
    }

    /// Check if global concurrency cap is reached
    #[allow(dead_code)]
    pub fn is_global_at_capacity(&self) -> bool {
        if let Ok(active) = self.active_tasks.lock() {
            active.len() >= self.config.global_max_concurrent_tasks
        } else {
            true
        }
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
    #[allow(dead_code)]
    pub fn new(config: SchedulerConfig) -> Self {
        Self {
            level1: Arc::new(TaskScheduler::new(config)),
            workers: Mutex::new(HashMap::new()),
            assignments: Mutex::new(HashMap::new()),
            fan_out_groups: Mutex::new(HashMap::new()),
        }
    }

    /// Register a worker for a role
    #[allow(dead_code)]
    pub fn register_worker(&self, worker_id: &str, role: &str) -> Result<()> {
        let mut workers = self
            .workers
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        workers
            .entry(role.to_string())
            .or_default()
            .insert(worker_id.to_string());
        info!("Worker {} registered for role {}", worker_id, role);
        Ok(())
    }

    /// Remove a worker
    #[allow(dead_code)]
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
            if let Ok(mut assignments) = self.assignments.lock() {
                if let Some(task_id) = assignments.remove(worker_id) {
                    // Mark the task as failed since the worker left
                    let _ = self.level1.fail(&task_id, true);
                }
            }
            info!("Worker {} unregistered from role {}", worker_id, role);
            Ok(())
        } else {
            Err(anyhow!("Worker {} not found for role {}", worker_id, role))
        }
    }

    /// Find an idle worker for the role, dequeue from level-1, assign the task.
    /// Returns (worker_id, task) on success.
    #[allow(dead_code)]
    pub fn assign_next(&self, role: &str) -> Option<(String, ScheduledTask)> {
        // Find an idle worker for this role
        let idle_worker = {
            let workers = self.workers.lock().ok()?;
            let assignments = self.assignments.lock().ok()?;
            workers
                .get(role)?
                .iter()
                .find(|wid| !assignments.contains_key(*wid))
                .cloned()
        };

        let worker_id = idle_worker?;

        // Dequeue a task from level-1
        let task = self.level1.dequeue(role)?;

        // Assign
        if let Ok(mut assignments) = self.assignments.lock() {
            assignments.insert(worker_id.clone(), task.task_id.clone());
        }

        debug!(
            "Assigned task {} to worker {} (role {})",
            task.task_id, worker_id, role
        );
        Some((worker_id, task))
    }

    /// Complete the task assigned to worker
    #[allow(dead_code)]
    pub fn complete_task(&self, worker_id: &str) -> Result<()> {
        let task_id = {
            let mut assignments = self
                .assignments
                .lock()
                .map_err(|e| anyhow!("Lock error: {}", e))?;
            assignments
                .remove(worker_id)
                .ok_or_else(|| anyhow!("No active assignment for worker {}", worker_id))?
        };
        self.level1.complete(&task_id)
    }

    /// Submit multiple tasks as a fan-out group. All tasks share the same role
    /// and are submitted to level-1. Returns group_id.
    #[allow(dead_code)]
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
        if let Ok(mut stats) = self.level1.stats.lock() {
            stats.l2_fan_out_count += 1;
        }

        info!("Created fan-out group {} with {} tasks", group_id, count);
        Ok(group_id)
    }

    /// Return (completed_count, total_count) for a fan-out group
    #[allow(dead_code)]
    pub fn fan_out_progress(&self, group_id: &str) -> Result<(usize, usize)> {
        let groups = self
            .fan_out_groups
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        let task_ids = groups
            .get(group_id)
            .ok_or_else(|| anyhow!("Fan-out group {} not found", group_id))?;
        let total = task_ids.len();

        let task_map = self
            .level1
            .task_map
            .lock()
            .map_err(|e| anyhow!("Lock error: {}", e))?;
        // Count tasks that are no longer in task_map (completed) or are not active
        let completed = task_ids
            .iter()
            .filter(|id| !task_map.contains_key(*id))
            .count();

        Ok((completed, total))
    }

    /// Aggregate profile from level-1 + worker stats
    #[allow(dead_code)]
    pub fn profile(&self) -> SchedulerProfile {
        let mut profile = self.level1.profile();
        if let Ok(assignments) = self.assignments.lock() {
            profile.l2_active_workers = assignments.len() as u32;
        }
        if let Ok(groups) = self.fan_out_groups.lock() {
            profile.l2_fan_out_count = groups.len() as u32;
        }
        profile
    }
}

/// Helper: current timestamp in milliseconds for unique fan-out group IDs.
#[allow(dead_code)]
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
        let first = scheduler.dequeue("worker").unwrap();
        assert_eq!(first.task_id, "task-high");

        let second = scheduler.dequeue("worker").unwrap();
        assert_eq!(second.task_id, "task-med");

        let third = scheduler.dequeue("worker").unwrap();
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

        // Dequeue two tasks
        assert!(scheduler.dequeue("role-a").is_some());
        assert!(scheduler.dequeue("role-b").is_some());

        // Global cap should prevent third dequeue
        assert!(scheduler.dequeue("role-c").is_none());

        // Complete one, then we can dequeue again
        scheduler.complete("t1").unwrap();
        assert!(scheduler.dequeue("role-c").is_some());
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
        let task_a = scheduler.dequeue("same-role").unwrap();
        let _task_b = scheduler.dequeue("same-role").unwrap();
        // Role cap should prevent third dequeue
        assert!(scheduler.dequeue("same-role").is_none());

        // Complete the dequeued task, then dequeue should work
        scheduler.complete(&task_a.task_id).unwrap();
        let task_c = scheduler.dequeue("same-role").unwrap();
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
                .task_map
                .lock()
                .unwrap()
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
            .task_map
            .lock()
            .unwrap()
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

        // Simulate aging
        {
            let mut last = scheduler.last_aging.lock().unwrap();
            *last = Instant::now() - Duration::from_secs(10);
        }
        scheduler.apply_aging();

        // Now submit a high-priority task
        let high_task = make_task("high-prio", "role", 100, 1000.0);
        scheduler.submit(high_task).unwrap();

        // Without aging, high-prio would be dequeued first.
        // With aging, the aged low-prio task may surpass it.
        let aged_bonus = {
            let task_map = scheduler.task_map.lock().unwrap();
            task_map.get("low-prio").unwrap().aging_bonus
        };

        // Since 10 + aged_bonus < 1000 (aging_rate * 10s = 1000, capped at 500),
        // high-prio should still be dequeued first by default.
        // Let's verify that aging has been applied.
        assert!(aged_bonus > 0.0, "Aging should have provided a bonus");

        // Now dequeue and check order. High priority should still win.
        let first = scheduler.dequeue("role").unwrap();
        assert_eq!(first.task_id, "high-prio", "High priority should still win");

        // But the low-prio task should have a significant aging bonus
        let bonus = scheduler
            .task_map
            .lock()
            .unwrap()
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
        let task = scheduler.dequeue("role").unwrap();
        assert_eq!(task.task_id, "t1");

        // Complete it
        scheduler.complete("t1").unwrap();
        assert!(scheduler.active_tasks.lock().unwrap().is_empty());
        let profile = scheduler.profile();
        assert_eq!(profile.total_completed, 1);

        // Test fail + requeue
        scheduler.submit(make_task("t2", "role", 2, 20.0)).unwrap();
        let _ = scheduler.dequeue("role").unwrap();

        // Fail with requeue
        scheduler.fail("t2", true).unwrap();
        assert!(scheduler.active_tasks.lock().unwrap().is_empty());
        assert!(scheduler.task_map.lock().unwrap().contains_key("t2"));

        // Verify retry count incremented
        let retries = scheduler
            .task_map
            .lock()
            .unwrap()
            .get("t2")
            .unwrap()
            .retries;
        assert_eq!(retries, 1);

        // Dequeue again, complete, verify stats
        let _ = scheduler.dequeue("role").unwrap();
        scheduler.complete("t2").unwrap();
        assert!(!scheduler.task_map.lock().unwrap().contains_key("t2"));

        // Test fail without requeue
        scheduler.submit(make_task("t3", "role", 3, 30.0)).unwrap();
        let _ = scheduler.dequeue("role").unwrap();
        scheduler.fail("t3", false).unwrap();
        assert!(!scheduler.task_map.lock().unwrap().contains_key("t3"));
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
        let (worker_id, task) = l2.assign_next("coder").unwrap();
        assert_eq!(task.role, "coder");
        assert!(worker_id == "worker-alpha" || worker_id == "worker-beta");

        let (worker_id2, task2) = l2.assign_next("coder").unwrap();
        assert_eq!(task2.role, "coder");
        assert_ne!(
            worker_id, worker_id2,
            "Two different workers should be assigned"
        );

        // All coder workers busy, next assign should be None
        assert!(l2.assign_next("coder").is_none());

        // Assign for reviewer role
        let (worker_id3, task3) = l2.assign_next("reviewer").unwrap();
        assert_eq!(worker_id3, "worker-gamma");
        assert_eq!(task3.task_id, "review-task");

        // Complete one coder task
        l2.complete_task(&worker_id).unwrap();

        // Submit a new task so we have something to assign
        let task4 = make_task("code-task-3", "coder", 4, 150.0);
        l2.level1.submit(task4).unwrap();

        // Now we can assign again for coder
        let (worker_id4, _) = l2.assign_next("coder").unwrap();
        assert!(worker_id4 == "worker-alpha" || worker_id4 == "worker-beta");
    }
}
