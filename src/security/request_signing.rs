//! Request Signing (GAP-B52-23)
//!
//! Provides Ed25519 and HMAC-SHA256 request signing with replay protection
//! via a 30-second clock skew window. Supports sign_request and verify_request
//! operations for authenticating inter-module and inter-node communication.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SigningError {
    #[error("signature verification failed")]
    VerificationFailed,

    #[error("invalid key: {0}")]
    InvalidKey(String),

    #[error("replay detected: timestamp {ts} ms has clock skew of {skew_ms} ms")]
    ReplayDetected { ts: u64, skew_ms: i64 },

    #[error("body hash mismatch")]
    BodyHashMismatch,

    #[error("encoding error: {0}")]
    EncodingError(String),
}

// ---------------------------------------------------------------------------
// SigningAlgorithm
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SigningAlgorithm {
    Ed25519,
    HmacSha256,
}

impl std::fmt::Display for SigningAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SigningAlgorithm::Ed25519 => write!(f, "Ed25519"),
            SigningAlgorithm::HmacSha256 => write!(f, "HmacSha256"),
        }
    }
}

// ---------------------------------------------------------------------------
// RequestSignature
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestSignature {
    /// Raw signature bytes (base64-encoded for serialization).
    pub signature: String,
    /// The algorithm used to generate the signature.
    pub algorithm: SigningAlgorithm,
    /// Identifier for the key used to sign.
    pub key_id: String,
    /// Unix timestamp in milliseconds (for replay protection).
    pub timestamp_ms: u64,
    /// SHA-256 hash of the request body (base64-encoded).
    pub body_hash: String,
}

// ---------------------------------------------------------------------------
// Signing Helper
// ---------------------------------------------------------------------------

/// Maximum allowed clock skew in seconds for replay protection.
pub const MAX_CLOCK_SKEW_S: u64 = 30;

/// Maximum allowed clock skew in milliseconds.
pub const MAX_CLOCK_SKEW_MS: u64 = MAX_CLOCK_SKEW_S * 1000;

/// Verify a request signature against the provided public key and body.
///
/// For Ed25519, `public_key` must be a 32-byte public key.
/// For HmacSha256, `public_key` is the HMAC shared secret.
///
/// This function performs replay protection by checking clock skew (max 30s).
pub fn verify_request(
    public_key: &[u8],
    body: &[u8],
    signature: &RequestSignature,
) -> Result<bool, SigningError> {
    // 1. Replay protection: check timestamp clock skew
    let now_ms = current_timestamp_ms();
    let skew_ms = if now_ms > signature.timestamp_ms {
        (now_ms - signature.timestamp_ms) as i64
    } else {
        -((signature.timestamp_ms - now_ms) as i64)
    };

    if skew_ms.abs() > MAX_CLOCK_SKEW_MS as i64 {
        return Err(SigningError::ReplayDetected {
            ts: signature.timestamp_ms,
            skew_ms,
        });
    }

    // 2. Verify body hash matches (body integrity)
    let b64_engine = base64::engine::general_purpose::STANDARD;
    let computed_hash = b64_engine.encode(sha256(body));
    if computed_hash != signature.body_hash {
        return Err(SigningError::BodyHashMismatch);
    }

    // 3. Verify the signature
    let to_verify = signing_payload(body, signature.timestamp_ms);
    let sig_bytes = b64_engine
        .decode(&signature.signature)
        .map_err(|e| SigningError::EncodingError(e.to_string()))?;

    let result = match signature.algorithm {
        SigningAlgorithm::Ed25519 => {
            use ed25519_dalek::Verifier;
            let public =
                ed25519_dalek::VerifyingKey::from_bytes(&public_key.try_into().map_err(|_| {
                    SigningError::InvalidKey("Ed25519 public key must be 32 bytes".into())
                })?)
                .map_err(|e| SigningError::InvalidKey(e.to_string()))?;

            let sig =
                ed25519_dalek::Signature::from_bytes(&sig_bytes.try_into().map_err(|_| {
                    SigningError::InvalidKey("Ed25519 signature must be 64 bytes".into())
                })?);

            public.verify(&to_verify, &sig).is_ok()
        }
        SigningAlgorithm::HmacSha256 => {
            use hmac::{digest::KeyInit, Mac};
            let mut mac = hmac::Hmac::<Sha256>::new_from_slice(public_key)
                .map_err(|e| SigningError::InvalidKey(e.to_string()))?;
            mac.update(&to_verify);
            mac.verify_slice(&sig_bytes).is_ok()
        }
    };

    if !result {
        return Err(SigningError::VerificationFailed);
    }

    Ok(true)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build the payload that is actually signed: timestamp || body_hash.
fn signing_payload(body: &[u8], timestamp_ms: u64) -> Vec<u8> {
    let b64_engine = base64::engine::general_purpose::STANDARD;
    let body_hash = b64_engine.encode(sha256(body));
    let payload = format!("{}:{}", timestamp_ms, body_hash);
    payload.into_bytes()
}

fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Sign a request body and produce a `RequestSignature`.
///
/// For Ed25519, `key_bytes` must be a 32-byte signing key seed.
/// For HmacSha256, `key_bytes` is the HMAC shared secret.
///
/// Gated behind `#[cfg(test)]` — this is only used by test code.
/// Production signing is handled by dedicated signing middleware.
#[cfg(test)]
pub fn sign_request(
    key_bytes: &[u8],
    body: &[u8],
    algorithm: SigningAlgorithm,
    key_id: &str,
) -> Result<RequestSignature, SigningError> {
    let b64_engine = base64::engine::general_purpose::STANDARD;
    let body_hash = b64_engine.encode(sha256(body));
    let now_ms = current_timestamp_ms();
    let to_sign = signing_payload(body, now_ms);

    let signature = match algorithm {
        SigningAlgorithm::Ed25519 => {
            use ed25519_dalek::Signer;
            let signing_key =
                ed25519_dalek::SigningKey::from_bytes(&key_bytes.try_into().map_err(|_| {
                    SigningError::InvalidKey("Ed25519 key must be 32 bytes".into())
                })?);
            let sig = signing_key.sign(&to_sign);
            b64_engine.encode(sig.to_bytes())
        }
        SigningAlgorithm::HmacSha256 => {
            use hmac::{digest::KeyInit, Mac};
            let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key_bytes)
                .map_err(|e| SigningError::InvalidKey(e.to_string()))?;
            mac.update(&to_sign);
            let result = mac.finalize();
            b64_engine.encode(result.into_bytes().as_slice())
        }
    };

    Ok(RequestSignature {
        signature,
        algorithm,
        key_id: key_id.to_string(),
        timestamp_ms: now_ms,
        body_hash,
    })
}

/// Build a `RequestSignature` from raw fields for test use.
#[cfg(test)]
#[allow(dead_code)]
fn make_signature_for_test(
    algorithm: SigningAlgorithm,
    key_id: &str,
    timestamp_ms: u64,
    body_hash: String,
    signature: String,
) -> RequestSignature {
    RequestSignature {
        signature,
        algorithm,
        key_id: key_id.to_string(),
        timestamp_ms,
        body_hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{KeyInit, Mac};

    #[test]
    fn test_replay_detection() {
        let secret = b"hmac-secret-key-22";
        let body = b"stale request";

        // Construct a stale RequestSignature directly
        let b64_engine = base64::engine::general_purpose::STANDARD;
        let sig = RequestSignature {
            signature: "AAAA".to_string(),
            algorithm: SigningAlgorithm::HmacSha256,
            key_id: "k1".to_string(),
            timestamp_ms: 1, // way in the past
            body_hash: b64_engine.encode(sha256(body)),
        };

        let err = verify_request(secret, body, &sig).unwrap_err();
        assert!(matches!(err, SigningError::ReplayDetected { .. }));
    }

    #[test]
    fn test_body_tampering() {
        let secret = b"hmac-secret-key-33";
        let body = b"original body";
        let b64_engine = base64::engine::general_purpose::STANDARD;

        // Construct a valid signature for original body
        let to_sign = signing_payload(body, current_timestamp_ms());
        let mut mac = hmac::Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(&to_sign);
        let sig_bytes = mac.finalize().into_bytes().to_vec();

        let sig = RequestSignature {
            signature: b64_engine.encode(&sig_bytes),
            algorithm: SigningAlgorithm::HmacSha256,
            key_id: "k2".to_string(),
            timestamp_ms: current_timestamp_ms(),
            body_hash: b64_engine.encode(sha256(body)),
        };

        let result = verify_request(secret, b"tampered body", &sig);
        assert!(result.is_err());
    }

    #[test]
    fn test_body_hash_mismatch_error() {
        let secret = b"test-secret";
        let body = b"my body";
        let b64_engine = base64::engine::general_purpose::STANDARD;

        // Construct a signature with corrupted body hash
        let to_sign = signing_payload(body, current_timestamp_ms());
        let mut mac = hmac::Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(&to_sign);
        let sig_bytes = mac.finalize().into_bytes().to_vec();

        let mut sig = RequestSignature {
            signature: b64_engine.encode(&sig_bytes),
            algorithm: SigningAlgorithm::HmacSha256,
            key_id: "k3".to_string(),
            timestamp_ms: current_timestamp_ms(),
            body_hash: b64_engine.encode(sha256(body)),
        };

        // Corrupt the body hash
        sig.body_hash = "AAAA".to_string();

        let err = verify_request(secret, body, &sig).unwrap_err();
        assert!(matches!(err, SigningError::BodyHashMismatch));
    }
}
