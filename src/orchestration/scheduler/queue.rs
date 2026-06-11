use std::collections::{BinaryHeap, HashMap};

use super::priority::ScheduledTask;

/// Merged state for queues and task_map behind a single RwLock.
///
/// ⚠️  DEADLOCK PREVENTION:
/// Lock ordering across the scheduler:
///   1. `state` (RwLock on SchedulerState) — highest priority
///   2. `active` (Mutex) — acquired after state
///   3. `stats`, `last_aging`, `role_limiters` — acquired independently
///
/// Never acquire `active` then `state` in that order.
pub(super) struct SchedulerState {
    /// Per-role priority queues of pending tasks (role → heap)
    pub(super) queues: HashMap<String, BinaryHeap<ScheduledTask>>,
    /// Task lookup by ID (includes both pending and active tasks)
    pub(super) task_map: HashMap<String, ScheduledTask>,
}
