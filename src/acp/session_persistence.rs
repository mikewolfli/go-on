//! Async SQLite persistence for ACP sessions.
//!
//! Provides persistent storage for `AcpSessionState` across server restarts.
//! Uses `rusqlite` (already a dependency) with `tokio::task::spawn_blocking` for
//! async compatibility.
//!
//! This module is only available when the `backend-sqlite` feature is enabled.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// A persisted ACP session record.
///
/// Mirrors the fields of [`crate::acp::r#impl::request::protocol_pack::AcpSessionState`]
/// with the addition of a stable database key and timestamps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    pub session_id: String,
    pub cwd: Option<String>,
    pub mode: String,
    pub additional_directories: Vec<String>,
    pub config_options: std::collections::HashMap<String, serde_json::Value>,
    pub created_at_ms: i64,
    pub last_active_ms: i64,
}

/// Async SQLite-backed session store.
///
/// Wraps a single `rusqlite::Connection` behind a `std::sync::Mutex` so that
/// all database operations are serialised through `tokio::task::spawn_blocking`,
/// keeping the async runtime free of blocking I/O.
pub struct SessionStore {
    _db_path: PathBuf,
    /// Synchronous connection locked inside `spawn_blocking` closures.
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl SessionStore {
    /// Open (or create) the session database at `db_path`.
    ///
    /// Creates the `acp_sessions` table if it does not already exist.
    pub async fn open(db_path: &Path) -> Result<Self> {
        let path = db_path.to_path_buf();
        let conn = Self::open_sync(&path).await?;
        Ok(Self {
            _db_path: path,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    async fn open_sync(path: &Path) -> Result<rusqlite::Connection> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS acp_sessions (
                    session_id TEXT PRIMARY KEY,
                    cwd TEXT,
                    mode TEXT NOT NULL DEFAULT 'ask',
                    additional_directories TEXT NOT NULL DEFAULT '[]',
                    config_options TEXT NOT NULL DEFAULT '{}',
                    created_at_ms INTEGER NOT NULL,
                    last_active_ms INTEGER NOT NULL
                );",
            )?;
            Ok::<_, anyhow::Error>(conn)
        })
        .await?
    }

    /// Store a session (insert or update).
    ///
    /// Uses `INSERT … ON CONFLICT … DO UPDATE` so that re-`session/resume` calls
    /// refresh the row rather than erroring.
    pub async fn upsert(&self, session: &PersistedSession) -> Result<()> {
        let session_id = session.session_id.clone();
        let cwd = session.cwd.clone();
        let mode = session.mode.clone();
        let additional_dirs = serde_json::to_string(&session.additional_directories)?;
        let config_opts = serde_json::to_string(&session.config_options)?;
        let created_at = session.created_at_ms;
        let last_active = session.last_active_ms;
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            guard.execute(
                "INSERT INTO acp_sessions (session_id, cwd, mode, additional_directories, config_options, created_at_ms, last_active_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(session_id) DO UPDATE SET
                    cwd = excluded.cwd,
                    mode = excluded.mode,
                    additional_directories = excluded.additional_directories,
                    config_options = excluded.config_options,
                    last_active_ms = excluded.last_active_ms",
                rusqlite::params![session_id, cwd, mode, additional_dirs, config_opts, created_at, last_active],
            )?;
            Ok::<_, anyhow::Error>(())
        }).await??;
        Ok(())
    }

    /// Load a single session by ID.
    ///
    /// Returns `None` when no row matches `session_id`.
    pub async fn load(&self, session_id: &str) -> Result<Option<PersistedSession>> {
        let session_id = session_id.to_string();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            let mut stmt = guard.prepare(
                "SELECT session_id, cwd, mode, additional_directories, config_options, created_at_ms, last_active_ms
                 FROM acp_sessions WHERE session_id = ?1"
            )?;
            let mut rows = stmt.query(rusqlite::params![session_id])?;
            if let Some(row) = rows.next()? {
                let sid: String = row.get(0)?;
                let cwd: Option<String> = row.get(1)?;
                let mode: String = row.get(2)?;
                let additional_dirs_str: String = row.get(3)?;
                let config_opts_str: String = row.get(4)?;
                let created_at: i64 = row.get(5)?;
                let last_active: i64 = row.get(6)?;

                let additional_directories: Vec<String> =
                    serde_json::from_str(&additional_dirs_str).unwrap_or_default();
                let config_options: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_str(&config_opts_str).unwrap_or_default();

                Ok(Some(PersistedSession {
                    session_id: sid,
                    cwd,
                    mode,
                    additional_directories,
                    config_options,
                    created_at_ms: created_at,
                    last_active_ms: last_active,
                }))
            } else {
                Ok(None)
            }
        }).await?
    }

    /// Delete a session by ID.
    ///
    /// Returns `true` when a row was actually deleted.
    pub async fn delete(&self, session_id: &str) -> Result<bool> {
        let session_id = session_id.to_string();
        let conn = self.conn.clone();
        let affected = tokio::task::spawn_blocking(move || {
            conn.lock().unwrap().execute(
                "DELETE FROM acp_sessions WHERE session_id = ?1",
                rusqlite::params![session_id],
            )
        })
        .await??;
        Ok(affected > 0)
    }

    /// List all persisted sessions ordered by `last_active_ms` descending.
    pub async fn list_all(&self) -> Result<Vec<PersistedSession>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            let mut stmt = guard.prepare(
                "SELECT session_id, cwd, mode, additional_directories, config_options, created_at_ms, last_active_ms
                 FROM acp_sessions ORDER BY last_active_ms DESC"
            )?;
            let sessions = stmt.query_map([], |row| {
                let sid: String = row.get(0)?;
                let cwd: Option<String> = row.get(1)?;
                let mode: String = row.get(2)?;
                let additional_dirs_str: String = row.get(3)?;
                let config_opts_str: String = row.get(4)?;
                let created_at: i64 = row.get(5)?;
                let last_active: i64 = row.get(6)?;

                let additional_directories: Vec<String> =
                    serde_json::from_str(&additional_dirs_str).unwrap_or_default();
                let config_options: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_str(&config_opts_str).unwrap_or_default();

                Ok(PersistedSession {
                    session_id: sid,
                    cwd,
                    mode,
                    additional_directories,
                    config_options,
                    created_at_ms: created_at,
                    last_active_ms: last_active,
                })
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok(sessions)
        }).await?
    }

    /// Remove sessions that haven't been active since `before_ms`.
    ///
    /// Returns the number of deleted rows.
    pub async fn cleanup_stale(&self, before_ms: i64) -> Result<usize> {
        let conn = self.conn.clone();
        let affected = tokio::task::spawn_blocking(move || {
            conn.lock().unwrap().execute(
                "DELETE FROM acp_sessions WHERE last_active_ms < ?1",
                rusqlite::params![before_ms],
            )
        })
        .await??;
        Ok(affected)
    }
}
