#[cfg(feature = "backend-sqlite")]
use anyhow::Result;
#[cfg(feature = "backend-sqlite")]
use std::path::PathBuf;
#[cfg(feature = "backend-sqlite")]
use tracing::{debug, info};

#[cfg(feature = "backend-sqlite")]
use super::priority::{Priority, ScheduledTask};
use super::{SchedulerConfig, TaskScheduler};

// ──────────────────────────────────────────────
// Scheduler Persistence (SQLite-backed, feature-gated)
// ──────────────────────────────────────────────

/// SQLite-backed persistence for scheduler state.
///
/// Persists the task queue so that pending tasks survive process restarts.
/// Only compiled when the `backend-sqlite` feature is enabled.
#[cfg(feature = "backend-sqlite")]
pub struct SchedulerPersistence {
    /// Path to the SQLite database file.  `None` disables persistence.
    db_path: Option<PathBuf>,
}

#[cfg(feature = "backend-sqlite")]
impl SchedulerPersistence {
    /// Create a new persistence layer.
    ///
    /// Pass `Some(path)` to enable persistence to a SQLite file.
    /// Pass `None` to disable persistence (all methods become no-ops).
    pub fn new(db_path: Option<PathBuf>) -> Self {
        Self { db_path }
    }

    /// Whether persistence is enabled.
    pub fn is_enabled(&self) -> bool {
        self.db_path.is_some()
    }

    /// Initialize the database schema (idempotent).
    fn ensure_schema(&self, conn: &rusqlite::Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scheduler_queue (
                task_id    TEXT PRIMARY KEY,
                role       TEXT NOT NULL,
                priority   INTEGER NOT NULL,
                base_score REAL NOT NULL,
                urgency    REAL NOT NULL,
                cost_efficiency REAL NOT NULL,
                deadline_pressure REAL NOT NULL,
                aging_bonus REAL NOT NULL,
                submitted_at INTEGER NOT NULL,
                retries    INTEGER NOT NULL,
                max_retries INTEGER NOT NULL
            );",
        )?;
        Ok(())
    }

    /// Serialize all pending tasks and write them to the database.
    pub fn snapshot_queue(&self, tasks: &[ScheduledTask]) -> Result<()> {
        let db_path = match &self.db_path {
            Some(p) => p,
            None => return Ok(()),
        };
        let conn = rusqlite::Connection::open(db_path)?;
        self.ensure_schema(&conn)?;

        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM scheduler_queue", [])?;

        let mut stmt = tx.prepare(
            "INSERT INTO scheduler_queue
             (task_id, role, priority, base_score, urgency,
              cost_efficiency, deadline_pressure, aging_bonus,
              submitted_at, retries, max_retries)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        )?;

        for task in tasks {
            stmt.execute(rusqlite::params![
                task.task_id,
                task.role,
                task.priority.0,
                task.base_score,
                task.urgency,
                task.cost_efficiency,
                task.deadline_pressure,
                task.aging_bonus,
                task.submitted_at,
                task.retries,
                task.max_retries,
            ])?;
        }
        drop(stmt);
        tx.commit()?;

        debug!(
            "Persisted {} scheduler tasks to {}",
            tasks.len(),
            db_path.display()
        );
        Ok(())
    }

    /// Restore the task queue from the database.
    pub fn restore_queue(&self) -> Result<Vec<ScheduledTask>> {
        let db_path = match &self.db_path {
            Some(p) => p,
            None => return Ok(Vec::new()),
        };
        if !db_path.exists() {
            return Ok(Vec::new());
        }

        let conn = rusqlite::Connection::open(db_path)?;
        self.ensure_schema(&conn)?;

        let mut stmt = conn.prepare(
            "SELECT task_id, role, priority, base_score, urgency,
                    cost_efficiency, deadline_pressure, aging_bonus,
                    submitted_at, retries, max_retries
             FROM scheduler_queue
             ORDER BY submitted_at ASC",
        )?;

        let tasks: Vec<ScheduledTask> = stmt
            .query_map([], |row| {
                Ok(ScheduledTask {
                    task_id: row.get(0)?,
                    role: row.get(1)?,
                    priority: Priority(row.get(2)?),
                    base_score: row.get(3)?,
                    urgency: row.get(4)?,
                    cost_efficiency: row.get(5)?,
                    deadline_pressure: row.get(6)?,
                    aging_bonus: row.get(7)?,
                    submitted_at: row.get(8)?,
                    retries: row.get::<_, u32>(9)?,
                    max_retries: row.get::<_, u32>(10)?,
                    provider: None,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        info!(
            "Restored {} scheduler tasks from {}",
            tasks.len(),
            db_path.display()
        );
        Ok(tasks)
    }

    /// Remove a single task from the persistence store (called on completion).
    pub fn remove_task(&self, task_id: &str) -> Result<()> {
        let db_path = match &self.db_path {
            Some(p) => p,
            None => return Ok(()),
        };
        if !db_path.exists() {
            return Ok(());
        }
        let conn = rusqlite::Connection::open(db_path)?;
        conn.execute(
            "DELETE FROM scheduler_queue WHERE task_id = ?1",
            rusqlite::params![task_id],
        )?;
        Ok(())
    }

    /// Persist a single task (called on submit).
    pub fn save_task(&self, task: &ScheduledTask) -> Result<()> {
        let db_path = match &self.db_path {
            Some(p) => p,
            None => return Ok(()),
        };
        let conn = rusqlite::Connection::open(db_path)?;
        self.ensure_schema(&conn)?;
        conn.execute(
            "INSERT OR REPLACE INTO scheduler_queue
             (task_id, role, priority, base_score, urgency,
              cost_efficiency, deadline_pressure, aging_bonus,
              submitted_at, retries, max_retries)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                task.task_id,
                task.role,
                task.priority.0,
                task.base_score,
                task.urgency,
                task.cost_efficiency,
                task.deadline_pressure,
                task.aging_bonus,
                task.submitted_at,
                task.retries,
                task.max_retries,
            ],
        )?;
        Ok(())
    }
}

/// Factory: create a scheduler with SQLite-backed persistence when
/// the `backend-sqlite` feature is enabled.  Falls back to an in-memory
/// scheduler otherwise.
#[cfg(feature = "backend-sqlite")]
pub fn create_persistent_scheduler(db_path: Option<std::path::PathBuf>) -> TaskScheduler {
    let config = SchedulerConfig::default();
    let persistence = SchedulerPersistence::new(db_path);
    let scheduler = TaskScheduler::new_with_persistence(config, persistence);
    // Persist empty state to initialise the database.
    let _ = scheduler.persist_all();
    scheduler
}

/// Fallback when the `backend-sqlite` feature is disabled — creates an
/// in-memory scheduler.
#[cfg(not(feature = "backend-sqlite"))]
pub fn create_persistent_scheduler(_db_path: Option<std::path::PathBuf>) -> TaskScheduler {
    TaskScheduler::new(SchedulerConfig::default())
}
