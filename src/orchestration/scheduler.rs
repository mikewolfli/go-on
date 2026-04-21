//! S8+S9: Multi-priority Dual Scheduler
//!
//! Provides a scheduler that routes tasks into prioritized queues and a
//! companion worker-side scheduler that handles dequeue, timeout, and
//! backpressure signalling.  Two structs for separation of concerns:
//!
//! - `TaskScheduler`      — producer side (enqueue, priority assignment)
//! - `WorkerScheduler`    — consumer side (dequeue, deadline enforcement)

#![allow(dead_code)]

use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Task priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Background = 0,
    Normal     = 1,
    High       = 2,
    Critical   = 3,
}

impl Default for TaskPriority {
    fn default() -> Self { TaskPriority::Normal }
}

/// A scheduled task entry
#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: String,
    pub priority: TaskPriority,
    pub deadline: Option<Instant>,
    pub payload: serde_json::Value,
}

// Ord impl so BinaryHeap gives highest priority first
impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool { self.priority == other.priority }
}
impl Eq for ScheduledTask {}
impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority.cmp(&other.priority)
    }
}

/// Configuration for the scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Maximum number of tasks allowed in the queue (0 = unlimited)
    #[serde(default = "default_max_queue")]
    pub max_queue_depth: usize,
    /// Default task deadline in seconds (0 = no deadline)
    #[serde(default)]
    pub default_deadline_seconds: u64,
    /// Number of worker threads / slots
    #[serde(default = "default_workers")]
    pub worker_slots: usize,
}

fn default_enabled() -> bool { true }
fn default_max_queue() -> usize { 1000 }
fn default_workers() -> usize { 4 }

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self { enabled: true, max_queue_depth: 1000, default_deadline_seconds: 0, worker_slots: 4 }
    }
}

/// Producer-side scheduler
pub struct TaskScheduler {
    config: SchedulerConfig,
    queue: Arc<Mutex<BinaryHeap<ScheduledTask>>>,
}

impl TaskScheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        Self { config, queue: Arc::new(Mutex::new(BinaryHeap::new())) }
    }

    /// Enqueue a task.  Returns Err if the queue is at capacity.
    pub fn enqueue(&self, mut task: ScheduledTask) -> Result<(), &'static str> {
        if !self.config.enabled { return Err("scheduler_disabled"); }
        // Set default deadline from config if not provided
        if task.deadline.is_none() && self.config.default_deadline_seconds > 0 {
            task.deadline = Some(Instant::now() + Duration::from_secs(self.config.default_deadline_seconds));
        }
        let mut q = self.queue.lock().map_err(|_| "lock_poisoned")?;
        if self.config.max_queue_depth > 0 && q.len() >= self.config.max_queue_depth {
            return Err("queue_at_capacity");
        }
        q.push(task);
        Ok(())
    }

    pub fn queue_depth(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn queue_handle(&self) -> Arc<Mutex<BinaryHeap<ScheduledTask>>> {
        self.queue.clone()
    }
}

/// Consumer-side scheduler (worker dispatcher)
pub struct WorkerScheduler {
    config: SchedulerConfig,
    queue: Arc<Mutex<BinaryHeap<ScheduledTask>>>,
    /// Active task count tracker (simple u32 for now)
    active: Arc<Mutex<u32>>,
}

impl WorkerScheduler {
    pub fn new(config: SchedulerConfig, queue: Arc<Mutex<BinaryHeap<ScheduledTask>>>) -> Self {
        Self {
            config,
            queue,
            active: Arc::new(Mutex::new(0)),
        }
    }

    /// Dequeue the highest-priority non-expired task.
    /// Returns None when queue is empty or all workers are occupied.
    pub fn dequeue(&self) -> Option<ScheduledTask> {
        let active = self.active.lock().ok()?;
        if *active >= self.config.worker_slots as u32 {
            return None; // backpressure
        }
        drop(active);

        let mut q = self.queue.lock().ok()?;
        let now = Instant::now();
        // Drain expired tasks
        while let Some(top) = q.peek() {
            if let Some(deadline) = top.deadline {
                if deadline < now {
                    q.pop();
                    continue;
                }
            }
            break;
        }
        let task = q.pop()?;
        *self.active.lock().ok()? += 1;
        Some(task)
    }

    /// Signal that a worker has completed its task
    pub fn complete(&self) {
        if let Ok(mut active) = self.active.lock() {
            *active = active.saturating_sub(1);
        }
    }

    pub fn active_count(&self) -> u32 {
        self.active.lock().map(|a| *a).unwrap_or(0)
    }
}
