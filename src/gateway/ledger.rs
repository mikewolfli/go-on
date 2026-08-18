//! SQLite-backed delivery ledger (M3.4, gated on `backend-sqlite`).
//!
//! Records every rendered reply delivery per `(platform, chat_id,
//! message_hash)` so a platform redelivery (retry of an identical inbound
//! payload) is answered idempotently instead of running a second agent turn.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Deliveries table: one row per delivered reply, keyed by the dedup triple.
const CREATE_DELIVERIES_TABLE: &str = "CREATE TABLE IF NOT EXISTS deliveries (
    platform     TEXT NOT NULL,
    chat_id      TEXT NOT NULL,
    message_hash TEXT NOT NULL,
    delivered_at INTEGER NOT NULL,
    ok           INTEGER NOT NULL,
    PRIMARY KEY (platform, chat_id, message_hash)
)";

/// Delivery ledger: dedup store for rendered replies (M3.4).
///
/// One SQLite connection guarded by a mutex — the gateway listener serves
/// concurrent requests, and every operation is a single short statement.
pub struct DeliveryLedger {
    conn: Mutex<Connection>,
}

impl DeliveryLedger {
    /// Open (creating on first use) the ledger database at `path`. Passing
    /// `":memory:"` opens a non-persistent in-memory ledger (unit tests,
    /// ephemeral gateways).
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("open delivery ledger at {}", path.display()))?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(CREATE_DELIVERIES_TABLE)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Record a delivered reply for a message. Re-recording the same
    /// `(platform, chat_id, message_hash)` is a no-op (idempotent).
    pub fn record_delivery(&self, platform: &str, chat_id: &str, message_hash: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR IGNORE INTO deliveries (platform, chat_id, message_hash, delivered_at, ok)
             VALUES (?1, ?2, ?3, ?4, 1)",
            rusqlite::params![platform, chat_id, message_hash, unix_timestamp_secs()],
        )?;
        Ok(())
    }

    /// Whether a reply was already delivered for this message (dedup key).
    pub fn already_delivered(
        &self,
        platform: &str,
        chat_id: &str,
        message_hash: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM deliveries
             WHERE platform = ?1 AND chat_id = ?2 AND message_hash = ?3",
            rusqlite::params![platform, chat_id, message_hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_then_dedup_round_trip() {
        let ledger =
            DeliveryLedger::open(std::path::Path::new(":memory:")).expect("in-memory ledger");
        assert!(!ledger
            .already_delivered("telegram", "chat-1", "hash-a")
            .expect("query succeeds"));

        ledger
            .record_delivery("telegram", "chat-1", "hash-a")
            .expect("record succeeds");
        assert!(ledger
            .already_delivered("telegram", "chat-1", "hash-a")
            .expect("query succeeds"));

        // Re-recording the same triple is idempotent (single row).
        ledger
            .record_delivery("telegram", "chat-1", "hash-a")
            .expect("re-record succeeds");

        // Every other dedup coordinate is a distinct delivery.
        assert!(!ledger
            .already_delivered("telegram", "chat-2", "hash-a")
            .expect("query succeeds"));
        assert!(!ledger
            .already_delivered("wecom", "chat-1", "hash-a")
            .expect("query succeeds"));
        assert!(!ledger
            .already_delivered("telegram", "chat-1", "hash-b")
            .expect("query succeeds"));
    }

    #[test]
    fn ledger_persists_to_disk_and_reopens() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("deliveries.sqlite3");
        {
            let ledger = DeliveryLedger::open(&path).expect("open ledger");
            ledger
                .record_delivery("wecom", "chat-7", "h")
                .expect("record succeeds");
        }
        let reopened = DeliveryLedger::open(&path).expect("reopen ledger");
        assert!(reopened
            .already_delivered("wecom", "chat-7", "h")
            .expect("query succeeds"));
        assert!(!reopened
            .already_delivered("wecom", "chat-8", "h")
            .expect("query succeeds"));
    }
}
