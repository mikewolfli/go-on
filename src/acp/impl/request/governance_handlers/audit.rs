//! Governance audit event types and persistence.

use super::*;

// ---------------------------------------------------------------------------
// Governance audit event types
// ---------------------------------------------------------------------------

const GOVERNANCE_AUDIT_DIR: &str = ".goon/governance";
const GOVERNANCE_AUDIT_FILE: &str = "audit.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GovernanceAuditEvent {
    pub(super) timestamp: u64,
    pub(super) action: String,
    pub(super) actor: String,
    pub(super) result: String,
    pub(super) detail: Value,
}

pub(crate) fn append_governance_audit_event(event: &GovernanceAuditEvent) -> Result<()> {
    let dir = std::path::Path::new(GOVERNANCE_AUDIT_DIR);
    fs::create_dir_all(dir)?;
    let path = dir.join(GOVERNANCE_AUDIT_FILE);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(event)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

pub(crate) fn load_governance_audit_events(limit: usize) -> Result<Vec<GovernanceAuditEvent>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let path = std::path::Path::new(GOVERNANCE_AUDIT_DIR).join(GOVERNANCE_AUDIT_FILE);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut events = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: GovernanceAuditEvent = serde_json::from_str(trimmed)?;
        events.push(event);
    }

    if events.len() > limit {
        Ok(events.split_off(events.len() - limit))
    } else {
        Ok(events)
    }
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
