//! File-write chokepoint: uniform change-event audit for every write tool.
//!
//! All file-writing entry points (write_file, edit_file, apply_patch) funnel
//! their mutations through [`record_file_change`], which records one
//! "what changed" entry per write in the canonical audit sink
//! ([`crate::governance::audit::global_audit_log`]): the content SHA-256
//! before and after the mutation. Together with the sink's hash chain this
//! lets the audit chain replay exactly what each write changed without
//! storing file contents.
//!
//! Hashing is deliberately best-effort: [`file_hash`] returns `Ok(None)` for
//! missing files, and callers downgrade hard failures to `None` +
//! `tracing::warn!` so an unreadable or oversized file can never turn a
//! successful write into a failed tool call.

use crate::governance::audit::{chrono_now, global_audit_log, AuditLogEntry};
use anyhow::Result;
use std::path::Path;

/// Cap for content hashing (64 MiB). Write payloads are already bounded by
/// `builtin_tools::MAX_WRITE_PAYLOAD_BYTES` (50 MiB), so every file a write
/// tool creates is below this bound; the cap only guards append-grown or
/// externally modified files so hashing never reads an unbounded file. A file
/// above the cap hashes as `Err` and the caller records `None` hashes
/// (best-effort audit, never a tool failure).
const MAX_HASH_FILE_BYTES: usize = 64 * 1024 * 1024;

/// One file mutation: the path that changed, the operation that changed it,
/// and the content hashes before/after. A `None` hash means the file did not
/// exist on that side (e.g. a freshly created file has `old_hash: None`) or
/// the hash could not be computed (best-effort audit).
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub path: String,
    pub op: &'static str,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
}

/// Record a [`FileChangeEvent`] in the canonical audit sink.
///
/// The entry uses `decision = "file_change"` (so the audit chain can filter
/// for file mutations), `tool` = the operation name, `inputs` = {path, op}
/// and `outputs` = {old_hash, new_hash}. Recording is fire-and-forget: the
/// sink swallows its own I/O errors by design, and the write has already
/// happened when this is called — this is an after-the-fact audit trail, not
/// a gate.
pub fn record_file_change(task_id: &str, phase: &str, event: &FileChangeEvent) {
    global_audit_log().record(AuditLogEntry {
        timestamp: chrono_now(),
        task_id: task_id.to_string(),
        phase: phase.to_string(),
        agent: None,
        tool: Some(event.op.to_string()),
        decision: "file_change".to_string(),
        inputs: serde_json::json!({ "path": event.path, "op": event.op }),
        outputs: Some(serde_json::json!({
            "old_hash": event.old_hash,
            "new_hash": event.new_hash,
        })),
        error: None,
        confidence: None,
        data_classification: None,
        compliance_tags: vec![],
        retention_policy: None,
        correlation_id: None,
    });
}

/// Content hash of `path`, or `None` when the file does not exist.
///
/// Reads are capped at [`MAX_HASH_FILE_BYTES`]; a file above the cap (or one
/// that cannot be read for another reason) returns `Err`. Callers must
/// downgrade that to `None` + `warn!`, never propagate it, so hashing can
/// never fail a write that already succeeded.
pub fn file_hash(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let data =
        crate::orchestration::tool::exec_common::read_file_capped(path, MAX_HASH_FILE_BYTES)?;
    Ok(Some(crate::shared::sha256_hex(&data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_hash_none_for_missing_some_for_existing() {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let missing = dir.path().join("does-not-exist.txt");
        assert!(
            file_hash(&missing)
                .expect("missing file hashes to Ok(None)")
                .is_none(),
            "a missing file must hash to None"
        );

        let path = dir.path().join("content.txt");
        std::fs::write(&path, b"payload bytes").expect("fixture should be written");
        let hash = file_hash(&path)
            .expect("existing file hashes to Ok(Some)")
            .expect("existing file yields Some hash");
        assert_eq!(hash, crate::shared::sha256_hex(b"payload bytes"));
    }

    #[test]
    fn record_file_change_lands_in_global_audit_log() {
        // The global sink is process-wide and shared with parallel tests, so
        // assert on a unique task_id instead of clearing the shared log.
        let task_id = format!("write-audit-test-{}", std::process::id());
        record_file_change(
            &task_id,
            "test",
            &FileChangeEvent {
                path: "/tmp/audit-probe.txt".to_string(),
                op: "write_file",
                old_hash: None,
                new_hash: Some("0123456789abcdef".to_string()),
            },
        );

        let entry = global_audit_log()
            .entries()
            .into_iter()
            .find(|e| e.task_id == task_id && e.decision == "file_change")
            .expect("record_file_change must land in the global audit log");
        assert_eq!(entry.tool.as_deref(), Some("write_file"));
        assert_eq!(entry.inputs["path"], "/tmp/audit-probe.txt");
        assert_eq!(entry.inputs["op"], "write_file");
        assert_eq!(
            entry.outputs.as_ref().expect("outputs must be set")["new_hash"],
            "0123456789abcdef"
        );
        assert_eq!(
            entry.outputs.as_ref().expect("outputs must be set")["old_hash"],
            serde_json::Value::Null
        );
    }
}
