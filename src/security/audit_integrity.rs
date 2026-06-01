#![allow(dead_code)]

//! Audit Integrity — Hash Chain Auditor (GAP-B52-27)
//!
//! Provides tamper-evident audit logging using a hash chain.
//! Each entry includes the previous entry's hash, a payload hash,
//! a timestamp, and an optional signature. The chain can be verified
//! for integrity and exported as a report.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("chain file not found: {0}")]
    ChainFileNotFound(String),

    #[error("integrity violation at entry {entry_id}: {reason}")]
    IntegrityViolation { entry_id: String, reason: String },

    #[error("serialization error: {0}")]
    SerializationError(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("signature verification failed: {0}")]
    SignatureVerificationFailed(String),
}

impl From<std::io::Error> for AuditError {
    fn from(e: std::io::Error) -> Self {
        AuditError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AuditError {
    fn from(e: serde_json::Error) -> Self {
        AuditError::SerializationError(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// IntegrityViolation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityViolation {
    pub entry_id: String,
    pub expected_prev_hash: String,
    pub actual_prev_hash: String,
    pub reason: String,
    pub entry: Option<AuditEntry>,
}

// ---------------------------------------------------------------------------
// AuditEntry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique entry identifier (monotonic, e.g., UUID or sequence number).
    pub entry_id: String,
    /// SHA-256 of the previous entry's full content (hex-encoded).
    /// For the genesis entry, this is "0000...0000".
    pub prev_hash: String,
    /// SHA-256 of the payload (hex-encoded).
    pub payload_hash: String,
    /// The actual payload content (arbitrary JSON or structured data).
    pub payload: serde_json::Value,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Optional signature for non-repudiation (base64-encoded).
    pub signature: Option<String>,
    /// Key ID used for signing, if signed.
    pub key_id: Option<String>,
}

impl AuditEntry {
    /// Create a new audit entry chained to the previous entry's hash.
    pub fn new(
        entry_id: String,
        prev_hash: String,
        payload: serde_json::Value,
        timestamp_ms: u64,
    ) -> Self {
        let payload_bytes = serde_json::to_vec(&payload).unwrap_or_default();
        let payload_hash = hex::encode(sha256(&payload_bytes));

        Self {
            entry_id,
            prev_hash,
            payload_hash,
            payload,
            timestamp_ms,
            signature: None,
            key_id: None,
        }
    }

    /// Compute the canonical hash of this entry for chaining.
    /// Hash = SHA-256(entry_id || prev_hash || payload_hash || timestamp_ms)
    pub fn compute_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.entry_id.as_bytes());
        hasher.update(self.prev_hash.as_bytes());
        hasher.update(self.payload_hash.as_bytes());
        hasher.update(self.timestamp_ms.to_string().as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Sign this entry using the provided key.
    /// This sets the `signature` and `key_id` fields.
    pub fn sign(
        &mut self,
        key_id: &str,
        private_key: &[u8],
        algorithm: crate::security::request_signing::SigningAlgorithm,
    ) -> Result<(), AuditError> {
        let body = self.compute_hash().into_bytes();
        let sig =
            crate::security::request_signing::sign_request(private_key, &body, algorithm, key_id)
                .map_err(|e| AuditError::SignatureVerificationFailed(e.to_string()))?;

        self.signature = Some(sig.signature);
        self.key_id = Some(sig.key_id);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HashChainAuditor
// ---------------------------------------------------------------------------

/// Tamper-evident audit log using a hash chain.
///
/// Each appended entry is cryptographically linked to the previous one,
/// making any retroactive modification detectable.
pub struct HashChainAuditor {
    /// Path to the chain file (newline-delimited JSON).
    chain_file: PathBuf,
    /// Current hash of the most recent entry (for quick chaining).
    current_hash: String,
    /// ID of the last entry written.
    last_entry_id: Option<String>,
}

impl HashChainAuditor {
    /// Create or load an existing hash chain auditor.
    ///
    /// If the chain file exists, it loads the last entry's hash for continued chaining.
    /// If not, it creates a genesis chain initialized with a zero hash.
    pub fn new(chain_file: PathBuf) -> Result<Self, AuditError> {
        let (current_hash, last_entry_id) = if chain_file.exists() {
            let entries = Self::load_all_entries(&chain_file)?;
            if let Some(last) = entries.last() {
                (last.compute_hash(), Some(last.entry_id.clone()))
            } else {
                (Self::genesis_hash(), None)
            }
        } else {
            (Self::genesis_hash(), None)
        };

        Ok(Self {
            chain_file,
            current_hash,
            last_entry_id,
        })
    }

    /// Append a new audit entry to the chain.
    pub fn append(&mut self, payload: serde_json::Value) -> Result<AuditEntry, AuditError> {
        let entry_id = uuid::Uuid::new_v4().to_string();
        let timestamp_ms = current_timestamp_ms();

        let entry = AuditEntry::new(entry_id, self.current_hash.clone(), payload, timestamp_ms);

        // Serialize and write
        let line = serde_json::to_string(&entry)
            .map_err(|e| AuditError::SerializationError(e.to_string()))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.chain_file)?;

        writeln!(file, "{}", line)?;

        // Update chain state
        self.current_hash = entry.compute_hash();
        self.last_entry_id = Some(entry.entry_id.clone());

        debug!(entry = %entry.entry_id, hash = %self.current_hash, "Audit entry appended");
        Ok(entry)
    }

    /// Verify the integrity of the entire hash chain.
    /// Returns a list of violations (empty if the chain is intact).
    pub fn verify_integrity(&self) -> Result<Vec<IntegrityViolation>, AuditError> {
        let entries = Self::load_all_entries(&self.chain_file)?;
        let mut violations = Vec::new();
        let mut expected_prev_hash = Self::genesis_hash();

        for entry in &entries {
            // Check prev_hash matches the expected value
            if entry.prev_hash != expected_prev_hash {
                violations.push(IntegrityViolation {
                    entry_id: entry.entry_id.clone(),
                    expected_prev_hash: expected_prev_hash.clone(),
                    actual_prev_hash: entry.prev_hash.clone(),
                    reason: format!(
                        "prev_hash mismatch: expected {}, got {}",
                        expected_prev_hash, entry.prev_hash
                    ),
                    entry: Some(entry.clone()),
                });
            }

            // Verify payload_hash matches the actual payload
            let payload_bytes = serde_json::to_vec(&entry.payload).unwrap_or_default();
            let computed_payload_hash = hex::encode(sha256(&payload_bytes));
            if computed_payload_hash != entry.payload_hash {
                violations.push(IntegrityViolation {
                    entry_id: entry.entry_id.clone(),
                    expected_prev_hash: expected_prev_hash.clone(),
                    actual_prev_hash: entry.prev_hash.clone(),
                    reason: format!(
                        "payload_hash mismatch: expected {}, got {}",
                        computed_payload_hash, entry.payload_hash
                    ),
                    entry: Some(entry.clone()),
                });
            }

            // Verify the entry's own hash chain (optional signature check would go here)
            let computed_hash = entry.compute_hash();
            // Store this entry's hash as the expected prev_hash for the next entry
            expected_prev_hash = computed_hash;
        }

        Ok(violations)
    }

    /// Export an audit report for entries within a time range.
    pub fn export_audit_report(&self, from_ms: u64, to_ms: u64) -> Result<AuditReport, AuditError> {
        let all_entries = Self::load_all_entries(&self.chain_file)?;

        let filtered: Vec<AuditEntry> = all_entries
            .into_iter()
            .filter(|e| e.timestamp_ms >= from_ms && e.timestamp_ms <= to_ms)
            .collect();

        let violations = self.verify_integrity()?;
        let is_chain_intact = violations.is_empty();

        Ok(AuditReport {
            from_timestamp_ms: from_ms,
            to_timestamp_ms: to_ms,
            entry_count: filtered.len(),
            entries: filtered,
            integrity_violations: violations,
            is_chain_intact,
        })
    }

    /// Get the current hash (hash of the most recent entry).
    pub fn current_hash(&self) -> &str {
        &self.current_hash
    }

    /// Get the last entry ID.
    pub fn last_entry_id(&self) -> Option<&str> {
        self.last_entry_id.as_deref()
    }

    /// Load all entries from the chain file.
    fn load_all_entries(chain_file: &Path) -> Result<Vec<AuditEntry>, AuditError> {
        if !chain_file.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(chain_file)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let entry: AuditEntry = serde_json::from_str(&line)
                .map_err(|e| AuditError::SerializationError(format!("line parse: {}", e)))?;
            entries.push(entry);
        }

        Ok(entries)
    }

    /// The genesis hash: 64 zero hex chars (SHA-256 hash of empty string-ish).
    fn genesis_hash() -> String {
        "0000000000000000000000000000000000000000000000000000000000000000".to_string()
    }

    /// Count the number of entries in the chain.
    pub fn entry_count(&self) -> Result<usize, AuditError> {
        Ok(Self::load_all_entries(&self.chain_file)?.len())
    }
}

// ---------------------------------------------------------------------------
// AuditReport
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    pub from_timestamp_ms: u64,
    pub to_timestamp_ms: u64,
    pub entry_count: usize,
    pub entries: Vec<AuditEntry>,
    pub integrity_violations: Vec<IntegrityViolation>,
    pub is_chain_intact: bool,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_auditor() -> (TempDir, HashChainAuditor) {
        let dir = TempDir::new().unwrap();
        let chain_file = dir.path().join("audit.chain");
        let auditor = HashChainAuditor::new(chain_file).unwrap();
        (dir, auditor)
    }

    #[test]
    fn test_append_and_verify() {
        let (_dir, mut auditor) = setup_auditor();

        let entry = auditor
            .append(serde_json::json!({"action": "deploy", "user": "alice"}))
            .unwrap();
        assert_eq!(auditor.last_entry_id(), Some(entry.entry_id.as_str()));

        let violations = auditor.verify_integrity().unwrap();
        assert!(violations.is_empty(), "Chain should be intact");
    }

    #[test]
    fn test_multiple_entries() {
        let (_dir, mut auditor) = setup_auditor();

        auditor
            .append(serde_json::json!({"event": "first"}))
            .unwrap();
        auditor
            .append(serde_json::json!({"event": "second"}))
            .unwrap();
        auditor
            .append(serde_json::json!({"event": "third"}))
            .unwrap();

        assert_eq!(auditor.entry_count().unwrap(), 3);
        let violations = auditor.verify_integrity().unwrap();
        assert!(violations.is_empty());
    }

    #[test]
    fn test_detect_tampering() {
        let (_dir, mut auditor) = setup_auditor();

        auditor
            .append(serde_json::json!({"event": "login"}))
            .unwrap();
        auditor
            .append(serde_json::json!({"event": "deploy"}))
            .unwrap();
        auditor
            .append(serde_json::json!({"event": "logout"}))
            .unwrap();

        // Manually tamper with the chain file
        let content = fs::read_to_string(&auditor.chain_file).unwrap();
        let tampered = content.replace("login", "admin_login");
        fs::write(&auditor.chain_file, tampered).unwrap();

        let violations = auditor.verify_integrity().unwrap();
        assert!(!violations.is_empty(), "Tampering should be detected");
    }

    #[test]
    fn test_export_report() {
        let (_dir, mut auditor) = setup_auditor();

        let now = current_timestamp_ms();
        auditor.append(serde_json::json!({"event": "old"})).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let later = current_timestamp_ms();

        let report = auditor.export_audit_report(now, later).unwrap();
        // At minimum, the "old" event should be within range
        assert!(report.entry_count >= 1);
    }

    #[test]
    fn test_entry_hash_chain() {
        let (_dir, mut auditor) = setup_auditor();

        let e1 = auditor.append(serde_json::json!({"idx": 1})).unwrap();
        let e2 = auditor.append(serde_json::json!({"idx": 2})).unwrap();

        // e2.prev_hash should equal e1.compute_hash()
        assert_eq!(e2.prev_hash, e1.compute_hash());
    }

    #[test]
    fn test_genesis_hash() {
        let auditor =
            HashChainAuditor::new(PathBuf::from("/tmp/nonexistent_chain_test_file")).unwrap();
        assert_eq!(
            auditor.current_hash(),
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
    }
}
