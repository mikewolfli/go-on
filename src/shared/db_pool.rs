//! Database I/O concurrency management.
//!
//! Provides a shared Semaphore that limits concurrent database operations,
//! preventing the async runtime from being overwhelmed by blocking I/O.
//! Each store wraps its `spawn_blocking` calls with a permit from this
//! semaphore.  No additional connection-pool dependencies are required.

use std::sync::LazyLock;
use tokio::sync::Semaphore;

/// Maximum number of concurrent database I/O operations allowed across
/// all stores (cache, vector, warm, task_graph, session).
const MAX_CONCURRENT_DB_OPS: usize = 4;

/// Global semaphore that limits concurrent DB I/O operations.
static DB_IO_SEM: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(MAX_CONCURRENT_DB_OPS));

/// Acquire a permit for a database I/O operation.
///
/// Hold the returned permit for the duration of the `spawn_blocking` call
/// to limit concurrent blocking DB operations across all stores.
pub async fn acquire_db_permit() -> tokio::sync::SemaphorePermit<'static> {
    DB_IO_SEM.acquire().await.expect("DB I/O semaphore closed")
}
