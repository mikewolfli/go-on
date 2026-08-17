//! M3.3: user-level cron — durable SQLite job store + schedule evaluation.
//!
//! # Scheduling semantics
//!
//! - **First run**: a newly added job's `next_run_at` is the first match of its
//!   expression strictly after `now` — a job is never due immediately.
//! - **After each run**: `next_run_at` is the next match strictly after the
//!   run's completion time (interval-from-completion). Fire times that elapsed
//!   while the job was running are skipped, never queued.
//! - **Overlap prevention**: a job whose run is in flight is never due again.
//!   The in-memory `running` set inside the store is the chokepoint: the tick
//!   only spawns a job after `begin_run` has claimed it. The set is in-memory
//!   on purpose — it is lost on crash, and the orphaned `cron_runs` row then
//!   drives crash recovery instead.
//! - **Crash recovery**: rows in `cron_runs` without a `finished_at` are marked
//!   crashed by [`CronStore::recover_crashed_runs`] (called once when the
//!   server's tick loop starts). The job's `next_run_at` was never advanced, so
//!   it becomes due again on the next tick — a missed fire while the server was
//!   down collapses into a single catch-up run.
//! - **Enable/disable**: disabling leaves `next_run_at` untouched. Re-enabling
//!   resets the schedule to the first future match when `next_run_at` is
//!   missing or already past, so re-enabling never fires a backlog of missed
//!   runs.
//!
//! When `next_run_at` is `None` the job is not schedulable (its expression
//! cannot be matched again, or it was never scheduled) and it will never be
//! due.

use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use chrono::{TimeZone, Utc};
use croner::Cron;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single user-level cron job.
///
/// `next_run_at` / `last_run_at` are epoch seconds (see the module docs for
/// the scheduling semantics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub expression: String,
    /// The `workflow.execute` params the job runs with when it fires.
    pub payload: Value,
    pub enabled: bool,
    pub created_at: i64,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
}

impl CronJob {
    /// Build a new job. `created_at` is set to now; `next_run_at` is filled in
    /// by [`CronStore::add`] (first future match of the expression).
    pub fn new(id: String, expression: String, payload: Value, enabled: bool) -> Self {
        Self {
            id,
            expression,
            payload,
            enabled,
            created_at: crate::shared::timestamps::now_ts(),
            last_run_at: None,
            next_run_at: None,
        }
    }
}

/// Canonical on-disk location of the cron job store (`.goon/cron/cron.db`,
/// mirroring the `goon_subdir("memory")` convention of `memory_base_path`).
pub fn cron_db_path() -> std::path::PathBuf {
    crate::shared::goon_paths::goon_subdir("cron").join("cron.db")
}

/// Parse a cron expression with the `croner` crate — the single validation
/// point shared by the CLI (`go-on cron add`) and the store.
pub fn parse_schedule(expression: &str) -> Result<Cron> {
    Cron::from_str(expression).map_err(|e| anyhow!("invalid cron expression '{expression}': {e}"))
}

/// Next fire time (epoch seconds) strictly after `after_ts` for `expression`.
///
/// The search starts from the second after `after_ts` (`inclusive = false`),
/// so a fire time that has already passed is never returned and a freshly
/// added job never fires immediately. Errors when the expression can never be
/// matched again (e.g. Feb 30), so callers can reject unschedulable schedules
/// at add time and stop rescheduling a job whose expression can no longer fire.
pub fn next_fire_after(expression: &str, after_ts: i64) -> Result<i64> {
    let schedule = parse_schedule(expression)?;
    let after = Utc
        .timestamp_opt(after_ts, 0)
        .single()
        .ok_or_else(|| anyhow!("timestamp {after_ts} is out of range for cron scheduling"))?;
    let next = schedule.find_next_occurrence(&after, false).map_err(|e| {
        anyhow!("cannot find a future fire time for '{expression}' after {after_ts}: {e}")
    })?;
    Ok(next.timestamp())
}

/// Shared, locked store state: the SQLite connection plus the in-memory set of
/// job ids with a run currently in flight.
struct CronStoreInner {
    conn: Connection,
    /// Overlap prevention: ids of jobs whose run was claimed by `begin_run`
    /// but not yet released by `finish_run`. In-memory only — see the module
    /// docs for why this is intentional (crash recovery).
    running: HashSet<String>,
}

/// Durable SQLite-backed store for user-level cron jobs.
///
/// The lock is intentionally a plain `std::sync::Mutex`: the store is only
/// touched by the tick loop (async context, quick synchronous SQLite ops) and
/// by short-lived CLI commands. A poisoned lock is recovered via
/// `into_inner`, matching the rest of the codebase.
pub struct CronStore {
    inner: Mutex<CronStoreInner>,
}

impl CronStore {
    /// Open (creating if needed) the store at `db_path` and ensure the schema
    /// exists. Run history from a previous process is left untouched here —
    /// [`CronStore::recover_crashed_runs`] is a separate, explicit step the
    /// server tick calls once at startup, so CLI commands never rewrite run
    /// history.
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating cron store directory {}", parent.display()))?;
        }
        let conn = Connection::open(db_path)
            .with_context(|| format!("opening cron store {}", db_path.display()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cron_jobs (
                id TEXT PRIMARY KEY,
                expression TEXT NOT NULL,
                payload TEXT NOT NULL,
                enabled INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                last_run_at INTEGER,
                next_run_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS cron_runs (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                finished_at INTEGER,
                ok INTEGER,
                output TEXT
            );",
        )
        .context("creating cron schema")?;
        Ok(Self {
            inner: Mutex::new(CronStoreInner {
                conn,
                running: HashSet::new(),
            }),
        })
    }

    /// Persist `job`, computing `next_run_at` as the first future match of its
    /// expression (first-run semantics: a new job never fires immediately).
    /// Returns the stored job (with `next_run_at` filled in).
    pub fn add(&self, job: CronJob) -> Result<CronJob> {
        let next_run_at = next_fire_after(&job.expression, crate::shared::timestamps::now_ts())?;
        let mut job = job;
        job.next_run_at = Some(next_run_at);
        let guard = self.lock();
        guard
            .conn
            .execute(
                "INSERT INTO cron_jobs (id, expression, payload, enabled, created_at, last_run_at, next_run_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    job.id.clone(),
                    job.expression.clone(),
                    serde_json::to_string(&job.payload)
                        .context("serializing cron job payload")?,
                    job.enabled as i64,
                    job.created_at,
                    job.last_run_at,
                    job.next_run_at,
                ],
            )
            .with_context(|| format!("inserting cron job {}", job.id))?;
        Ok(job)
    }

    /// Remove a job by id. Returns `true` when a job was deleted. Database
    /// errors are logged and reported as `false` (the store stays usable; the
    /// CLI reports the failure through the "not found" message).
    pub fn remove(&self, id: &str) -> bool {
        let guard = self.lock();
        match guard
            .conn
            .execute("DELETE FROM cron_jobs WHERE id = ?1", [id])
        {
            Ok(rows) => rows > 0,
            Err(e) => {
                tracing::warn!("cron: failed to remove job {id}: {e}");
                false
            }
        }
    }

    /// All jobs, ordered by creation time.
    pub fn list(&self) -> Result<Vec<CronJob>> {
        let guard = self.lock();
        let mut stmt = guard.conn.prepare(
            "SELECT id, expression, payload, enabled, created_at, last_run_at, next_run_at
             FROM cron_jobs
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], row_to_job)?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row?);
        }
        Ok(jobs)
    }

    /// Enable or disable a job. Returns `Ok(true)` when the job existed and
    /// was updated, `Ok(false)` when no such job exists. See the module docs
    /// for the enable/disable scheduling semantics.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool> {
        let now = crate::shared::timestamps::now_ts();
        let guard = self.lock();
        let Some(job) = query_job(&guard.conn, id)? else {
            return Ok(false);
        };
        let next_run_at = if enabled {
            match job.next_run_at {
                Some(next) if next > now => Some(next),
                // Missing or past fire time: reset to the first future match so
                // re-enabling never fires a backlog of missed runs.
                _ => Some(next_fire_after(&job.expression, now)?),
            }
        } else {
            job.next_run_at
        };
        guard
            .conn
            .execute(
                "UPDATE cron_jobs SET enabled = ?1, next_run_at = ?2 WHERE id = ?3",
                params![enabled as i64, next_run_at, id],
            )
            .with_context(|| format!("updating cron job {id}"))?;
        Ok(true)
    }

    /// Jobs that should fire at or before `now_ts`: enabled, with a
    /// `next_run_at` in the past, and not currently running (overlap
    /// prevention).
    pub fn due_jobs(&self, now_ts: i64) -> Result<Vec<CronJob>> {
        let guard = self.lock();
        let mut stmt = guard.conn.prepare(
            "SELECT id, expression, payload, enabled, created_at, last_run_at, next_run_at
             FROM cron_jobs
             WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY next_run_at, id",
        )?;
        let rows = stmt.query_map([now_ts], row_to_job)?;
        let mut due = Vec::new();
        for row in rows {
            let job = row?;
            if !guard.running.contains(&job.id) {
                due.push(job);
            }
        }
        Ok(due)
    }

    /// Claim `id` for a run starting at `started_at`. Returns `Ok(true)` when
    /// the claim succeeded (a `cron_runs` row is inserted and the job is marked
    /// running); `Ok(false)` when the job is already running. This is the
    /// overlap-prevention chokepoint: the tick only spawns a job the store has
    /// successfully claimed, so the same job can never fire twice concurrently.
    pub fn begin_run(&self, id: &str, started_at: i64) -> Result<bool> {
        let mut guard = self.lock();
        if guard.running.contains(id) {
            return Ok(false);
        }
        guard
            .conn
            .execute(
                "INSERT INTO cron_runs (id, job_id, started_at, finished_at, ok, output)
                 VALUES (?1, ?2, ?3, NULL, NULL, '')",
                params![uuid::Uuid::new_v4().to_string(), id, started_at],
            )
            .with_context(|| format!("inserting cron run for job {id}"))?;
        guard.running.insert(id.to_string());
        Ok(true)
    }

    /// Complete a run claimed with [`CronStore::begin_run`] at `started_at`:
    /// record the outcome in `cron_runs`, release the running marker, and
    /// advance `last_run_at` / `next_run_at`. `next_run_at` is the next match
    /// strictly after the completion time (interval-from-completion — fire
    /// times that elapsed while the job ran are skipped, never queued). If the
    /// expression can no longer be matched, `next_run_at` becomes `None` and
    /// the job stops scheduling (visible in `list`).
    pub fn finish_run(&self, id: &str, started_at: i64, ok: bool, output: &str) -> Result<()> {
        let finished_at = crate::shared::timestamps::now_ts();
        let mut guard = self.lock();
        guard
            .conn
            .execute(
                "UPDATE cron_runs SET finished_at = ?1, ok = ?2, output = ?3
                 WHERE job_id = ?4 AND started_at = ?5 AND finished_at IS NULL",
                params![finished_at, ok as i64, output, id, started_at],
            )
            .with_context(|| format!("finalizing cron run for job {id}"))?;
        guard.running.remove(id);
        let expression: Option<String> = guard
            .conn
            .query_row(
                "SELECT expression FROM cron_jobs WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let next_run_at = match expression {
            Some(expression) => match next_fire_after(&expression, finished_at) {
                Ok(next) => Some(next),
                Err(e) => {
                    tracing::warn!(
                        "cron job {id}: cannot compute next fire time after run; job will not reschedule: {e}"
                    );
                    None
                }
            },
            // The job was removed while its run was in flight.
            None => None,
        };
        guard
            .conn
            .execute(
                "UPDATE cron_jobs SET last_run_at = ?1, next_run_at = ?2 WHERE id = ?3",
                params![finished_at, next_run_at, id],
            )
            .with_context(|| format!("advancing schedule of cron job {id}"))?;
        Ok(())
    }

    /// Crash recovery: mark every `cron_runs` row without a `finished_at` as
    /// crashed. Called once when the server's tick loop starts; rows left
    /// in-flight by an abrupt process exit are the only evidence of the lost
    /// run, and their job becomes due again on the next tick (catch-up fire).
    /// Returns the number of runs marked crashed.
    pub fn recover_crashed_runs(&self, now_ts: i64) -> Result<usize> {
        let guard = self.lock();
        let updated = guard
            .conn
            .execute(
                "UPDATE cron_runs
                 SET finished_at = ?1, ok = 0, output = 'crashed: server restarted before the run completed'
                 WHERE finished_at IS NULL",
                [now_ts],
            )
            .context("recovering crashed cron runs")?;
        Ok(updated)
    }

    /// Lock with poison recovery, matching the rest of the codebase.
    fn lock(&self) -> std::sync::MutexGuard<'_, CronStoreInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Map one `cron_jobs` row to a [`CronJob`]. The stored payload is JSON that
/// we wrote ourselves; a corrupt payload is surfaced as a conversion error
/// rather than silently replaced.
fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<CronJob> {
    let payload_json: String = row.get(2)?;
    let payload: Value = match serde_json::from_str(&payload_json) {
        Ok(value) => value,
        Err(e) => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(e),
            ));
        }
    };
    Ok(CronJob {
        id: row.get(0)?,
        expression: row.get(1)?,
        payload,
        enabled: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
        last_run_at: row.get(5)?,
        next_run_at: row.get(6)?,
    })
}

/// Fetch a single job by id (`None` when absent).
fn query_job(conn: &Connection, id: &str) -> Result<Option<CronJob>> {
    let mut stmt = conn.prepare(
        "SELECT id, expression, payload, enabled, created_at, last_run_at, next_run_at
         FROM cron_jobs WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], row_to_job)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;
    use serde_json::json;

    fn test_store() -> (tempfile::TempDir, CronStore) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let store = CronStore::new(&dir.path().join("cron.db")).expect("open store");
        (dir, store)
    }

    fn job(id: &str, expression: &str, enabled: bool) -> CronJob {
        CronJob::new(
            id.to_string(),
            expression.to_string(),
            json!({ "task": id }),
            enabled,
        )
    }

    #[test]
    fn add_list_remove_set_enabled_round_trip() {
        let (_dir, store) = test_store();
        let added = store.add(job("a", "* * * * *", true)).expect("add a");
        assert!(added.next_run_at.is_some());
        assert!(added.next_run_at.unwrap() > crate::shared::timestamps::now_ts());
        let added_b = store.add(job("b", "* * * * *", false)).expect("add b");
        assert!(!added_b.enabled);

        let jobs = store.list().expect("list");
        assert_eq!(jobs.len(), 2);

        assert!(store.set_enabled("a", false).expect("disable a"));
        assert!(!store.set_enabled("missing", false).expect("missing job"));
        let jobs = store.list().expect("list");
        assert!(!jobs.iter().find(|j| j.id == "a").expect("job a").enabled);

        assert!(store.remove("a"));
        assert!(!store.remove("a")); // already gone
        let jobs = store.list().expect("list");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "b");
    }

    #[test]
    fn invalid_expression_is_rejected() {
        let (_dir, store) = test_store();
        assert!(parse_schedule("not a cron expression").is_err());
        assert!(store.add(job("x", "not a cron expression", true)).is_err());
        // An expression that can never match (Feb 30) is also rejected at add.
        assert!(store.add(job("y", "0 0 30 2 *", true)).is_err());
        assert!(store.list().expect("list").is_empty());
    }

    #[test]
    fn due_jobs_respects_next_run_at_and_enabled() {
        let (_dir, store) = test_store();
        // `* * * * *` fires at the next minute boundary — never immediately
        // (first-run semantics: next_run_at is a strictly future match).
        let added = store.add(job("a", "* * * * *", true)).expect("add");
        let next_run_at = added.next_run_at.expect("next run");

        // Not due before its fire time.
        assert!(store.due_jobs(next_run_at - 1).expect("due").is_empty());
        // Due at/after its fire time.
        assert_eq!(store.due_jobs(next_run_at).expect("due").len(), 1);
        // Disabled jobs are never due, even once the fire time passed.
        store.set_enabled("a", false).expect("disable");
        assert!(store.due_jobs(next_run_at).expect("due").is_empty());
    }

    #[test]
    fn five_minute_expression_first_fire_is_a_future_boundary() {
        let now = crate::shared::timestamps::now_ts();
        let next = next_fire_after("*/5 * * * *", now).expect("next fire");
        assert!(next > now);
        let dt = Utc.timestamp_opt(next, 0).single().expect("valid time");
        assert_eq!(dt.second(), 0);
        assert_eq!(dt.minute() % 5, 0);
    }

    #[test]
    fn overlap_prevention_claims_and_releases() {
        let (_dir, store) = test_store();
        let added = store.add(job("a", "* * * * *", true)).expect("add");
        let next_run_at = added.next_run_at.expect("next run");

        // The job is due once its fire time arrives.
        assert_eq!(store.due_jobs(next_run_at).expect("due").len(), 1);
        // Claim the run — the job must no longer be due even though its fire
        // time has arrived, and a second claim is refused while it is in
        // flight (overlap prevention).
        assert!(store.begin_run("a", next_run_at).expect("begin run"));
        assert!(store.due_jobs(next_run_at).expect("due").is_empty());
        assert!(!store.begin_run("a", next_run_at).expect("begin again"));

        // Finishing releases the marker (the job can be claimed again) and
        // records last_run_at.
        store
            .finish_run("a", next_run_at, true, "ok")
            .expect("finish run");
        assert!(store
            .begin_run("a", next_run_at)
            .expect("re-claim after finish"));
        let after = store
            .list()
            .expect("list")
            .into_iter()
            .find(|j| j.id == "a")
            .expect("job a");
        assert!(after.last_run_at.is_some());
    }

    #[test]
    fn next_fire_advances_past_completion() {
        // Interval-from-completion: a run that completes exactly at a fire time
        // advances the schedule by one full period — the just-fired time is
        // never repeated.
        let boundary = Utc
            .with_ymd_and_hms(2026, 8, 17, 10, 5, 0)
            .single()
            .expect("valid time")
            .timestamp();
        assert_eq!(
            next_fire_after("* * * * *", boundary).expect("next fire"),
            boundary + 60
        );
        assert_eq!(
            next_fire_after("*/5 * * * *", boundary).expect("next fire"),
            boundary + 300
        );
    }

    #[test]
    fn crash_recovery_marks_inflight_runs_and_keeps_job_due() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("cron.db");
        let store = CronStore::new(&path).expect("open store");
        store.add(job("a", "* * * * *", true)).expect("add");
        let next_run_at = store.list().expect("list")[0]
            .next_run_at
            .expect("next run");
        store.begin_run("a", next_run_at).expect("begin run");

        // Abrupt exit: no finish_run. A fresh store instance (e.g. after a
        // restart) must recover the orphaned run — and only once.
        let reopened = CronStore::new(&path).expect("reopen store");
        let now = crate::shared::timestamps::now_ts();
        assert_eq!(
            reopened.recover_crashed_runs(now).expect("recover"),
            1,
            "the in-flight run is marked crashed"
        );
        assert_eq!(
            reopened.recover_crashed_runs(now).expect("recover again"),
            0,
            "recovery is idempotent"
        );
        // The job's next_run_at was never advanced, so it is due again
        // (catch-up fire after restart).
        assert_eq!(reopened.due_jobs(next_run_at).expect("due").len(), 1);
    }

    #[test]
    fn cron_job_serde_round_trip() {
        let original = job("a", "*/5 * * * *", true);
        let encoded = serde_json::to_string(&original).expect("serialize");
        let decoded: CronJob = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, original);
    }
}
