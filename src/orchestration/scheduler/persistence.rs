use super::{SchedulerConfig, TaskScheduler};

/// Factory: create a plain in-memory scheduler.
///
/// The `db_path` argument is accepted for backward compatibility
/// but is ignored — persistence is no longer used.
pub fn create_persistent_scheduler(_db_path: Option<std::path::PathBuf>) -> TaskScheduler {
    TaskScheduler::new(SchedulerConfig::default())
}
