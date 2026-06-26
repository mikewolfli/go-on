use super::{SchedulerConfig, TaskScheduler};

/// Factory: create a plain in-memory scheduler.
pub fn create_in_memory_scheduler() -> TaskScheduler {
    TaskScheduler::new(SchedulerConfig::default())
}
