//! Secret Rotation (GAP-B52-26)
//!
//! Manages cryptographic key lifecycle with automatic rotation policies.
//! Supports multiple backends: keyring (OS keychain), environment variables,
//! and HashiCorp Vault (stub). Keys are auto-rotated on access when stale.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("key not found: {0}")]
    KeyNotFound(String),

    #[error("rotation failed: {0}")]
    RotationFailed(String),

    #[error("backend error: {0}")]
    BackendError(String),

    #[error("encoding error: {0}")]
    EncodingError(String),

    #[error("policy violation: {0}")]
    PolicyViolation(String),
}

// ---------------------------------------------------------------------------
// KeyId type alias
// ---------------------------------------------------------------------------

pub type KeyId = String;

// ---------------------------------------------------------------------------
// SecretEntry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretEntry {
    pub key_id: KeyId,
    pub key_bytes: Vec<u8>,
    pub algorithm: SecretAlgorithm,
    pub created_at_ms: u64,
    pub rotated_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub metadata: HashMap<String, String>,
}

impl SecretEntry {
    /// Create a new secret entry with the current timestamp.
    pub fn new(
        key_id: KeyId,
        key_bytes: Vec<u8>,
        algorithm: SecretAlgorithm,
        ttl: Option<Duration>,
    ) -> Self {
        let now = current_timestamp_ms();
        Self {
            key_id,
            key_bytes,
            algorithm,
            created_at_ms: now,
            rotated_at_ms: now,
            expires_at_ms: ttl.map(|d| now + d.as_millis() as u64),
            metadata: HashMap::new(),
        }
    }

    /// Check if the secret has expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at_ms
            .map(|exp| current_timestamp_ms() > exp)
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// SecretAlgorithm
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SecretAlgorithm {
    Ed25519,
    HmacSha256,
    Aes256Gcm,
    Generic,
}

// ---------------------------------------------------------------------------
// RotationPolicy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationPolicy {
    /// Maximum age of a key before rotation is required (in seconds).
    pub max_age_secs: u64,
    /// Whether to auto-rotate on access when the key is stale.
    pub auto_rotate_on_access: bool,
    /// Number of previous key versions to retain.
    pub retain_versions: u32,
    /// Minimum key length in bytes.
    pub min_key_length: usize,
}

impl Default for RotationPolicy {
    fn default() -> Self {
        Self {
            max_age_secs: 86400 * 90, // 90 days
            auto_rotate_on_access: true,
            retain_versions: 2,
            min_key_length: 16,
        }
    }
}

// ---------------------------------------------------------------------------
// KeyRotator trait
// ---------------------------------------------------------------------------

/// Trait for key rotation backends.
#[async_trait::async_trait]
pub trait KeyRotator: Send + Sync {
    /// Generate a new key with the given key ID and algorithm.
    async fn generate_key(
        &self,
        key_id: &str,
        algorithm: SecretAlgorithm,
    ) -> Result<SecretEntry, SecretError>;

    /// Store a secret entry.
    async fn store_key(&self, entry: &SecretEntry) -> Result<(), SecretError>;

    /// Retrieve a secret entry.
    async fn retrieve_key(&self, key_id: &str) -> Result<Option<SecretEntry>, SecretError>;

    /// Delete a secret entry.
    async fn delete_key(&self, key_id: &str) -> Result<(), SecretError>;

    /// List all key IDs managed by this rotator.
    async fn list_keys(&self) -> Result<Vec<KeyId>, SecretError>;
}

// ---------------------------------------------------------------------------
// MemoryRotator (in-memory, for testing / single-node)
// ---------------------------------------------------------------------------

/// Rotator backed by an in-memory HashMap.
pub struct MemoryRotator {
    keys: RwLock<HashMap<KeyId, SecretEntry>>,
}

impl MemoryRotator {
    pub fn new() -> Self {
        Self {
            keys: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryRotator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl KeyRotator for MemoryRotator {
    async fn generate_key(
        &self,
        key_id: &str,
        algorithm: SecretAlgorithm,
    ) -> Result<SecretEntry, SecretError> {
        let key_bytes = match algorithm {
            SecretAlgorithm::Ed25519 => {
                use rand::RngCore;
                let mut key = vec![0u8; 64];
                rand::rngs::OsRng.fill_bytes(&mut key);
                key
            }
            _ => {
                use rand::RngCore;
                let mut key = vec![0u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut key);
                key
            }
        };

        let entry = SecretEntry::new(key_id.to_string(), key_bytes, algorithm, None);
        self.store_key(&entry).await?;
        Ok(entry)
    }

    async fn store_key(&self, entry: &SecretEntry) -> Result<(), SecretError> {
        self.keys
            .write()
            .await
            .insert(entry.key_id.clone(), entry.clone());
        debug!(key = %entry.key_id, "Stored in memory");
        Ok(())
    }
    async fn retrieve_key(&self, key_id: &str) -> Result<Option<SecretEntry>, SecretError> {
        Ok(self.keys.read().await.get(key_id).cloned())
    }

    async fn delete_key(&self, key_id: &str) -> Result<(), SecretError> {
        self.keys.write().await.remove(key_id);
        info!(key = %key_id, "Deleted from memory");
        Ok(())
    }

    async fn list_keys(&self) -> Result<Vec<KeyId>, SecretError> {
        Ok(self.keys.read().await.keys().cloned().collect())
    }
}

// ---------------------------------------------------------------------------
// EnvRotator (environment variables)
// ---------------------------------------------------------------------------

/// Rotator backed by environment variables.
#[allow(dead_code)]
pub struct EnvRotator {
    prefix: String,
}

#[allow(dead_code)]
impl EnvRotator {
    pub fn new(prefix: String) -> Self {
        Self { prefix }
    }

    fn env_key(&self, key_id: &str) -> String {
        format!(
            "{}_{}",
            self.prefix,
            key_id.to_uppercase().replace('-', "_")
        )
    }
}

#[async_trait::async_trait]
impl KeyRotator for EnvRotator {
    async fn generate_key(
        &self,
        key_id: &str,
        algorithm: SecretAlgorithm,
    ) -> Result<SecretEntry, SecretError> {
        use rand::RngCore;
        let mut key = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);

        let encoded = base64::engine::general_purpose::STANDARD.encode(&key);
        std::env::set_var(self.env_key(key_id), encoded);

        let entry = SecretEntry::new(key_id.to_string(), key, algorithm, None);
        debug!(key = %key_id, "Stored in environment variable");
        Ok(entry)
    }

    async fn store_key(&self, entry: &SecretEntry) -> Result<(), SecretError> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&entry.key_bytes);
        std::env::set_var(self.env_key(&entry.key_id), encoded);
        Ok(())
    }

    async fn retrieve_key(&self, key_id: &str) -> Result<Option<SecretEntry>, SecretError> {
        let env_key = self.env_key(key_id);
        match std::env::var(&env_key) {
            Ok(encoded) => {
                let key_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&encoded)
                    .map_err(|e| SecretError::EncodingError(e.to_string()))?;
                Ok(Some(SecretEntry::new(
                    key_id.to_string(),
                    key_bytes,
                    SecretAlgorithm::Generic,
                    None,
                )))
            }
            Err(_) => Ok(None),
        }
    }

    async fn delete_key(&self, key_id: &str) -> Result<(), SecretError> {
        std::env::remove_var(self.env_key(key_id));
        info!(key = %key_id, "Deleted from environment");
        Ok(())
    }

    async fn list_keys(&self) -> Result<Vec<KeyId>, SecretError> {
        // Environment variables don't support listing by prefix easily.
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// VaultRotator (HashiCorp Vault integration)
// ---------------------------------------------------------------------------

/// Rotator backed by HashiCorp Vault.
///
/// When the `vault` feature is enabled, uses reqwest to connect
/// to a HashiCorp Vault server for key management. Without it, all operations
/// return `BackendError("Vault not configured")`.
#[allow(dead_code)] // Reserved—wired via server startup when vault is configured
/// Wired via server startup when vault feature is enabled.
pub struct VaultRotator {
    endpoint: String,
    /// Vault API token; only available when the `vault` feature is enabled.
    #[cfg(feature = "vault")]
    token: String,
    mount_path: String,
    #[cfg(feature = "vault")]
    client: &'static reqwest::Client,
}

impl VaultRotator {
    /// Create a new VaultRotator.
    ///
    /// When the `vault` feature is enabled, uses reqwest to call the
    /// HashiCorp Vault REST API. Without it, all operations return
    /// `BackendError("Vault not configured")`.
    #[allow(dead_code)] // Reserved—wired via server startup when vault is configured
    /// Wired via server startup when vault feature is enabled.
    pub fn new(
        endpoint: String,
        #[cfg(feature = "vault")] token: String,
        mount_path: String,
    ) -> Self {
        Self {
            endpoint,
            #[cfg(feature = "vault")]
            token,
            mount_path,
            #[cfg(feature = "vault")]
            client: crate::shared::http_client::http_client(),
        }
    }

    /// Build common headers for Vault API calls.
    #[cfg(feature = "vault")]
    #[allow(dead_code)] // Reserved—wired via server startup
    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&self.token) {
            headers.insert("X-Vault-Token", value);
        }
        headers
    }
}

#[async_trait::async_trait]
impl KeyRotator for VaultRotator {
    async fn generate_key(
        &self,
        key_id: &str,
        algorithm: SecretAlgorithm,
    ) -> Result<SecretEntry, SecretError> {
        #[cfg(feature = "vault")]
        {
            let key_type = match algorithm {
                SecretAlgorithm::Aes256Gcm => "aes256-gcm96",
                SecretAlgorithm::HmacSha256 => "hmac",
                SecretAlgorithm::Ed25519 => "ed25519",
                SecretAlgorithm::Generic => "aes256-gcm96",
            };
            let url = format!(
                "{}/v1/{}/keys/{}",
                self.endpoint.trim_end_matches('/'),
                self.mount_path.trim_matches('/'),
                key_id
            );
            let body = serde_json::json!({"type": key_type});
            let resp = self
                .client
                .post(&url)
                .headers(self.headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| SecretError::BackendError(format!("Vault HTTP error: {}", e)))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(SecretError::BackendError(format!(
                    "Vault API error ({}): {}",
                    status, text
                )));
            }
            let entry = SecretEntry {
                key_id: key_id.to_string(),
                key_bytes: vec![],
                algorithm,
                created_at_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                rotated_at_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                expires_at_ms: None,
                metadata: std::collections::HashMap::new(),
            };
            tracing::info!(target: "vault", key = %key_id, "VaultRotator: key created");
            return Ok(entry);
        }
        #[cfg(not(feature = "vault"))]
        {
            #[allow(unused_variables)]
            let _ = (key_id, algorithm);
            return Err(SecretError::BackendError(format!(
                "Vault not configured: would create key {} at {}/{}",
                key_id, self.endpoint, self.mount_path
            )));
        }
    }

    async fn store_key(&self, entry: &SecretEntry) -> Result<(), SecretError> {
        #[cfg(feature = "vault")]
        {
            let path = format!("data/{}", entry.key_id);
            let url = format!(
                "{}/v1/{}/{}",
                self.endpoint.trim_end_matches('/'),
                self.mount_path.trim_matches('/'),
                path
            );
            let body = serde_json::json!({
                "data": {
                    "key_id": entry.key_id,
                    "algorithm": format!("{:?}", entry.algorithm),
                    "created_ms": entry.created_at_ms,
                    "key_bytes": entry.key_bytes,
                }
            });
            let resp = self
                .client
                .post(&url)
                .headers(self.headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| SecretError::BackendError(format!("Vault HTTP error: {}", e)))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(SecretError::BackendError(format!(
                    "Vault API error ({}): {}",
                    status, text
                )));
            }
            tracing::debug!(target: "vault", key = %entry.key_id, "VaultRotator: key stored");
            return Ok(());
        }
        #[cfg(not(feature = "vault"))]
        {
            #[allow(unused_variables)]
            let _ = entry;
            return Err(SecretError::BackendError(
                "Vault not configured: store".into(),
            ));
        }
    }

    async fn retrieve_key(&self, key_id: &str) -> Result<Option<SecretEntry>, SecretError> {
        #[cfg(feature = "vault")]
        {
            let url = format!(
                "{}/v1/{}/keys/{}",
                self.endpoint.trim_end_matches('/'),
                self.mount_path.trim_matches('/'),
                key_id
            );
            let resp = self
                .client
                .get(&url)
                .headers(self.headers())
                .send()
                .await
                .map_err(|e| SecretError::BackendError(format!("Vault HTTP error: {}", e)))?;
            if resp.status().as_u16() == 404 {
                return Ok(None);
            }
            if !resp.status().is_success() {
                let status = resp.status();
                return Err(SecretError::BackendError(format!(
                    "Vault API error ({}): key not found",
                    status
                )));
            }
            let entry = SecretEntry {
                key_id: key_id.to_string(),
                key_bytes: vec![],
                algorithm: SecretAlgorithm::Aes256Gcm,
                created_at_ms: 0,
                rotated_at_ms: 0,
                expires_at_ms: None,
                metadata: std::collections::HashMap::new(),
            };
            tracing::debug!(target: "vault", key = %key_id, "VaultRotator: key retrieved");
            return Ok(Some(entry));
        }
        #[cfg(not(feature = "vault"))]
        {
            #[allow(unused_variables)]
            let _ = key_id;
            return Err(SecretError::BackendError(
                "Vault not configured: retrieve".into(),
            ));
        }
    }

    async fn delete_key(&self, key_id: &str) -> Result<(), SecretError> {
        #[cfg(feature = "vault")]
        {
            let url = format!(
                "{}/v1/{}/keys/{}",
                self.endpoint.trim_end_matches('/'),
                self.mount_path.trim_matches('/'),
                key_id
            );
            let resp = self
                .client
                .delete(&url)
                .headers(self.headers())
                .send()
                .await
                .map_err(|e| SecretError::BackendError(format!("Vault HTTP error: {}", e)))?;
            if !resp.status().is_success() {
                let status = resp.status();
                return Err(SecretError::BackendError(format!(
                    "Vault API error ({}): delete failed",
                    status
                )));
            }
            tracing::debug!(target: "vault", key = %key_id, "VaultRotator: key deleted");
            return Ok(());
        }
        #[cfg(not(feature = "vault"))]
        {
            #[allow(unused_variables)]
            let _ = key_id;
            return Err(SecretError::BackendError(
                "Vault not configured: delete".into(),
            ));
        }
    }

    async fn list_keys(&self) -> Result<Vec<KeyId>, SecretError> {
        #[cfg(feature = "vault")]
        {
            let url = format!(
                "{}/v1/{}/keys?list=true",
                self.endpoint.trim_end_matches('/'),
                self.mount_path.trim_matches('/'),
            );
            let resp = self
                .client
                .get(&url)
                .headers(self.headers())
                .send()
                .await
                .map_err(|e| SecretError::BackendError(format!("Vault HTTP error: {}", e)))?;
            if !resp.status().is_success() {
                let status = resp.status();
                return Err(SecretError::BackendError(format!(
                    "Vault API error ({}): list failed",
                    status
                )));
            }
            tracing::debug!(target: "vault", "VaultRotator: listed keys");
            return Ok(vec![]);
        }
        #[cfg(not(feature = "vault"))]
        {
            return Err(SecretError::BackendError(
                "Vault not configured: list".into(),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// SecretManager
// ---------------------------------------------------------------------------

/// Central secret manager that handles key lifecycle, rotation policy,
/// and delegates storage to a configurable KeyRotator backend.
pub struct SecretManager {
    /// In-memory cache of active secrets.
    secrets: RwLock<HashMap<KeyId, SecretEntry>>,
    /// Previous key versions (for decryption of old data).
    previous_versions: RwLock<HashMap<KeyId, Vec<SecretEntry>>>,
    /// Rotation policy.
    rotation_policy: RotationPolicy,
    /// The backend rotator.
    rotator: Arc<dyn KeyRotator>,
}

impl SecretManager {
    pub fn new(rotation_policy: RotationPolicy, rotator: Arc<dyn KeyRotator>) -> Self {
        Self {
            secrets: RwLock::new(HashMap::new()),
            previous_versions: RwLock::new(HashMap::new()),
            rotation_policy,
            rotator,
        }
    }

    /// Register a new key. If it already exists, this is a no-op.
    pub async fn register_key(
        &self,
        key_id: KeyId,
        algorithm: SecretAlgorithm,
    ) -> Result<SecretEntry, SecretError> {
        // Check if already cached
        {
            let secrets = self.secrets.read().await;
            if let Some(entry) = secrets.get(&key_id) {
                return Ok(entry.clone());
            }
        }

        // Check if the backend already has it
        if let Some(entry) = self.rotator.retrieve_key(&key_id).await? {
            self.secrets.write().await.insert(key_id, entry.clone());
            return Ok(entry);
        }

        // Generate a new key
        let entry = self.rotator.generate_key(&key_id, algorithm).await?;
        self.secrets
            .write()
            .await
            .insert(key_id.clone(), entry.clone());
        info!(key = %key_id, "New key registered");
        Ok(entry)
    }

    /// Get a key by ID. Auto-rotates if the key is stale according to the policy.
    pub async fn get_key(&self, key_id: &str) -> Result<SecretEntry, SecretError> {
        // Check cache first
        {
            let secrets = self.secrets.read().await;
            if let Some(entry) = secrets.get(key_id) {
                if !self.needs_rotation(entry) {
                    return Ok(entry.clone());
                }
            }
        }

        // Try backend
        if let Some(entry) = self.rotator.retrieve_key(key_id).await? {
            if self.needs_rotation(&entry) && self.rotation_policy.auto_rotate_on_access {
                return self.rotate_key(key_id).await;
            }
            self.secrets
                .write()
                .await
                .insert(key_id.to_string(), entry.clone());
            return Ok(entry);
        }

        Err(SecretError::KeyNotFound(key_id.to_string()))
    }

    /// Rotate a key, generating a new key and retaining the old version.
    pub async fn rotate_key(&self, key_id: &str) -> Result<SecretEntry, SecretError> {
        let algorithm = {
            let cached = { self.secrets.read().await.get(key_id).cloned() };
            if let Some(entry) = cached {
                entry.algorithm
            } else if let Ok(Some(entry)) = self.rotator.retrieve_key(key_id).await {
                entry.algorithm
            } else {
                SecretAlgorithm::Generic
            }
        };

        // Archive current key as a previous version
        if let Ok(Some(old_entry)) = self.rotator.retrieve_key(key_id).await {
            let mut versions = self.previous_versions.write().await;
            let versions_list = versions.entry(key_id.to_string()).or_default();
            versions_list.push(old_entry);
            // Trim to retain_versions
            while versions_list.len() > self.rotation_policy.retain_versions as usize {
                versions_list.remove(0);
            }
        }

        // Delete old key from backend
        let _ = self.rotator.delete_key(key_id).await;

        // Generate new key
        let new_entry = self.rotator.generate_key(key_id, algorithm).await?;

        // Update cache
        self.secrets
            .write()
            .await
            .insert(key_id.to_string(), new_entry.clone());

        info!(key = %key_id, "Key rotated");
        Ok(new_entry)
    }

    /// Get previous versions of a key (for decrypting data encrypted with old keys).
    pub async fn get_previous_versions(&self, key_id: &str) -> Vec<SecretEntry> {
        self.previous_versions
            .read()
            .await
            .get(key_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Check if a key needs rotation based on the policy.
    fn needs_rotation(&self, entry: &SecretEntry) -> bool {
        if entry.is_expired() {
            return true;
        }
        let age_secs = (current_timestamp_ms().saturating_sub(entry.rotated_at_ms)) / 1000;
        age_secs >= self.rotation_policy.max_age_secs
    }

    /// Get the current rotation policy.
    pub fn rotation_policy(&self) -> &RotationPolicy {
        &self.rotation_policy
    }

    /// Update the rotation policy.
    pub fn set_rotation_policy(&mut self, policy: RotationPolicy) {
        self.rotation_policy = policy;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_env_manager() -> SecretManager {
        let policy = RotationPolicy {
            max_age_secs: 999999,
            auto_rotate_on_access: false,
            retain_versions: 2,
            min_key_length: 16,
        };
        let rotator = Arc::new(EnvRotator::new("GOON_TEST".into()));
        SecretManager::new(policy, rotator)
    }

    #[tokio::test]
    async fn test_register_and_get_key() {
        let mgr = make_env_manager();
        let entry = mgr
            .register_key("test-key-1".into(), SecretAlgorithm::Generic)
            .await
            .unwrap();
        assert_eq!(entry.key_id, "test-key-1");

        let fetched = mgr.get_key("test-key-1").await.unwrap();
        assert_eq!(fetched.key_id, "test-key-1");
        assert_eq!(fetched.key_bytes.len(), 32);
    }

    #[tokio::test]
    async fn test_get_nonexistent_key() {
        let mgr = make_env_manager();
        let err = mgr.get_key("nonexistent").await.unwrap_err();
        assert!(matches!(err, SecretError::KeyNotFound(_)));
    }

    #[tokio::test]
    async fn test_rotate_key() {
        let mgr = make_env_manager();
        let original = mgr
            .register_key("rotate-key".into(), SecretAlgorithm::HmacSha256)
            .await
            .unwrap();

        let rotated = mgr.rotate_key("rotate-key").await.unwrap();
        assert_ne!(original.key_bytes, rotated.key_bytes);
    }

    #[tokio::test]
    async fn test_previous_versions() {
        let mgr = make_env_manager();
        let _first = mgr
            .register_key("ver-key".into(), SecretAlgorithm::Generic)
            .await
            .unwrap();
        let _second = mgr.rotate_key("ver-key").await.unwrap();
        let _third = mgr.rotate_key("ver-key").await.unwrap();

        let versions = mgr.get_previous_versions("ver-key").await;
        assert_eq!(versions.len(), 2); // retain_versions = 2
    }
}
