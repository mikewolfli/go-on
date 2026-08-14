//! Audit — F-GAP-03
//!
//! Audit logging system for go-on (Phase 2 — thread-safe + persistence)
//!
//! Provides [`ThreadSafeAuditLog`] — thread-safe version with optional NDJSON file persistence.
//!
//! Records agent/tool/phase decisions for compliance and debugging.
//!
//! # Integrity
//!
//! The canonical sink is also the single producer of the tamper-evident hash
//! chain (`audit_chain.ndjson`, sibling of the NDJSON log). Every persisted
//! entry is chained by the same background writer thread, so the entire audit
//! stream (not just select security events) is protected against retroactive
//! modification. The chain primitives live in
//! [`crate::security::audit_integrity`]; this module owns their wiring.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;

use crate::security::audit_integrity::HashChainAuditor;

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<std::io::Error> for AuditError {
    fn from(e: std::io::Error) -> Self {
        AuditError::Storage(e.to_string())
    }
}

impl From<serde_json::Error> for AuditError {
    fn from(e: serde_json::Error) -> Self {
        AuditError::Serialization(e.to_string())
    }
}

// ─── AuditLogEntry (unchanged) ──────────────────────────────────────────────

/// Audit log entry for all agent/tool/phase decisions
///
/// # Related types
/// - [`super::security_governor::AuditEntry`] — a security-policy-specific audit
///   entry used by [`super::security_governor::SecurityGovernor`]. This type is
///   the general-purpose audit record; `AuditEntry` covers only policy evaluations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub timestamp: String,
    pub task_id: String,
    pub phase: String,
    pub agent: Option<String>,
    pub tool: Option<String>,
    pub decision: String,
    pub inputs: serde_json::Value,
    pub outputs: Option<serde_json::Value>,
    pub error: Option<String>,
    pub confidence: Option<f32>,
    #[serde(default)]
    pub data_classification: Option<String>,
    #[serde(default)]
    pub compliance_tags: Vec<String>,
    #[serde(default)]
    pub retention_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

// ─── ThreadSafeAuditLog ─────────────────────────────────────────────────────

/// Thread-safe audit log with optional NDJSON persistence.
///
/// When a `log_path` is configured, every recorded entry is appended as a JSON
/// line to the file. Buffer overflow warnings are emitted via `tracing::warn!`.
///
/// When the active file exceeds 100 MB it is compressed into a gzip archive
/// (`audit.ndjson.1.gz` → `.2.gz` → …) and a fresh file is started. Old
/// archives beyond `max_archives` are deleted automatically.
///
/// Every persisted entry is additionally appended to the sibling hash chain
/// (`audit_chain.ndjson`, same directory) by the same writer thread — this is
/// the single tamper-evidence layer for the whole audit stream. The chain file
/// rotates at the same size threshold; each archive keeps its own intact chain
/// and the fresh file restarts from the genesis hash.
///
/// Cloning shares the same underlying buffer (an `Arc`), so every handle
/// observes the same entries — this is how the process-wide single sink is
/// handed to multiple subsystems.
#[derive(Clone)]
pub struct ThreadSafeAuditLog {
    inner: Arc<Mutex<AuditLogInner>>,
}

struct AuditLogInner {
    entries: VecDeque<AuditLogEntry>,
    max_entries: usize,
    dropped_count: u64,
    /// Channel to the background NDJSON writer thread (set when `log_path` is
    /// configured). The write is fire-and-forget so the request hot path never
    /// performs synchronous disk I/O. Bounded (`SyncSender`): the writer
    /// thread is a single consumer, and `try_send` drops entries when it
    /// falls behind instead of queuing unboundedly.
    writer: Option<std::sync::mpsc::SyncSender<AuditWriterMsg>>,
}

/// Messages consumed by the background audit writer thread.
// Clippy's large_enum_variant is intentionally allowed: `Entry` is the
// request-hot-path message (sent on every audit record); boxing it would add
// a heap allocation per record for no benefit, while `Flush` is a rare
// control message.
#[allow(clippy::large_enum_variant)]
enum AuditWriterMsg {
    /// Append one entry to the NDJSON file.
    Entry(AuditLogEntry),
    /// Acknowledge after all prior messages have been flushed to disk.
    Flush(std::sync::mpsc::Sender<()>),
}

impl ThreadSafeAuditLog {
    /// Return a new `Arc`-cloned handle to the same underlying log.
    #[cfg(test)]
    pub fn clone_arc(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }

    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AuditLogInner {
                entries: VecDeque::new(),
                max_entries,
                dropped_count: 0,
                writer: None,
            })),
        }
    }

    pub fn new_with_path(max_entries: usize, log_path: impl Into<PathBuf>) -> Self {
        let log_path = log_path.into();
        // Create the parent directory once at construction so the per-record
        // append path doesn't need to stat/scan it on every write.
        if let Some(parent) = log_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                tracing::warn!(
                    "audit: failed to create log directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
        // Spawn a dedicated writer thread that owns all NDJSON file I/O. The
        // request hot path (`record`) only pushes to the in-memory buffer and
        // sends the entry over a bounded channel (4096, `sync_channel`) — no
        // synchronous disk write; on overflow the entry is dropped and counted.
        // The same thread also owns the hash-chain append, so the log line and
        // its chain entry are written in exact order by a single producer.
        let (tx, rx) = std::sync::mpsc::sync_channel::<AuditWriterMsg>(4096);
        let writer_path = log_path.clone();
        let chain_path = log_path
            .parent()
            .map(|p| p.join("audit_chain.ndjson"))
            .unwrap_or_else(|| PathBuf::from("audit_chain.ndjson"));
        let max_archives = 10usize;
        std::thread::Builder::new()
            .name("goon-audit-writer".to_string())
            .spawn(move || {
                // Tamper-evidence layer: chain every entry that was persisted.
                // If the chain cannot be initialized (e.g. unwritable sibling
                // path), the audit log itself keeps working and the failure is
                // reported once.
                let mut chain = open_chain(&chain_path);
                while let Ok(msg) = rx.recv() {
                    match msg {
                        AuditWriterMsg::Entry(entry) => {
                            let persisted = match append_ndjson_entry(&writer_path, &entry) {
                                Ok(true) => {
                                    // The archive cleanup (directory scan) only
                                    // runs when the file was rotated.
                                    if let Some(parent) = writer_path.parent() {
                                        cleanup_old_archives(parent, "audit.ndjson", max_archives);
                                    }
                                    true
                                }
                                Ok(false) => true,
                                Err(e) => {
                                    tracing::warn!(
                                        "audit: failed to persist entry to {}: {}",
                                        writer_path.display(),
                                        e
                                    );
                                    false
                                }
                            };
                            // Chain only entries that actually reached the log
                            // file, keeping the two artifacts in lockstep.
                            if persisted {
                                if let Some(auditor) = chain.as_mut() {
                                    match serde_json::to_value(&entry) {
                                        Ok(payload) => {
                                            if let Err(e) = auditor.append(payload) {
                                                tracing::warn!("audit: chain append failed: {}", e);
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "audit: chain serialization failed: {}",
                                                e
                                            )
                                        }
                                    }
                                }
                                if chain.is_some() {
                                    match rotate_chain_if_needed(
                                        &mut chain,
                                        &chain_path,
                                        CHAIN_ROTATION_BYTES,
                                    ) {
                                        Ok(true) => {
                                            // Chain archives use a different
                                            // stem — clean them up too,
                                            // otherwise they accumulate
                                            // forever (disk growth stays
                                            // bounded only for the NDJSON
                                            // archives).
                                            if let Some(parent) = chain_path.parent() {
                                                cleanup_old_archives(
                                                    parent,
                                                    "audit_chain.ndjson",
                                                    max_archives,
                                                );
                                            }
                                        }
                                        Ok(false) => {}
                                        Err(e) => {
                                            tracing::warn!("audit: chain rotation failed: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        AuditWriterMsg::Flush(ack) => {
                            let _ = ack.send(());
                        }
                    }
                }
            })
            .expect("failed to spawn audit writer thread");
        Self {
            inner: Arc::new(Mutex::new(AuditLogInner {
                entries: VecDeque::new(),
                max_entries,
                dropped_count: 0,
                writer: Some(tx),
            })),
        }
    }

    /// Block until all previously recorded entries have been appended to the
    /// NDJSON file. Used by tests and shutdown paths that need a durable
    /// guarantee after a `record` call.
    pub fn flush(&self) {
        let tx = { self.audit_lock_guard().writer.clone() };
        if let Some(tx) = tx {
            let (ack_tx, ack_rx) = std::sync::mpsc::channel();
            if tx.send(AuditWriterMsg::Flush(ack_tx)).is_ok() {
                let _ = ack_rx.recv();
            }
        }
    }

    /// Record an entry. If the buffer is full, the oldest entry is dropped and a
    /// warning is emitted. If a `log_path` is configured, the entry is appended
    /// as a JSON line to the file (I/O errors are logged but do not crash).
    pub fn record(&self, entry: AuditLogEntry) {
        let mut inner = self.audit_lock_guard();

        // Redact sensitive fields
        let mut entry = entry;
        entry.inputs = redact_sensitive(&entry.inputs);
        entry.outputs = entry.outputs.map(|o| redact_sensitive(&o));

        // Buffer overflow check — drop oldest entry and emit warning
        if inner.entries.len() >= inner.max_entries {
            inner.dropped_count += 1;
            tracing::warn!(
                "audit buffer full, dropping entry at timestamp: {}",
                entry.timestamp
            );
            inner.entries.pop_front();
        }

        inner.entries.push_back(entry.clone());

        // NDJSON persistence is offloaded to a dedicated writer thread; the
        // channel send is non-blocking (`try_send` on a bounded channel), so
        // the request hot path never performs open/append/close disk I/O and
        // never blocks on a backed-up writer.
        if let Some(ref tx) = inner.writer {
            // Bounded channel: on overflow (writer thread backed up under a
            // burst), drop the entry and count it instead of letting the
            // queue grow unboundedly.
            if tx.try_send(AuditWriterMsg::Entry(entry)).is_err() {
                inner.dropped_count += 1;
                tracing::warn!("audit: writer queue full, dropping entry");
            }
        }
    }

    /// Return a snapshot of all entries currently in the buffer.
    pub fn entries(&self) -> Vec<AuditLogEntry> {
        let inner = self.audit_lock_guard();
        inner.entries.iter().cloned().collect()
    }

    /// Return the number of entries that have been dropped due to buffer overflow.
    pub fn dropped_count(&self) -> u64 {
        let inner = self.audit_lock_guard();
        inner.dropped_count
    }

    /// Clear all entries and reset the dropped count.
    pub fn clear(&self) {
        let mut inner = self.audit_lock_guard();
        inner.entries.clear();
        inner.dropped_count = 0;
    }

    /// Acquire the inner lock, recovering from poisoning via `into_inner()` + `warn!`.
    fn audit_lock_guard(&self) -> std::sync::MutexGuard<'_, AuditLogInner> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("audit lock poisoned, recovering");
            poisoned.into_inner()
        })
    }

    /// Return the number of entries currently in the buffer.
    pub fn len(&self) -> usize {
        let inner = self.audit_lock_guard();
        inner.entries.len()
    }

    /// Return `true` if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the timestamp of the most recently recorded entry, if any.
    pub fn last_write_time(&self) -> Option<String> {
        let inner = self.audit_lock_guard();
        inner.entries.back().map(|e| e.timestamp.clone())
    }

    /// Create a new thread-safe audit log with NDJSON persistence at `~/.goon/audit.ndjson`.
    ///
    /// The home directory is expanded at runtime. If the home directory cannot
    /// be determined, the path defaults to `.goon/audit.ndjson` relative to the
    /// current working directory.
    pub fn new_with_default_path(max_entries: usize) -> Self {
        let path = dirs_or_fallback();
        Self::new_with_path(max_entries, path)
    }
}

/// Return the process-wide canonical audit sink.
///
/// All audit writers (HarnessBus, intelligence hub, SecurityGovernor, MCP
/// tool audit) record through this single `ThreadSafeAuditLog`, which owns the
/// one NDJSON persistence layer (`~/.goon/audit.ndjson`) and the one buffer.
/// Subsystems receive a cheap `Clone` handle (shared `Arc`).
pub fn global_audit_log() -> &'static ThreadSafeAuditLog {
    static GLOBAL_AUDIT_LOG: std::sync::LazyLock<ThreadSafeAuditLog> =
        std::sync::LazyLock::new(|| ThreadSafeAuditLog::new_with_default_path(10_000));
    &GLOBAL_AUDIT_LOG
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Append a single entry as a JSON line (NDJSON) to the given file.
///
/// Returns `Ok(true)` when the file was rotated (archive cleanup should then
/// run), `Ok(false)` otherwise. If the file exceeds 100 MB, it is automatically
/// compressed into a gzip archive (`<filename>.1.gz`) and a fresh file starts.
fn append_ndjson_entry(path: &Path, entry: &AuditLogEntry) -> Result<bool, AuditError> {
    // File rotation: compress+gzip archive when >100 MB
    let mut rotated = false;
    if path.exists() && fs::metadata(path)?.len() > 100 * 1024 * 1024 {
        rotate_file(path)?;
        rotated = true;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(entry)?;
    writeln!(file, "{line}")?;
    Ok(rotated)
}

/// Rotate a file by compressing it to a gzip archive and starting fresh.
fn rotate_file(path: &Path) -> Result<(), AuditError> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    // Find the next free archive slot. Overwriting a fixed `<stem>.1.gz` on
    // every rotation silently dropped the previous archive — audit history
    // had gaps — while `cleanup_old_archives` expects numbered `.N.gz` files.
    let dir = path.parent().unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut n = 1usize;
    let archive_path = loop {
        let candidate = dir.join(format!("{file_name}.{n}.gz"));
        if !candidate.exists() {
            break candidate;
        }
        n += 1;
    };

    // Stream the source into the gzip encoder (the previous implementation
    // buffered the whole multi-hundred-MB file in memory before compressing).
    let mut original = fs::File::open(path)?;
    let out = fs::File::create(&archive_path)?;
    let mut encoder = GzEncoder::new(out, Compression::default());
    std::io::copy(&mut original, &mut encoder)?;
    let out = encoder.finish()?;
    let compressed_len = out.metadata().map(|m| m.len()).unwrap_or(0);

    tracing::info!(
        "Audit log rotated: {} -> {} ({} bytes compressed)",
        path.display(),
        archive_path.display(),
        compressed_len,
    );

    // Truncate the original file
    fs::write(path, b"")?;
    Ok(())
}

/// Remove old archive files, keeping only the `max_to_keep` most recent.
///
/// Archive files follow the pattern `<stem>.N.gz` (e.g. `audit.ndjson.1.gz`).
/// Files are sorted by their modification time and the oldest beyond the limit
/// are deleted. This prevents unbounded disk growth in the audit directory.
fn cleanup_old_archives(dir: &Path, stem: &str, max_to_keep: usize) {
    if max_to_keep == 0 {
        return;
    }
    let Ok(mut entries) = fs::read_dir(dir).map(|e| {
        e.filter_map(|e| e.ok())
            .filter(|e| {
                // Strict `<stem>.<digits>.gz` match: a bare prefix match would
                // also delete unrelated files like `audit.ndjson.backup.gz`.
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| {
                        n.strip_prefix(stem)
                            .and_then(|rest| rest.strip_suffix(".gz"))
                            .map(|num| {
                                !num.is_empty()
                                    && num.starts_with('.')
                                    && num[1..].chars().all(|c| c.is_ascii_digit())
                            })
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>()
    }) else {
        return;
    };

    // Sort by modification time (oldest first) so we can delete the oldest.
    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());

    if entries.len() <= max_to_keep {
        return;
    }

    for entry in entries
        .iter()
        .take(entries.len().saturating_sub(max_to_keep))
    {
        if let Err(e) = fs::remove_file(entry.path()) {
            tracing::warn!(
                "audit: failed to remove old archive {}: {}",
                entry.path().display(),
                e
            );
        } else {
            tracing::debug!("audit: removed old archive {}", entry.path().display());
        }
    }
}

/// Resolve `~/.goon/audit.ndjson` or fall back to `.goon/audit.ndjson`.
fn dirs_or_fallback() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let base = if home.is_empty() {
        PathBuf::from(".goon")
    } else {
        PathBuf::from(home).join(".goon")
    };
    base.join("audit.ndjson")
}

/// Chain-file rotation threshold (100 MB, matching the NDJSON log). Each gzip
/// archive retains its own intact hash chain; the fresh file starts a new
/// chain period from the genesis hash, so tamper-evidence never weakens while
/// disk growth stays bounded.
const CHAIN_ROTATION_BYTES: u64 = 100 * 1024 * 1024;

/// Open the hash chain for a writer thread, optionally signing every entry
/// with the Ed25519 seed in `GOON_AUDIT_SIGNING_KEY` (hex-encoded 32-byte
/// seed or 64-byte keypair) for non-repudiation.
fn open_chain(chain_path: &Path) -> Option<HashChainAuditor> {
    let opened = match audit_signing_key_from_env() {
        Some((key_id, key)) => {
            HashChainAuditor::new_signed(chain_path.to_path_buf(), &key_id, &key)
        }
        None => HashChainAuditor::new(chain_path.to_path_buf()),
    };
    match opened {
        Ok(auditor) => Some(auditor),
        Err(e) => {
            tracing::warn!(
                "audit: hash chain unavailable at {}: {}",
                chain_path.display(),
                e
            );
            None
        }
    }
}

/// Parse the optional `GOON_AUDIT_SIGNING_KEY` environment variable: a
/// hex-encoded Ed25519 seed (32 bytes) or keypair (64 bytes). When set, every
/// chain entry is signed; the matching public key can be supplied to
/// `governance.audit.verify` for signature verification.
fn audit_signing_key_from_env() -> Option<(String, Vec<u8>)> {
    let raw = std::env::var("GOON_AUDIT_SIGNING_KEY").ok()?;
    let bytes = hex::decode(raw.trim()).ok()?;
    if bytes.len() != 32 && bytes.len() != 64 {
        tracing::warn!(
            "audit: GOON_AUDIT_SIGNING_KEY must be 64 (seed) or 128 (keypair) hex chars, got {}",
            raw.trim().len()
        );
        return None;
    }
    Some(("goon-audit".to_string(), bytes))
}

/// Rotate the hash chain file once it exceeds `threshold` bytes.
///
/// The archive (`<chain>.1.gz`) keeps the completed chain intact, and the
/// in-memory auditor is replaced with a fresh genesis chain for the new file.
/// The signing key (if `GOON_AUDIT_SIGNING_KEY` is set) is re-read via
/// [`open_chain`] so signature verification stays intact across rotation.
/// Returns `Ok(true)` when a rotation happened.
fn rotate_chain_if_needed(
    chain: &mut Option<HashChainAuditor>,
    chain_path: &Path,
    threshold: u64,
) -> Result<bool, AuditError> {
    if chain.is_none() || !chain_path.exists() {
        return Ok(false);
    }
    if fs::metadata(chain_path)?.len() <= threshold {
        return Ok(false);
    }
    rotate_file(chain_path)?;
    *chain = open_chain(chain_path);
    Ok(true)
}

/// Resolve `~/.goon/audit_chain.ndjson` (or `.goon/audit_chain.ndjson`), the
/// sibling of the canonical audit sink. The sink's writer thread chains every
/// persisted entry to this file; `governance.audit.verify` reads it back.
pub(crate) fn audit_chain_path() -> PathBuf {
    dirs_or_fallback()
        .parent()
        .map(|p| p.join("audit_chain.ndjson"))
        .unwrap_or_else(|| PathBuf::from("audit_chain.ndjson"))
}

/// Get the current UTC time as an ISO-8601 string.
pub(crate) fn chrono_now() -> String {
    // ISO-8601 UTC timestamp. The Y/M/D part delegates to the canonical
    // epoch→date conversion (`security_advisor::unix_ts_to_ymd`, shared with
    // memory_persistence) instead of this module's own day-loop
    // implementation — both are UTC civil-date conversions, and keeping one
    // canonical converter prevents the two from drifting. Time-of-day is
    // formatted here from the seconds remainder. Output format
    // (`YYYY-MM-DDTHH:MM:SSZ`) is unchanged.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let (y, m, d) = crate::security::security_advisor::unix_ts_to_ymd(secs);
    let time_secs = secs.rem_euclid(86_400);
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hours, minutes, seconds
    )
}

/// Redact sensitive fields from a JSON value.
pub fn redact_sensitive(value: &serde_json::Value) -> serde_json::Value {
    match value {
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (k, v) in map {
                let lower = k.to_lowercase();
                if lower.contains("api_key")
                    || lower.contains("secret")
                    || lower.contains("password")
                    || lower.contains("token")
                {
                    redacted.insert(k.clone(), Value::String("**REDACTED**".to_string()));
                } else {
                    redacted.insert(k.clone(), redact_sensitive(v));
                }
            }
            Value::Object(redacted)
        }
        Value::String(s) => {
            // Redact common API key patterns in string values
            if s.len() > 20
                && (s.starts_with("sk-") || s.starts_with("pk-") || s.starts_with("AKIA"))
            {
                // Prefix is ASCII by construction (sk-/pk-/AKIA); the tail is
                // sliced char-safely so a multi-byte ending cannot panic.
                let tail: String = s
                    .chars()
                    .rev()
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                Value::String(format!("{}...{}", &s[..2], tail))
            } else {
                Value::String(s.clone())
            }
        }
        other => other.clone(),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;

    fn sample_entry(timestamp: &str) -> AuditLogEntry {
        AuditLogEntry {
            timestamp: timestamp.to_string(),
            task_id: "task-001".to_string(),
            phase: "test".to_string(),
            agent: None,
            tool: None,
            decision: "proceed".to_string(),
            inputs: serde_json::json!({"prompt": "hello"}),
            outputs: None,
            error: None,
            confidence: None,
            data_classification: None,
            compliance_tags: vec![],
            retention_policy: None,
            correlation_id: None,
        }
    }

    // ── Thread safety test ──────────────────────────────────────────────────

    #[test]
    fn test_thread_safety() {
        let log = ThreadSafeAuditLog::new(1000);
        let mut handles = vec![];

        for i in 0..10 {
            let log_clone = log.clone_arc();
            handles.push(thread::spawn(move || {
                for j in 0..100 {
                    let ts = format!("2026-05-26T{:02}:{:02}:00Z", i, j);
                    log_clone.record(sample_entry(&ts));
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        let entries = log.entries();
        assert_eq!(
            entries.len(),
            1000,
            "expected 1000 entries from 10×100 threads"
        );
    }

    // ── Buffer overflow warning test ────────────────────────────────────────

    #[test]
    fn test_buffer_overflow_warning() {
        let log = ThreadSafeAuditLog::new(3);

        log.record(sample_entry("t1"));
        log.record(sample_entry("t2"));
        log.record(sample_entry("t3"));
        // The next two records should trigger buffer-full warnings
        log.record(sample_entry("t4"));
        log.record(sample_entry("t5"));

        let entries = log.entries();
        assert_eq!(entries.len(), 3, "buffer should hold at most 3 entries");
        // The three newest entries should survive
        assert_eq!(entries[0].timestamp, "t3");
        assert_eq!(entries[1].timestamp, "t4");
        assert_eq!(entries[2].timestamp, "t5");
        assert_eq!(log.dropped_count(), 2, "expected 2 dropped entries");
    }

    // ── File persistence test ───────────────────────────────────────────────

    #[test]
    fn test_file_persistence() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let log_path = dir.path().join("audit.ndjson");

        let log = ThreadSafeAuditLog::new_with_path(10, log_path.clone());

        log.record(sample_entry("2026-05-26T10:00:00Z"));
        log.record(sample_entry("2026-05-26T10:00:01Z"));
        log.record(sample_entry("2026-05-26T10:00:02Z"));

        // Persistence is asynchronous via the background writer thread; flush
        // before reading the file back.
        log.flush();

        // Read back the file and verify
        let content = fs::read_to_string(&log_path).expect("failed to read log file");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "expected 3 NDJSON lines");

        let parsed: AuditLogEntry = serde_json::from_str(lines[0]).expect("invalid JSON");
        assert_eq!(parsed.timestamp, "2026-05-26T10:00:00Z");
        assert_eq!(parsed.decision, "proceed");
    }

    // ── Redaction sanity check ──────────────────────────────────────────────

    #[test]
    fn test_redaction() {
        let log = ThreadSafeAuditLog::new(10);
        let mut entry = sample_entry("redact-test");
        entry.inputs = serde_json::json!({
            "api_key": "sk-12345678901234567890",
            "prompt": "hello world",
        });

        log.record(entry);
        let entries = log.entries();
        assert_eq!(entries.len(), 1);
        let recorded = &entries[0];
        assert_eq!(
            recorded.inputs["api_key"],
            serde_json::json!("**REDACTED**")
        );
        assert_eq!(recorded.inputs["prompt"], serde_json::json!("hello world"));
    }

    // ── Hash chain integrity: every persisted entry is chained ──────────────

    #[test]
    fn test_every_record_is_chained() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let log_path = dir.path().join("audit.ndjson");
        let chain_path = dir.path().join("audit_chain.ndjson");

        let log = ThreadSafeAuditLog::new_with_path(10, log_path.clone());
        log.record(sample_entry("2026-05-26T10:00:00Z"));
        log.record(sample_entry("2026-05-26T10:00:01Z"));
        log.record(sample_entry("2026-05-26T10:00:02Z"));
        log.flush();

        // The sink's writer thread must have produced one chain entry per
        // persisted log line, and the chain must verify cleanly.
        let auditor = HashChainAuditor::new(chain_path.clone())
            .expect("chain file must exist next to the audit log");
        assert_eq!(
            auditor.entry_count().expect("entry count"),
            3,
            "every persisted entry must be chained"
        );
        assert!(
            auditor.verify_integrity(None).expect("verify").is_empty(),
            "chain must be intact"
        );
    }

    #[test]
    fn test_chain_rotation_starts_fresh_period() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let log_path = dir.path().join("audit.ndjson");
        let chain_path = dir.path().join("audit_chain.ndjson");

        let log = ThreadSafeAuditLog::new_with_path(10, log_path.clone());
        log.record(sample_entry("2026-05-26T10:00:00Z"));
        log.record(sample_entry("2026-05-26T10:00:01Z"));
        log.flush();

        let mut chain = HashChainAuditor::new(chain_path.clone())
            .expect("chain file must exist")
            .into();
        let rotated =
            rotate_chain_if_needed(&mut chain, &chain_path, 1).expect("rotation must not fail");
        assert!(rotated, "threshold 1 byte must force rotation");
        assert!(
            dir.path().join("audit_chain.ndjson.1.gz").exists(),
            "old chain must be archived"
        );

        // The new file restarts from genesis: appending succeeds and the
        // previous entries are no longer part of the live chain.
        let fresh = HashChainAuditor::new(chain_path.clone()).expect("fresh chain");
        assert_eq!(fresh.entry_count().expect("entry count"), 0);
        let mut fresh = fresh;
        fresh
            .append(serde_json::json!({"event": "post_rotation"}))
            .expect("append after rotation must succeed");
        assert_eq!(fresh.entry_count().expect("entry count"), 1);
        assert!(
            fresh.verify_integrity(None).expect("verify").is_empty(),
            "fresh period chain must be intact"
        );
    }

    #[test]
    fn test_cleanup_old_archives_matches_only_numbered_archives() {
        // Regression: a bare prefix match (`starts_with(stem)`) would also
        // delete unrelated files like `audit.ndjson.backup.gz`.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let keep = dir.path().join("audit.ndjson.1.gz");
        let keep2 = dir.path().join("audit.ndjson.12.gz");
        let unrelated = dir.path().join("audit.ndjson.backup.gz");
        let also_unrelated = dir.path().join("audit_chain.ndjson.1.gz");
        for f in [&keep, &keep2, &unrelated, &also_unrelated] {
            std::fs::write(f, b"x").unwrap();
        }

        cleanup_old_archives(dir.path(), "audit.ndjson", 100);

        // Numbered archives survive; unrelated files must NOT be deleted.
        assert!(keep.exists(), "numbered archive must be kept");
        assert!(keep2.exists(), "numbered archive must be kept");
        assert!(
            unrelated.exists(),
            "non-numbered file must not be deleted by prefix match"
        );
        assert!(
            also_unrelated.exists(),
            "other stem's archives must not be deleted"
        );

        // With a keep-limit of 1, the oldest by mtime is removed (here the
        // first-created `keep`), while the unrelated files still survive.
        let older = dir.path().join("audit.ndjson.0.gz");
        std::fs::write(&older, b"x").unwrap();
        cleanup_old_archives(dir.path(), "audit.ndjson", 1);
        assert!(!keep.exists(), "oldest-by-mtime numbered archive removed");
        assert!(keep2.exists() || older.exists(), "newer archives kept");
        assert!(unrelated.exists(), "unrelated still untouched");
        assert!(also_unrelated.exists(), "other stem still untouched");
    }
}
