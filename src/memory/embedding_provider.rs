//! Embedding provider abstraction and implementations.
//!
//! Provides a trait for generating text embeddings and several concrete
//! implementations: an OpenAI API-backed provider, a local character-hash
//! fallback (matching the original minhash approach), and a configurable
//! wrapper that selects between them at runtime.

use sha2::{Digest, Sha256};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait for embedding providers that convert text into a float vector.
pub trait EmbeddingProvider: Send + Sync {
    /// Embed `text` into a vector of `dimensions`.
    fn embed(&self, text: &str) -> Vec<f32>;
}

// ---------------------------------------------------------------------------
// Local (character-hash / minhash) provider — identical to the original
// fallback in vector.rs
// ---------------------------------------------------------------------------

const LOCAL_EMBED_MAX_TOKEN_COUNT: usize = 1024;
const LOCAL_MINHASH_NUM_HASHES: usize = 4;

fn local_tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter_map(|token| {
            let t = token.trim().to_ascii_lowercase();
            if t.len() >= 2 {
                Some(t)
            } else {
                None
            }
        })
        .collect()
}

/// Embed text using a minhash-like LSH approach with multiple hash functions.
/// This is the same algorithm as the original `embed_text` fallback in
/// `vector.rs`.
fn local_hash_embed(text: &str, dimensions: usize) -> Vec<f32> {
    let mut vector = vec![0_f32; dimensions];
    if dimensions == 0 {
        return vector;
    }

    static WARNED_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WARNED_ONCE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        warn!(
            "LocalEmbeddingProvider: using minhash fallback embedding —
             no real embedding model configured"
        );
    }

    let tokens: Vec<String> = local_tokenize(text)
        .into_iter()
        .take(LOCAL_EMBED_MAX_TOKEN_COUNT)
        .collect();

    if tokens.is_empty() {
        return vector;
    }

    for token in &tokens {
        for seed in 0..LOCAL_MINHASH_NUM_HASHES {
            let mut hasher = Sha256::new();
            hasher.update(seed.to_le_bytes());
            hasher.update(token.as_bytes());
            let digest = hasher.finalize();

            let mut idx_bytes = [0_u8; 8];
            idx_bytes.copy_from_slice(&digest[0..8]);
            let idx = (u64::from_le_bytes(idx_bytes) as usize) % dimensions;

            let sign = if digest[9] % 2 == 0 { 1.0 } else { -1.0 };
            vector[idx] += sign;
        }
    }

    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }

    vector
}

/// Local embedding provider that uses character-hash (minhash) fallback.
/// Matches the behaviour of the original `embed_text` function in vector.rs.
pub struct LocalEmbeddingProvider {
    /// Dimensionality of the output vectors.
    pub dimensions: usize,
}

impl LocalEmbeddingProvider {
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }
}

impl EmbeddingProvider for LocalEmbeddingProvider {
    fn embed(&self, text: &str) -> Vec<f32> {
        local_hash_embed(text, self.dimensions)
    }
}

// ---------------------------------------------------------------------------
// OpenAI embedding provider — calls the OpenAI Embeddings API
// ---------------------------------------------------------------------------

/// Configuration for the OpenAI embedding provider.
pub struct OpenAiEmbeddingConfig {
    /// API base URL (default: https://api.openai.com/v1).
    pub api_base: String,
    /// Model name (default: text-embedding-3-small).
    pub model: String,
    /// API key.
    pub api_key: String,
    /// Dimensionality of the output vectors.
    pub dimensions: usize,
}

impl Default for OpenAiEmbeddingConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.openai.com/v1".to_string(),
            model: "text-embedding-3-small".to_string(),
            api_key: String::new(),
            dimensions: 1536,
        }
    }
}

/// Embedding provider backed by the OpenAI Embeddings API.
pub struct OpenAiEmbeddingProvider {
    config: OpenAiEmbeddingConfig,
    client: reqwest::blocking::Client,
}

impl OpenAiEmbeddingProvider {
    pub fn new(config: OpenAiEmbeddingConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self { config, client }
    }
}

impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn embed(&self, text: &str) -> Vec<f32> {
        let url = format!("{}/embeddings", self.config.api_base.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.config.model,
            "input": text,
        });

        match self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&body)
            .send()
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!(
                        "OpenAiEmbeddingProvider: API returned {} — falling back to local hash",
                        resp.status()
                    );
                    return local_hash_embed(text, self.config.dimensions);
                }
                match resp.json::<serde_json::Value>() {
                    Ok(json) => {
                        let data = &json["data"];
                        if let Some(embedding) = data[0]["embedding"].as_array() {
                            let vec: Vec<f32> = embedding
                                .iter()
                                .filter_map(|v| v.as_f64().map(|f| f as f32))
                                .collect();
                            if vec.len() == self.config.dimensions {
                                return vec;
                            }
                            warn!(
                                "OpenAiEmbeddingProvider: expected {} dimensions, got {} — falling back to local hash",
                                self.config.dimensions,
                                vec.len()
                            );
                        } else {
                            warn!(
                                "OpenAiEmbeddingProvider: unexpected response shape — falling back to local hash"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "OpenAiEmbeddingProvider: failed to parse response: {} — falling back to local hash",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    "OpenAiEmbeddingProvider: request failed: {} — falling back to local hash",
                    e
                );
            }
        }

        local_hash_embed(text, self.config.dimensions)
    }
}

// ---------------------------------------------------------------------------
// Configurable embedding provider — switches between providers at runtime
// ---------------------------------------------------------------------------

/// Selection of which embedding backend to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingBackend {
    /// Use the local minhash fallback.
    Local,
    /// Use the OpenAI API.
    OpenAi,
}

/// Runtime-configurable embedding provider that dispatches to either a local
/// hash-based provider or an OpenAI API provider based on configuration.
pub struct ConfigurableEmbeddingProvider {
    /// The inner embedding provider.
    inner: Box<dyn EmbeddingProvider>,
    backend: EmbeddingBackend,
    dimensions: usize,
}

impl std::fmt::Debug for ConfigurableEmbeddingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigurableEmbeddingProvider")
            .field("inner", &"<dyn EmbeddingProvider>")
            .field("backend", &self.backend)
            .field("dimensions", &self.dimensions)
            .finish()
    }
}

impl ConfigurableEmbeddingProvider {
    /// Create a new configurable provider.
    ///
    /// When `backend` is `Local`, the provider uses the minhash fallback
    /// which produces vectors of `dimensions` length.
    ///
    /// When `backend` is `OpenAi`, the provider calls the OpenAI API using
    /// the provided config.  If the API is unreachable or returns an error,
    /// it transparently falls back to the local hash provider.
    pub fn new(backend: EmbeddingBackend, openai_config: Option<OpenAiEmbeddingConfig>) -> Self {
        let dimensions = match &backend {
            EmbeddingBackend::Local => 128,
            EmbeddingBackend::OpenAi => {
                openai_config.as_ref().map(|c| c.dimensions).unwrap_or(1536)
            }
        };
        let inner: Box<dyn EmbeddingProvider> = match &backend {
            EmbeddingBackend::Local => {
                info!(
                    "ConfigurableEmbeddingProvider: using local minhash ({} dims)",
                    dimensions
                );
                Box::new(LocalEmbeddingProvider::new(dimensions))
            }
            EmbeddingBackend::OpenAi => {
                let cfg = openai_config.unwrap_or_default();
                info!(
                    "ConfigurableEmbeddingProvider: using OpenAI '{}' ({} dims)",
                    cfg.model, cfg.dimensions
                );
                Box::new(OpenAiEmbeddingProvider::new(cfg))
            }
        };
        Self {
            inner,
            backend,
            dimensions,
        }
    }

    /// Create a local-only provider with the given dimensions.
    pub fn new_local(dimensions: usize) -> Self {
        info!(
            "ConfigurableEmbeddingProvider: using local minhash ({} dims)",
            dimensions
        );
        Self {
            inner: Box::new(LocalEmbeddingProvider::new(dimensions)),
            backend: EmbeddingBackend::Local,
            dimensions,
        }
    }

    /// Returns the dimensionality of this provider's output vectors.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Returns which backend is currently configured.
    pub fn backend(&self) -> &EmbeddingBackend {
        &self.backend
    }
}

impl EmbeddingProvider for ConfigurableEmbeddingProvider {
    fn embed(&self, text: &str) -> Vec<f32> {
        self.inner.embed(text)
    }
}

// ---------------------------------------------------------------------------
// Helper: create a ConfigurableEmbeddingProvider from environment variables
// ---------------------------------------------------------------------------

#[allow(dead_code)]
/// Build a `ConfigurableEmbeddingProvider` based on the env var
/// `GO_ON_EMBEDDING_BACKEND` (values: `local` or `openai`).
///
/// When `openai` is selected, additional env vars are read:
/// - `OPENAI_API_KEY` or `GO_ON_OPENAI_API_KEY`
/// - `OPENAI_EMBEDDING_MODEL`  (default: `text-embedding-3-small`)
/// - `OPENAI_API_BASE`         (default: `https://api.openai.com/v1`)
/// - `OPENAI_EMBEDDING_DIMENSIONS` (default: `1536`)
pub fn embedding_provider_from_env() -> ConfigurableEmbeddingProvider {
    let backend_str = std::env::var("GO_ON_EMBEDDING_BACKEND")
        .unwrap_or_else(|_| "local".to_string())
        .to_lowercase();

    match backend_str.as_str() {
        "openai" => {
            let api_key = std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("GO_ON_OPENAI_API_KEY"))
                .unwrap_or_default();
            let model = std::env::var("OPENAI_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".to_string());
            let api_base = std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            let dims = std::env::var("OPENAI_EMBEDDING_DIMENSIONS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1536);

            let config = OpenAiEmbeddingConfig {
                api_base,
                model,
                api_key,
                dimensions: dims,
            };
            ConfigurableEmbeddingProvider::new(EmbeddingBackend::OpenAi, Some(config))
        }
        _ => {
            let dims = std::env::var("LOCAL_EMBEDDING_DIMENSIONS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(128);
            ConfigurableEmbeddingProvider::new_local(dims)
        }
    }
}
