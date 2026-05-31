//! Audit — F-GAP-03
//!
//! Audit logging system for go-on (Phase 2 — thread-safe + persistence)
//!
//! Provides:
//! - [`AuditLog`] — original single-threaded circular buffer (backward compatible)
//! - [`ThreadSafeAuditLog`] — thread-safe version with optional NDJSON file persistence
//!
//! Both structures record agent/tool/phase decisions for compliance and debugging.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ─── AuditLogEntry (unchanged) ──────────────────────────────────────────────

/// Audit log entry for all agent/tool/phase decisions
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
}

// ─── AuditLog (single-threaded, backward compatible) ────────────────────────

/// Audit log sink for collecting decision traces (single-threaded).
///
/// This is the original implementation kept for backward compatibility.
/// For thread-safe usage with file persistence, use [`ThreadSafeAuditLog`].
pub struct AuditLog {
    entries: VecDeque<AuditLogEntry>,
    max_entries: usize,
}

impl AuditLog {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_entries,
        }
    }

    pub fn record(&mut self, entry: AuditLogEntry) {
        let mut entry = entry;
        entry.inputs = redact_sensitive(&entry.inputs);
        entry.outputs = entry.outputs.map(|o| redact_sensitive(&o));
        if self.entries.len() >= self.max_entries {
            tracing::warn!(
                "audit buffer full, dropping entry at timestamp: {}",
                entry.timestamp
            );
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub fn entries(&self) -> Vec<AuditLogEntry> {
        self.entries.iter().cloned().collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// ─── ThreadSafeAuditLog ─────────────────────────────────────────────────────

/// Thread-safe audit log with optional NDJSON file persistence.
///
/// Wraps internal state in `Arc<Mutex<...>>` so it can be shared across threads.
/// When a `log_path` is configured, every recorded entry is appended as a JSON
/// line to the file. Buffer overflow warnings are emitted via `tracing::warn!`.
#[allow(dead_code)] // F-GAP-49 — reserved for Phase 2 audit pipeline integration
pub struct ThreadSafeAuditLog {
    inner: Arc<Mutex<AuditLogInner>>,
}

#[allow(dead_code)] // F-GAP-49 — reserved for Phase 2 audit pipeline integration
struct AuditLogInner {
    entries: VecDeque<AuditLogEntry>,
    max_entries: usize,
    dropped_count: u64,
    log_path: Option<PathBuf>,
}

#[allow(dead_code)] // F-GAP-49 — reserved for Phase 2 audit pipeline integration
impl ThreadSafeAuditLog {
    /// Create a new thread-safe audit log with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AuditLogInner {
                entries: VecDeque::new(),
                max_entries,
                dropped_count: 0,
                log_path: None,
            })),
        }
    }

    /// Create a new thread-safe audit log with capacity and NDJSON file path.
    pub fn new_with_path(max_entries: usize, log_path: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AuditLogInner {
                entries: VecDeque::new(),
                max_entries,
                dropped_count: 0,
                log_path: Some(log_path.into()),
            })),
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

        // NDJSON persistence — append entry as a JSON line to the log file
        if let Some(ref log_path) = inner.log_path {
            if let Err(e) = append_ndjson_entry(log_path, &entry) {
                tracing::warn!(
                    "audit: failed to persist entry to {}: {}",
                    log_path.display(),
                    e
                );
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

    /// Share the same underlying audit log by cloning the `Arc`.
    ///
    /// All clones share the same inner buffer and file path.
    #[allow(dead_code)] // F-GAP-49 — reserved for Phase 2 audit pipeline integration
    pub fn clone_arc(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Append a single entry as a JSON line (NDJSON) to the given file.
///
/// If the file exceeds 100 MB, it is automatically compressed into a gzip
/// archive (`<filename>.1.gz`) and a fresh file is started.
fn append_ndjson_entry(
    path: &Path,
    entry: &AuditLogEntry,
) -> Result<(), Box<dyn std::error::Error>> {
    // File rotation: compress+gzip archive when >100 MB
    if path.exists() && fs::metadata(path)?.len() > 100 * 1024 * 1024 {
        rotate_file(path)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(entry)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Rotate a file by compressing it to a gzip archive and starting fresh.
fn rotate_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;

    let archive_path = path.with_extension("ndjson.1.gz");
    let mut original = fs::File::open(path)?;
    let mut data = Vec::new();
    original.read_to_end(&mut data)?;

    let compressed = {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&data)?;
        encoder.finish()?
    };

    let compressed_len = compressed.len();
    fs::write(&archive_path, &compressed)?;
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
                Value::String(format!("{}...{}", &s[..2], &s[s.len() - 2..]))
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
}
