//! Governance audit event types and persistence.

use super::*;
use crate::governance::audit::AuditLogEntry;

// ---------------------------------------------------------------------------
// Governance audit event types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovernanceAuditEvent {
    pub(super) timestamp: u64,
    pub(super) action: String,
    pub(super) actor: String,
    pub(super) result: String,
    pub(super) detail: Value,
}

/// Record a governance audit event through the process-wide canonical audit
/// sink (`governance::audit::global_audit_log`). This replaced the separate
/// `.goon/governance/audit.ndjson` file, which was un-chained, non-rotating,
/// and excluded from the main audit stream / `governance.audit.verify`.
pub(crate) fn append_governance_audit_event(event: &GovernanceAuditEvent) -> Result<()> {
    crate::governance::audit::global_audit_log().record(AuditLogEntry {
        timestamp: crate::governance::audit::chrono_now(),
        task_id: event.action.clone(),
        phase: "governance".to_string(),
        agent: Some(event.actor.clone()),
        tool: None,
        decision: event.result.clone(),
        inputs: event.detail.clone(),
        outputs: None,
        error: None,
        confidence: None,
        data_classification: None,
        compliance_tags: vec![],
        retention_policy: None,
        correlation_id: None,
    });
    Ok(())
}

/// Load the most recent governance audit events from the canonical sink's
/// in-memory buffer (no per-request file I/O; the sink owns persistence).
pub(crate) fn load_governance_audit_events(limit: usize) -> Result<Vec<GovernanceAuditEvent>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut events: Vec<GovernanceAuditEvent> = crate::governance::audit::global_audit_log()
        .entries()
        .into_iter()
        .filter(|entry| entry.phase == "governance")
        .map(|entry| GovernanceAuditEvent {
            timestamp: iso_to_ms(&entry.timestamp)
                .unwrap_or_else(|| crate::shared::timestamps::now_ts_ms().max(0) as u64),
            action: entry.task_id.clone(),
            actor: entry.agent.clone().unwrap_or_default(),
            result: entry.decision.clone(),
            detail: entry.inputs.clone(),
        })
        .collect();

    let len = events.len();
    if len > limit {
        events.drain(0..len - limit);
    }
    Ok(events)
}

/// Parse the canonical sink's ISO-8601 timestamp (`YYYY-MM-DDTHH:MM:SSZ`,
/// seconds precision) into milliseconds since epoch.
fn iso_to_ms(iso: &str) -> Option<u64> {
    let digits: Vec<&str> = iso
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .collect();
    if digits.len() < 6 {
        return None;
    }
    let year: u64 = digits[0].parse().ok()?;
    let month: u64 = digits[1].parse().ok()?;
    let day: u64 = digits[2].parse().ok()?;
    let hour: u64 = digits[3].parse().ok()?;
    let minute: u64 = digits[4].parse().ok()?;
    let second: u64 = digits[5].parse().ok()?;
    if !(1970..=2100).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    fn days_in_year(y: u64) -> u64 {
        if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) {
            366
        } else {
            365
        }
    }
    fn days_in_month(y: u64, m: u64) -> u64 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        }
    }
    let mut total_days = 0u64;
    for y in 1970..year {
        total_days += days_in_year(y);
    }
    for m in 1..month {
        total_days += days_in_month(year, m);
    }
    total_days += day.saturating_sub(1);
    Some((total_days * 86400 + hour * 3600 + minute * 60 + second) * 1000)
}

// ---------------------------------------------------------------------------
// governance.audit.verify — verify the canonical sink's hash chain
// ---------------------------------------------------------------------------

/// Verify the tamper-evident hash chain produced by the canonical audit sink
/// (see `governance/audit.rs`). Returns the chain summary plus any integrity
/// violations; when `from_ms`/`to_ms` are provided, also returns the exported
/// audit report for that time window.
///
/// Optional `public_key_hex` (hex-encoded Ed25519 public key, 32 bytes)
/// enables signature verification of signed entries (chains written with
/// `GOON_AUDIT_SIGNING_KEY`).
pub(crate) fn governance_audit_verify_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let chain_path = crate::governance::audit::audit_chain_path();
    audit_chain_verify_at(&chain_path, &params)
}

/// Testable core of [`governance_audit_verify_payload`]: verifies the chain at
/// an explicit path, decoupled from the default `~/.goon` location.
fn audit_chain_verify_at(chain_path: &std::path::Path, params: &Value) -> Result<Value> {
    let auditor =
        crate::security::audit_integrity::HashChainAuditor::new(chain_path.to_path_buf())?;

    let public_key = params
        .get("public_key_hex")
        .and_then(Value::as_str)
        .and_then(|hex_str| hex::decode(hex_str).ok())
        .filter(|bytes| bytes.len() == 32);

    let violations = auditor.verify_integrity(public_key.as_deref())?;

    // Optional time-window export (both bounds required).
    let report = match (
        params.get("from_ms").and_then(Value::as_u64),
        params.get("to_ms").and_then(Value::as_u64),
    ) {
        (Some(from), Some(to)) => {
            let report = auditor.export_audit_report(from, to, public_key.as_deref())?;
            Some(json!(report))
        }
        _ => None,
    };

    let violation_summary: Vec<Value> = violations
        .iter()
        .map(|v| {
            json!({
                "entry_id": v.entry_id.clone(),
                "expected_prev_hash": v.expected_prev_hash.clone(),
                "actual_prev_hash": v.actual_prev_hash.clone(),
                "reason": v.reason.clone(),
            })
        })
        .collect();

    Ok(json!({
        "ok": true,
        "chain_file": chain_path.display().to_string(),
        "entry_count": auditor.entry_count()?,
        "current_hash": auditor.current_hash(),
        "last_entry_id": auditor.last_entry_id(),
        "is_chain_intact": violations.is_empty(),
        "violations": violation_summary,
        "report": report,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::audit_integrity::HashChainAuditor;

    fn write_chain(chain_path: &std::path::Path, count: usize) {
        let mut auditor = HashChainAuditor::new(chain_path.to_path_buf()).unwrap();
        for i in 0..count {
            auditor
                .append(json!({ "event": format!("event-{}", i) }))
                .unwrap();
        }
    }

    #[test]
    fn verify_reports_intact_chain() {
        let dir = tempfile::tempdir().unwrap();
        let chain_path = dir.path().join("audit_chain.ndjson");
        write_chain(&chain_path, 3);

        let result = audit_chain_verify_at(&chain_path, &json!({})).unwrap();
        assert_eq!(result["entry_count"], json!(3));
        assert_eq!(result["is_chain_intact"], json!(true));
        assert_eq!(result["violations"], json!([]));
        assert!(result["current_hash"].as_str().unwrap().len() == 64);
        assert!(result["last_entry_id"].is_string());
        assert!(result["report"].is_null());
    }

    #[test]
    fn verify_detects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let chain_path = dir.path().join("audit_chain.ndjson");
        write_chain(&chain_path, 3);

        // Tamper with the second line's payload while leaving the stored
        // payload_hash unchanged — verify_integrity must flag it.
        let content = std::fs::read_to_string(&chain_path).unwrap();
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let mut entry: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
        entry["payload"] = json!({"event": "tampered"});
        lines[1] = serde_json::to_string(&entry).unwrap();
        std::fs::write(&chain_path, lines.join("\n") + "\n").unwrap();

        let result = audit_chain_verify_at(&chain_path, &json!({})).unwrap();
        assert_eq!(result["is_chain_intact"], json!(false));
        let violations = result["violations"].as_array().unwrap();
        assert!(!violations.is_empty());
        assert!(violations[0]["reason"]
            .as_str()
            .unwrap()
            .contains("payload_hash mismatch"));
    }

    #[test]
    fn verify_exports_time_window_report() {
        let dir = tempfile::tempdir().unwrap();
        let chain_path = dir.path().join("audit_chain.ndjson");
        write_chain(&chain_path, 3);

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let result = audit_chain_verify_at(
            &chain_path,
            &json!({"from_ms": now_ms - 60_000, "to_ms": now_ms + 60_000}),
        )
        .unwrap();
        let report = result["report"].as_object().unwrap();
        assert_eq!(report["entry_count"], json!(3));
        assert_eq!(report["is_chain_intact"], json!(true));
    }
}
