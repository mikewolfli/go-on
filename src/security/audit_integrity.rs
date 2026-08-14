//! Audit Integrity — Hash Chain Auditor (GAP-B52-27)
//!
//! Tamper-evident audit logging primitives built on a hash chain.
//! Each entry includes the previous entry's hash, a payload hash,
//! a timestamp, and an optional signature. The chain can be verified
//! for integrity and exported as a report.
//!
//! # Integration
//!
//! Since the audit-pipeline unification, [`HashChainAuditor`] is the integrity
//! primitive of the canonical audit sink ([`crate::governance::audit`]): the
//! sink's background writer thread chains **every** persisted record to the
//! sibling `audit_chain.ndjson` file. Production code therefore never calls
//! [`HashChainAuditor`] directly — the module is exercised by the sink and by
//! the integrity tests in [`crate::governance::audit`] and
//! `tests/structural/test_security.rs`.

use base64::Engine;
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
        let payload_hash = crate::shared::sha256_hex(&payload_bytes);

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

    /// Sign this entry using the provided Ed25519 private key.
    /// This sets the `signature` and `key_id` fields.
    /// The signature covers the entry's canonical hash (`compute_hash()`).
    pub fn sign(&mut self, key_id: &str, private_key: &[u8]) -> Result<(), AuditError> {
        let body = self.compute_hash().into_bytes();

        use ed25519_dalek::Signer;
        let signing_key = if private_key.len() == 64 {
            let arr: &[u8; 64] = private_key.try_into().map_err(|_| {
                AuditError::SignatureVerificationFailed("Ed25519 keypair must be 64 bytes".into())
            })?;
            ed25519_dalek::SigningKey::from_keypair_bytes(arr)
                .map_err(|e| AuditError::SignatureVerificationFailed(e.to_string()))?
        } else if private_key.len() == 32 {
            let arr: &[u8; 32] = private_key.try_into().map_err(|_| {
                AuditError::SignatureVerificationFailed("Ed25519 seed must be 32 bytes".into())
            })?;
            ed25519_dalek::SigningKey::from_bytes(arr)
        } else {
            return Err(AuditError::SignatureVerificationFailed(
                "Ed25519 key must be 32 (seed) or 64 (keypair) bytes".into(),
            ));
        };

        let signature_bytes = signing_key.sign(&body).to_bytes().to_vec();
        self.signature = Some(base64::engine::general_purpose::STANDARD.encode(&signature_bytes));
        self.key_id = Some(key_id.to_string());
        Ok(())
    }

    /// Verify this entry's Ed25519 signature against the provided public key.
    ///
    /// The signature must have been created by [`AuditEntry::sign`] (i.e., it covers
    /// the entry's canonical hash returned by [`Self::compute_hash`]).
    pub fn verify_signature(&self, public_key: &[u8]) -> Result<(), AuditError> {
        let sig_b64 = self.signature.as_ref().ok_or_else(|| {
            AuditError::SignatureVerificationFailed("entry has no signature".to_string())
        })?;

        let body = self.compute_hash().into_bytes();
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(sig_b64)
            .map_err(|e| AuditError::SignatureVerificationFailed(e.to_string()))?;

        let pub_key_bytes: &[u8; 32] = public_key.try_into().map_err(|_| {
            AuditError::SignatureVerificationFailed("public key must be 32 bytes".to_string())
        })?;
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(pub_key_bytes)
            .map_err(|e| AuditError::SignatureVerificationFailed(e.to_string()))?;

        let sig_arr: &[u8; 64] = sig_bytes.as_slice().try_into().map_err(|_| {
            AuditError::SignatureVerificationFailed("signature must be 64 bytes".to_string())
        })?;
        let sig = ed25519_dalek::Signature::from_bytes(sig_arr);

        use ed25519_dalek::Verifier;
        verifying_key
            .verify(&body, &sig)
            .map_err(|e| AuditError::SignatureVerificationFailed(e.to_string()))?;

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
///
/// Optionally configured with a signing key to automatically sign every
/// entry for non-repudiation.
#[derive(Debug)]
pub struct HashChainAuditor {
    /// Path to the chain file (newline-delimited JSON).
    chain_file: PathBuf,
    /// Current hash of the most recent entry (for quick chaining).
    current_hash: String,
    /// ID of the last entry written.
    last_entry_id: Option<String>,
    /// Optional signing key (key_id, private key bytes) used to sign every
    /// entry appended via [`append()`].
    signing_key: Option<(String, Vec<u8>)>,
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
            signing_key: None,
        })
    }

    /// Create a new auditor that automatically signs each entry with the
    /// provided Ed25519 key.
    ///
    /// `key_id` is a human-readable identifier for the key (e.g., "ed25519-key-1").
    /// `private_key` must be the 32-byte seed or 64-byte keypair as expected by
    /// [`AuditEntry::sign`].
    pub fn new_signed(
        chain_file: PathBuf,
        key_id: &str,
        private_key: &[u8],
    ) -> Result<Self, AuditError> {
        let mut auditor = Self::new(chain_file)?;
        auditor.signing_key = Some((key_id.to_string(), private_key.to_vec()));
        Ok(auditor)
    }

    /// Append a new audit entry to the chain.
    pub fn append(&mut self, payload: serde_json::Value) -> Result<AuditEntry, AuditError> {
        let entry_id = uuid::Uuid::new_v4().to_string();
        let timestamp_ms = crate::shared::timestamps::now_ts_ms_u64();

        let mut entry = AuditEntry::new(entry_id, self.current_hash.clone(), payload, timestamp_ms);

        // Sign the entry if a signing key is configured
        if let Some((key_id, private_key)) = &self.signing_key {
            entry.sign(key_id, private_key)?;
        }

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

    /// Verify the integrity of the entire hash chain and optionally verify
    /// Ed25519 signatures.
    ///
    /// Hash chain validation checks:
    /// - `prev_hash` continuity between consecutive entries
    /// - `payload_hash` consistency with the stored payload
    ///
    /// If `public_key` is provided and an entry carries a signature, the entry's
    /// Ed25519 signature is verified against its canonical hash.
    ///
    /// Returns a list of violations (empty if the chain is intact).
    pub fn verify_integrity(
        &self,
        public_key: Option<&[u8]>,
    ) -> Result<Vec<IntegrityViolation>, AuditError> {
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
            let computed_payload_hash = crate::shared::sha256_hex(&payload_bytes);
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

            // Verify Ed25519 signature when a public key is provided
            if entry.signature.is_some() {
                if let Some(pk) = public_key {
                    if let Err(e) = entry.verify_signature(pk) {
                        violations.push(IntegrityViolation {
                            entry_id: entry.entry_id.clone(),
                            expected_prev_hash: expected_prev_hash.clone(),
                            actual_prev_hash: entry.prev_hash.clone(),
                            reason: format!("signature verification failed: {e}"),
                            entry: Some(entry.clone()),
                        });
                    }
                }
            }

            // Store this entry's hash as the expected prev_hash for the next entry
            expected_prev_hash = entry.compute_hash();
        }

        Ok(violations)
    }

    /// Export an audit report for entries within a time range.
    pub fn export_audit_report(
        &self,
        from_ms: u64,
        to_ms: u64,
        public_key: Option<&[u8]>,
    ) -> Result<AuditReport, AuditError> {
        let all_entries = Self::load_all_entries(&self.chain_file)?;

        let filtered: Vec<AuditEntry> = all_entries
            .into_iter()
            .filter(|e| e.timestamp_ms >= from_ms && e.timestamp_ms <= to_ms)
            .collect();

        let violations = self.verify_integrity(public_key)?;
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

        let violations = auditor.verify_integrity(None).unwrap();
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
        let violations = auditor.verify_integrity(None).unwrap();
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

        let violations = auditor.verify_integrity(None).unwrap();
        assert!(!violations.is_empty(), "Tampering should be detected");
    }

    #[test]
    fn test_export_report() {
        let (_dir, mut auditor) = setup_auditor();

        let now = crate::shared::timestamps::now_ts_ms_u64();
        auditor.append(serde_json::json!({"event": "old"})).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let later = crate::shared::timestamps::now_ts_ms_u64();

        let report = auditor.export_audit_report(now, later, None).unwrap();
        // At minimum, the "old" event should be within range
        assert!(report.entry_count >= 1);
    }

    #[test]
    fn test_sign_and_verify_signature() {
        let (_dir, mut auditor) = setup_auditor();

        // Generate an Ed25519 key pair
        let mut seed = [0u8; 32];
        use rand::Rng;
        rand::rng().fill_bytes(&mut seed);
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();

        // Append an entry and manually sign it
        let mut entry = auditor
            .append(serde_json::json!({"event": "sensitive_operation"}))
            .unwrap();
        entry
            .sign("ed25519-key-1", &signing_key.to_keypair_bytes())
            .unwrap();

        // Verify the signature with the correct public key
        assert!(entry.verify_signature(&verifying_key.to_bytes()).is_ok());

        // Verify with a wrong public key fails
        let wrong_pk = [0u8; 32];
        assert!(entry.verify_signature(&wrong_pk).is_err());
    }

    #[test]
    fn test_signature_tampering_detected() {
        let (_dir, mut auditor) = setup_auditor();

        // Generate an Ed25519 key pair
        let mut seed = [0u8; 32];
        use rand::Rng;
        rand::rng().fill_bytes(&mut seed);
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();

        // Append a signed entry
        let mut entry = auditor
            .append(serde_json::json!({"event": "deploy"}))
            .unwrap();
        entry
            .sign("ed25519-key-1", &signing_key.to_keypair_bytes())
            .unwrap();

        // Tamper with the payload (changes compute_hash)
        entry.payload = serde_json::json!({"event": "deploy_tampered"});
        // Also update payload_hash to reflect the tampered payload
        let tampered_bytes = serde_json::to_vec(&entry.payload).unwrap_or_default();
        entry.payload_hash = crate::shared::sha256_hex(&tampered_bytes);

        // Signature should now fail (different compute_hash)
        assert!(entry.verify_signature(&verifying_key.to_bytes()).is_err());
    }

    #[test]
    fn test_verify_integrity_with_signatures() {
        let (_dir, mut auditor) = setup_auditor();

        // Generate an Ed25519 key pair
        let mut seed = [0u8; 32];
        use rand::Rng;
        rand::rng().fill_bytes(&mut seed);
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();

        // Append unsigned entries first
        auditor
            .append(serde_json::json!({"event": "first"}))
            .unwrap();

        // Append a signed entry (manually create and overwrite)
        let mut entry = auditor
            .append(serde_json::json!({"event": "signed_event"}))
            .unwrap();
        entry
            .sign("ed25519-key-1", &signing_key.to_keypair_bytes())
            .unwrap();

        // We have to replace the entry in the chain file for verify_integrity to see it
        // Re-read all entries, update the last one, and write back
        let all_entries = HashChainAuditor::load_all_entries(&auditor.chain_file).unwrap();
        let mut updated = all_entries;
        updated.pop(); // remove unsigned version
        updated.push(entry); // add signed version

        let mut file = fs::File::create(&auditor.chain_file).unwrap();
        for e in &updated {
            writeln!(file, "{}", serde_json::to_string(e).unwrap()).unwrap();
        }
        drop(file);

        // Re-create auditor to pick up signed entries
        let auditor = HashChainAuditor::new(auditor.chain_file.clone()).unwrap();

        // Verify with correct public key -- should pass
        let violations = auditor
            .verify_integrity(Some(&verifying_key.to_bytes()))
            .unwrap();
        assert!(
            violations.is_empty(),
            "Chain with valid signatures should be intact: {:?}",
            violations
        );

        // Verify with wrong public key -- should report signature violations
        let wrong_pk = [0u8; 32];
        let violations = auditor.verify_integrity(Some(&wrong_pk)).unwrap();
        assert!(
            !violations.is_empty(),
            "Wrong public key should detect signature violations"
        );

        // Verify with no public key -- should pass (skips signature check)
        let violations = auditor.verify_integrity(None).unwrap();
        assert!(
            violations.is_empty(),
            "Chain should be intact when skipping signature check"
        );
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
