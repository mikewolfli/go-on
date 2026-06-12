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
    /// Optional tenant identifier for multi-tenant isolation.
    pub tenant_id: Option<String>,
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
            tenant_id: None,
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
        // Zero the key bytes in memory before dropping (S-FIX10)
        let mut keys = self.keys.write().await;
        if let Some(entry) = keys.get_mut(key_id) {
            for byte in entry.key_bytes.iter_mut() {
                unsafe {
                    std::ptr::write_volatile(byte, 0u8);
                }
            }
        }
        keys.remove(key_id);
        info!(key = %key_id, "Deleted from memory (zeroed)");
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
#[allow(dead_code)] // F-GAP-49 — reserved secret rotation feature
pub struct EnvRotator {
    prefix: String,
}

#[allow(dead_code)] // F-GAP-49 — reserved secret rotation feature
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
            client: match crate::shared::http_client::http_client() {
                Ok(c) => c,
                Err(e) => {
                    panic!("VaultRotator: failed to build shared HTTP client: {e}")
                }
            },
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
                tenant_id: None,
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
                tenant_id: None,
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

/// Default tenant key used when no tenant_id is provided.
const DEFAULT_TENANT: &str = "_global";

/// Central secret manager that handles key lifecycle, rotation policy,
/// and delegates storage to a configurable KeyRotator backend.
///
/// Supports multi-tenant isolation: each tenant gets its own key namespace.
///
/// # Default configuration
///
/// Environment variables:
/// - `GO_ON_SECRET_ROTATION_ENABLED` — set to "1" or "true" to enable
/// - `GO_ON_SECRET_ROTATION_INTERVAL_SECS` — rotation interval in seconds (default: 86400)
/// - `GO_ON_SECRET_ROTATION_RETAIN_VERSIONS` — number of previous versions to retain (default: 5)
pub struct SecretManager {
    /// In-memory cache of active secrets, keyed by (tenant → key_id).
    secrets: RwLock<HashMap<String, HashMap<KeyId, SecretEntry>>>,
    /// Previous key versions (for decryption of old data), keyed by (tenant → key_id).
    previous_versions: RwLock<HashMap<String, HashMap<KeyId, Vec<SecretEntry>>>>,
    /// Rotation policy.
    rotation_policy: RotationPolicy,
    /// The backend rotator.
    rotator: Arc<dyn KeyRotator>,
}

impl SecretManager {
    fn tenant_key(tenant_id: Option<&str>) -> String {
        tenant_id.unwrap_or(DEFAULT_TENANT).to_string()
    }

    /// Qualify a key_id with tenant for backend rotator storage, ensuring
    /// different tenants get independent secret entries even with the same key_id.
    fn qualified_key(key_id: &str, tenant: &str) -> String {
        format!("{}/{}", tenant, key_id)
    }

    pub fn new(rotation_policy: RotationPolicy, rotator: Arc<dyn KeyRotator>) -> Self {
        Self {
            secrets: RwLock::new(HashMap::new()),
            previous_versions: RwLock::new(HashMap::new()),
            rotation_policy,
            rotator,
        }
    }

    /// Create a `SecretManager` with defaults from environment variables.
    ///
    /// - `GO_ON_SECRET_ROTATION_INTERVAL_SECS` → `max_age_secs` (default: 86400)
    /// - `GO_ON_SECRET_ROTATION_RETAIN_VERSIONS` → `retain_versions` (default: 5)
    pub fn with_defaults() -> Self {
        let max_age_secs = std::env::var("GO_ON_SECRET_ROTATION_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(86400);
        let retain_versions = std::env::var("GO_ON_SECRET_ROTATION_RETAIN_VERSIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);

        let policy = RotationPolicy {
            max_age_secs,
            auto_rotate_on_access: false,
            retain_versions,
            min_key_length: 32,
        };
        let rotator = Arc::new(MemoryRotator::new());

        Self {
            secrets: RwLock::new(HashMap::new()),
            previous_versions: RwLock::new(HashMap::new()),
            rotation_policy: policy,
            rotator,
        }
    }

    /// Register a new key. If it already exists, this is a no-op.
    ///
    /// `tenant_id` is optional; when `None`, the key is stored under the `"_global"` namespace.
    pub async fn register_key(
        &self,
        key_id: KeyId,
        algorithm: SecretAlgorithm,
        tenant_id: Option<&str>,
    ) -> Result<SecretEntry, SecretError> {
        let tenant = Self::tenant_key(tenant_id);

        // Check if already cached
        {
            let secrets = self.secrets.read().await;
            if let Some(tenant_map) = secrets.get(&tenant) {
                if let Some(entry) = tenant_map.get(&key_id) {
                    return Ok(entry.clone());
                }
            }
        }

        // Check if the backend already has it (tenant-qualified key_id)
        let rotator_key = Self::qualified_key(&key_id, &tenant);
        if let Some(entry) = self.rotator.retrieve_key(&rotator_key).await? {
            // Normalize key_id: rotator uses qualified key, caller expects original
            let mut result_entry = entry.clone();
            result_entry.key_id = key_id.clone();
            self.secrets
                .write()
                .await
                .entry(tenant.clone())
                .or_default()
                .insert(key_id, result_entry.clone());
            return Ok(result_entry);
        }

        // Generate a new key (tenant-qualified)
        let entry = self.rotator.generate_key(&rotator_key, algorithm).await?;
        // Normalize key_id: rotator uses qualified key, caller expects original
        let mut result_entry = entry.clone();
        result_entry.key_id = key_id.clone();
        self.secrets
            .write()
            .await
            .entry(tenant.clone())
            .or_default()
            .insert(key_id.clone(), result_entry.clone());
        info!(key = %key_id, tenant = %tenant, "New key registered");
        Ok(result_entry)
    }

    /// Get a key by ID. Auto-rotates if the key is stale according to the policy.
    ///
    /// `tenant_id` is optional; when `None`, looks up the key under the `"_global"` namespace.
    pub async fn get_key(
        &self,
        key_id: &str,
        tenant_id: Option<&str>,
    ) -> Result<SecretEntry, SecretError> {
        let tenant = Self::tenant_key(tenant_id);

        // Check cache first
        {
            let secrets = self.secrets.read().await;
            if let Some(tenant_map) = secrets.get(&tenant) {
                if let Some(entry) = tenant_map.get(key_id) {
                    if !self.needs_rotation(entry) {
                        return Ok(entry.clone());
                    }
                }
            }
        }

        // Try backend (tenant-qualified key_id)
        let rotator_key = Self::qualified_key(key_id, &tenant);
        if let Some(entry) = self.rotator.retrieve_key(&rotator_key).await? {
            if self.needs_rotation(&entry) && self.rotation_policy.auto_rotate_on_access {
                return self.rotate_key(key_id, tenant_id).await;
            }
            // Normalize key_id: rotator uses qualified key, caller expects original
            let mut result_entry = entry.clone();
            result_entry.key_id = key_id.to_string();
            self.secrets
                .write()
                .await
                .entry(tenant.clone())
                .or_default()
                .insert(key_id.to_string(), result_entry.clone());
            return Ok(result_entry);
        }

        Err(SecretError::KeyNotFound(format!("{}/{}", tenant, key_id)))
    }

    /// Rotate a key, generating a new key and retaining the old version.
    ///
    /// `tenant_id` is optional; when `None`, operates on the `"_global"` namespace.
    pub async fn rotate_key(
        &self,
        key_id: &str,
        tenant_id: Option<&str>,
    ) -> Result<SecretEntry, SecretError> {
        let tenant = Self::tenant_key(tenant_id);

        let rotator_key = Self::qualified_key(key_id, &tenant);

        let algorithm = {
            let cached = {
                let secrets = self.secrets.read().await;
                secrets.get(&tenant).and_then(|m| m.get(key_id).cloned())
            };
            if let Some(entry) = cached {
                entry.algorithm
            } else if let Ok(Some(entry)) = self.rotator.retrieve_key(&rotator_key).await {
                entry.algorithm
            } else {
                SecretAlgorithm::Generic
            }
        };

        // Archive current key as a previous version
        if let Ok(Some(old_entry)) = self.rotator.retrieve_key(&rotator_key).await {
            let mut versions = self.previous_versions.write().await;
            let tenant_versions = versions.entry(tenant.clone()).or_default();
            let versions_list = tenant_versions.entry(key_id.to_string()).or_default();
            versions_list.push(old_entry);
            // Trim to retain_versions
            while versions_list.len() > self.rotation_policy.retain_versions as usize {
                versions_list.remove(0);
            }
        }

        // Delete old key from backend (tenant-qualified)
        let _ = self.rotator.delete_key(&rotator_key).await;

        // Generate new key (tenant-qualified)
        let new_entry = self.rotator.generate_key(&rotator_key, algorithm).await?;
        // Normalize key_id: rotator uses qualified key, caller expects original
        let mut result_entry = new_entry.clone();
        result_entry.key_id = key_id.to_string();

        // Update cache
        self.secrets
            .write()
            .await
            .entry(tenant.clone())
            .or_default()
            .insert(key_id.to_string(), result_entry.clone());

        info!(key = %key_id, tenant = %tenant, "Key rotated");
        Ok(result_entry)
    }

    /// Get previous versions of a key (for decrypting data encrypted with old keys).
    ///
    /// `tenant_id` is optional; when `None`, looks up versions under the `"_global"` namespace.
    pub async fn get_previous_versions(
        &self,
        key_id: &str,
        tenant_id: Option<&str>,
    ) -> Vec<SecretEntry> {
        let tenant = Self::tenant_key(tenant_id);
        self.previous_versions
            .read()
            .await
            .get(&tenant)
            .and_then(|m| m.get(key_id))
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

    /// Rotate all registered keys that have exceeded their max age.
    /// Returns the number of keys that were rotated.
    pub async fn rotate_all_expired(&self) -> usize {
        let mut count = 0;
        // Collect expired keys first to avoid holding the read lock during rotation
        let expired: Vec<(String, String)> = {
            let secrets = self.secrets.read().await;
            secrets
                .iter()
                .flat_map(|(tenant, tenant_secrets)| {
                    tenant_secrets
                        .iter()
                        .filter(|(_, entry)| self.needs_rotation(entry))
                        .map(move |(key_id, _)| (tenant.clone(), key_id.clone()))
                })
                .collect()
        };
        for (tenant, key_id) in expired {
            match self.rotate_key(&key_id, Some(&tenant)).await {
                Ok(_) => {
                    count += 1;
                    tracing::info!(
                        key = %key_id,
                        tenant = %tenant,
                        "Background rotation: key rotated"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        key = %key_id,
                        tenant = %tenant,
                        error = %e,
                        "Background rotation: failed to rotate key"
                    );
                }
            }
        }
        count
    }
}

/// Securely remove a key from the store by zeroing its bytes before dropping (S-FIX10).
#[allow(dead_code)] // F-GAP reserved
pub fn secure_remove(store: &mut HashMap<String, Vec<u8>>, key: &str) {
    if let Some(value) = store.get_mut(key) {
        for byte in value.iter_mut() {
            unsafe {
                std::ptr::write_volatile(byte, 0u8);
            }
        }
    }
    store.remove(key);
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
        // Use MemoryRotator for tenant isolation tests (EnvRotator has flat namespace)
        let rotator = Arc::new(MemoryRotator::new());
        SecretManager::new(policy, rotator)
    }

    #[tokio::test]
    async fn test_register_and_get_key() {
        let mgr = make_env_manager();
        let entry = mgr
            .register_key("test-key-1".into(), SecretAlgorithm::Generic, None)
            .await
            .unwrap();
        assert_eq!(entry.key_id, "test-key-1");
        assert!(entry.tenant_id.is_none());

        let fetched = mgr.get_key("test-key-1", None).await.unwrap();
        assert_eq!(fetched.key_id, "test-key-1");
        assert_eq!(fetched.key_bytes.len(), 32);
    }

    #[tokio::test]
    async fn test_get_nonexistent_key() {
        let mgr = make_env_manager();
        let err = mgr.get_key("nonexistent", None).await.unwrap_err();
        assert!(matches!(err, SecretError::KeyNotFound(_)));
    }

    #[tokio::test]
    async fn test_rotate_key() {
        let mgr = make_env_manager();
        let original = mgr
            .register_key("rotate-key".into(), SecretAlgorithm::HmacSha256, None)
            .await
            .unwrap();

        let rotated = mgr.rotate_key("rotate-key", None).await.unwrap();
        assert_ne!(original.key_bytes, rotated.key_bytes);
    }

    #[tokio::test]
    async fn test_previous_versions() {
        let mgr = make_env_manager();
        let _first = mgr
            .register_key("ver-key".into(), SecretAlgorithm::Generic, None)
            .await
            .unwrap();
        let _second = mgr.rotate_key("ver-key", None).await.unwrap();
        let _third = mgr.rotate_key("ver-key", None).await.unwrap();

        let versions = mgr.get_previous_versions("ver-key", None).await;
        assert_eq!(versions.len(), 2); // retain_versions = 2
    }

    #[tokio::test]
    async fn test_tenant_isolation() {
        let mgr = make_env_manager();
        // Same key_id in two different tenants should not conflict.
        let _entry_a = mgr
            .register_key(
                "shared-key".into(),
                SecretAlgorithm::Generic,
                Some("tenant-a"),
            )
            .await
            .unwrap();
        let _entry_b = mgr
            .register_key(
                "shared-key".into(),
                SecretAlgorithm::Generic,
                Some("tenant-b"),
            )
            .await
            .unwrap();

        let fetched_a = mgr.get_key("shared-key", Some("tenant-a")).await.unwrap();
        let fetched_b = mgr.get_key("shared-key", Some("tenant-b")).await.unwrap();
        // Both should have different key_bytes since they are separate tenants.
        assert_ne!(fetched_a.key_bytes, fetched_b.key_bytes);
    }
}
