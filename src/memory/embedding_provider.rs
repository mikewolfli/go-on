//! Embedding provider abstraction and implementations.
//!
//! Provides a trait for generating text embeddings and several concrete
//! implementations: an OpenAI API-backed provider, a local character-hash
//! fallback (matching the original minhash approach), and a configurable
//! wrapper that selects between them at runtime.

use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Trait for embedding providers that convert text into a float vector.
pub trait EmbeddingProvider: Send + Sync {
    /// Embed `text` into a vector of `dimensions`.
    fn embed(&self, text: &str) -> Vec<f32>;

    /// Return the expected dimensionality of this provider's output vectors.
    #[allow(
        dead_code,
        reason = "Public API — trait method reserved for callers who need to validate output dimensionality"
    )]
    fn expected_dimension(&self) -> usize;
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
/// This is the canonical implementation; consumers should import this rather
/// than duplicating the algorithm.
pub fn local_hash_embed(text: &str, dimensions: usize) -> Vec<f32> {
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

    fn expected_dimension(&self) -> usize {
        self.dimensions
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

    /// Check if this provider has an API key configured (not empty).
    pub fn has_api_key(&self) -> bool {
        !self.config.api_key.is_empty() && self.config.api_key != "sk-placeholder"
    }
}

impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn expected_dimension(&self) -> usize {
        self.config.dimensions
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        // If no real API key is configured, use local hash fallback silently
        if !self.has_api_key() {
            debug!("OpenAiEmbeddingProvider: no API key configured, using local hash");
            return local_hash_embed(text, self.config.dimensions);
        }

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
                    error!(
                        "OpenAiEmbeddingProvider: API returned {} — real embedding failed",
                        resp.status()
                    );
                    // Return zero vector to signal failure (not silent hash fallback)
                    return vec![0.0; self.config.dimensions];
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
                            error!(
                                "OpenAiEmbeddingProvider: expected {} dimensions, got {}",
                                self.config.dimensions,
                                vec.len()
                            );
                        } else {
                            error!("OpenAiEmbeddingProvider: unexpected response shape");
                        }
                    }
                    Err(e) => {
                        error!("OpenAiEmbeddingProvider: failed to parse response: {}", e);
                    }
                }
            }
            Err(e) => {
                error!(
                    "OpenAiEmbeddingProvider: HTTP error: {} — returning zero vector",
                    e
                );
            }
        }
        // Return zero vector to signal failure — caller can detect this
        vec![0.0; self.config.dimensions]
    }
}

// ---------------------------------------------------------------------------
// Ollama local embedding provider — calls a local Ollama instance
// ---------------------------------------------------------------------------
// Ollama supports many embedding models locally:
//   ollama pull nomic-embed-text    # 通用embedding (∼0.5GB)
//   ollama pull qwen2.5:7b          # Qwen 2.5 for embedding (∼4.5GB)
//   ollama pull bge-m3              # BGE multilingual (∼2.2GB)
// Endpoint: POST http://localhost:11434/api/embed
// ---------------------------------------------------------------------------

/// Configuration for the Ollama local embedding provider.
pub struct OllamaEmbeddingConfig {
    /// Ollama server URL (default: http://localhost:11434).
    pub base_url: String,
    /// Model name, e.g. "nomic-embed-text", "qwen2.5:7b".
    pub model: String,
    /// Dimensionality of the output vectors.
    pub dimensions: usize,
    /// If true (default), return a local hash embedding on failure.
    /// If false, return a zero vector to signal the failure.
    pub fallback_to_hash: bool,
}

impl Default for OllamaEmbeddingConfig {
    fn default() -> Self {
        Self {
            base_url: std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string()),
            model: std::env::var("OLLAMA_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".to_string()),
            dimensions: std::env::var("OLLAMA_EMBEDDING_DIMENSIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(768),
            fallback_to_hash: true,
        }
    }
}

/// Embedding provider backed by a local Ollama instance.
pub struct OllamaEmbeddingProvider {
    config: OllamaEmbeddingConfig,
    client: reqwest::blocking::Client,
}

impl OllamaEmbeddingProvider {
    pub fn new(config: OllamaEmbeddingConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self { config, client }
    }
}

impl EmbeddingProvider for OllamaEmbeddingProvider {
    fn expected_dimension(&self) -> usize {
        self.config.dimensions
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let url = format!("{}/api/embed", self.config.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.config.model,
            "input": text,
        });

        match self.client.post(&url).json(&body).send() {
            Ok(resp) => {
                if !resp.status().is_success() {
                    error!(
                        "OllamaEmbeddingProvider: server returned {} — is ollama running?",
                        resp.status()
                    );
                    return self.fallback_or_zero(text);
                }
                match resp.json::<serde_json::Value>() {
                    Ok(json) => {
                        // Ollama /api/embed returns: {"model":"...","embeddings":[[...]]}
                        if let Some(embeddings) = json["embeddings"].as_array() {
                            if let Some(embedding) = embeddings.first().and_then(|v| v.as_array()) {
                                let vec: Vec<f32> = embedding
                                    .iter()
                                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                                    .collect();
                                if !vec.is_empty() {
                                    return vec;
                                }
                            }
                        }
                        error!("OllamaEmbeddingProvider: unexpected response: {:?}", json);
                    }
                    Err(e) => {
                        error!("OllamaEmbeddingProvider: failed to parse response: {}", e);
                    }
                }
            }
            Err(e) => {
                error!(
                    "OllamaEmbeddingProvider: cannot connect to {} — {}. Is ollama running?",
                    self.config.base_url, e
                );
            }
        }
        self.fallback_or_zero(text)
    }
}

impl OllamaEmbeddingProvider {
    /// Fallback helper: returns hash embedding if `fallback_to_hash` is true,
    /// otherwise returns a zero vector to signal the embedding failure to callers.
    fn fallback_or_zero(&self, text: &str) -> Vec<f32> {
        if self.config.fallback_to_hash {
            warn!("OllamaEmbeddingProvider: falling back to local hash embedding");
            local_hash_embed(text, self.config.dimensions)
        } else {
            warn!("OllamaEmbeddingProvider: returning zero vector (fallback disabled)");
            vec![0.0f32; self.config.dimensions]
        }
    }
}

// ---------------------------------------------------------------------------
// Qwen3 (DashScope) embedding provider — calls the Alibaba Cloud DashScope API
// ---------------------------------------------------------------------------

/// Configuration for the Qwen3 (DashScope) embedding provider.
pub struct Qwen3EmbeddingConfig {
    /// DashScope API key (from https://dashscope.aliyun.com/).
    pub api_key: String,
    /// Model name, e.g. "text-embedding-v3" (Qwen3 official embedding).
    pub model: String,
    /// Dimensionality of the output vectors (v3 supports 768, 1024, 1536).
    pub dimensions: usize,
    /// If true (default), return a local hash embedding on failure.
    /// If false, return a zero vector to signal the failure.
    pub fallback_to_hash: bool,
}

impl Default for Qwen3EmbeddingConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("DASHSCOPE_API_KEY").unwrap_or_default(),
            model: "text-embedding-v3".to_string(),
            dimensions: 1024,
            fallback_to_hash: true,
        }
    }
}

/// Embedding provider backed by Alibaba Cloud DashScope API (Qwen3).
/// https://help.aliyun.com/zh/model-studio/developer-reference/text-embedding
pub struct Qwen3EmbeddingProvider {
    config: Qwen3EmbeddingConfig,
    client: reqwest::blocking::Client,
}

impl Qwen3EmbeddingProvider {
    pub fn new(config: Qwen3EmbeddingConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self { config, client }
    }

    fn has_api_key(&self) -> bool {
        !self.config.api_key.is_empty() && self.config.api_key != "sk-placeholder"
    }
}

impl EmbeddingProvider for Qwen3EmbeddingProvider {
    fn expected_dimension(&self) -> usize {
        self.config.dimensions
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        if !self.has_api_key() {
            debug!("Qwen3EmbeddingProvider: no DASHSCOPE_API_KEY configured");
            return self.fallback_or_zero(text);
        }

        let url = "https://dashscope.aliyuncs.com/api/v1/services/embeddings/text-embedding/text-embedding";
        let body = serde_json::json!({
            "model": self.config.model,
            "input": {
                "texts": [text]
            },
            "parameters": {
                "dimension": self.config.dimensions
            }
        });

        match self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&body)
            .send()
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!(
                        "Qwen3EmbeddingProvider: DashScope API returned {} — using fallback",
                        resp.status()
                    );
                    return self.fallback_or_zero(text);
                }
                match resp.json::<serde_json::Value>() {
                    Ok(json) => {
                        // DashScope response: output.embeddings[0].embedding
                        if let Some(embedding) = json
                            .pointer("/output/embeddings/0/embedding")
                            .and_then(|v| v.as_array())
                        {
                            let vec: Vec<f32> = embedding
                                .iter()
                                .filter_map(|v| v.as_f64().map(|f| f as f32))
                                .collect();
                            if vec.len() == self.config.dimensions {
                                return vec;
                            }
                            error!(
                                "Qwen3EmbeddingProvider: expected {} dimensions, got {}",
                                self.config.dimensions,
                                vec.len()
                            );
                        } else {
                            error!("Qwen3EmbeddingProvider: unexpected response shape");
                        }
                    }
                    Err(e) => {
                        error!("Qwen3EmbeddingProvider: failed to parse response: {}", e);
                    }
                }
            }
            Err(e) => {
                warn!("Qwen3EmbeddingProvider: HTTP error: {} — using fallback", e);
            }
        }
        self.fallback_or_zero(text)
    }
}

impl Qwen3EmbeddingProvider {
    /// Fallback helper: returns hash embedding if `fallback_to_hash` is true,
    /// otherwise returns a zero vector to signal the embedding failure to callers.
    fn fallback_or_zero(&self, text: &str) -> Vec<f32> {
        if self.config.fallback_to_hash {
            warn!("Qwen3EmbeddingProvider: falling back to local hash embedding");
            local_hash_embed(text, self.config.dimensions)
        } else {
            warn!("Qwen3EmbeddingProvider: returning zero vector (fallback disabled)");
            vec![0.0f32; self.config.dimensions]
        }
    }
}

// ---------------------------------------------------------------------------
// Configurable embedding provider — switches between providers at runtime
// ---------------------------------------------------------------------------

// Selection of which embedding backend to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingBackend {
    /// Use the local minhash fallback.
    Local,
    /// Use the OpenAI API.
    OpenAi,
    /// Use the Qwen3 (DashScope) API.
    Qwen3,
    /// Use a local Ollama instance.
    Ollama,
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
    /// the provided config.
    ///
    /// When `backend` is `Qwen3`, the provider calls the Alibaba Cloud
    /// DashScope API using the Qwen3 `text-embedding-v3` model.
    pub fn new(
        backend: EmbeddingBackend,
        openai_config: Option<OpenAiEmbeddingConfig>,
        qwen3_config: Option<Qwen3EmbeddingConfig>,
        ollama_config: Option<OllamaEmbeddingConfig>,
    ) -> Self {
        let dimensions = match &backend {
            EmbeddingBackend::Local => 128,
            EmbeddingBackend::OpenAi => {
                openai_config.as_ref().map(|c| c.dimensions).unwrap_or(1536)
            }
            EmbeddingBackend::Qwen3 => qwen3_config.as_ref().map(|c| c.dimensions).unwrap_or(1024),
            EmbeddingBackend::Ollama => ollama_config.as_ref().map(|c| c.dimensions).unwrap_or(768),
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
            EmbeddingBackend::Qwen3 => {
                let cfg = qwen3_config.unwrap_or_default();
                info!(
                    "ConfigurableEmbeddingProvider: using Qwen3 '{}' ({} dims) via DashScope",
                    cfg.model, cfg.dimensions
                );
                Box::new(Qwen3EmbeddingProvider::new(cfg))
            }
            EmbeddingBackend::Ollama => {
                let cfg = ollama_config.unwrap_or_default();
                info!(
                    "ConfigurableEmbeddingProvider: using Ollama '{}' ({} dims) at {}",
                    cfg.model, cfg.dimensions, cfg.base_url
                );
                Box::new(OllamaEmbeddingProvider::new(cfg))
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
        let vec = self.inner.embed(text);
        if vec.len() != self.dimensions {
            warn!(
                "Embedding dimension mismatch: got {} but expected {} (backend={:?})",
                vec.len(),
                self.dimensions,
                self.backend,
            );
        }
        vec
    }

    fn expected_dimension(&self) -> usize {
        self.dimensions
    }
}

// ---------------------------------------------------------------------------
// Helper: create a ConfigurableEmbeddingProvider from environment variables
// ---------------------------------------------------------------------------

/// Build a `ConfigurableEmbeddingProvider` based on the env var
/// `GO_ON_EMBEDDING_BACKEND` (values: `local`, `openai`, `qwen3`, `ollama`).
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
            ConfigurableEmbeddingProvider::new(EmbeddingBackend::OpenAi, Some(config), None, None)
        }
        "qwen3" => {
            let api_key = std::env::var("DASHSCOPE_API_KEY").unwrap_or_default();
            let model = std::env::var("QWEN_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-v3".to_string());
            let dims = std::env::var("QWEN_EMBEDDING_DIMENSIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1024);
            let config = Qwen3EmbeddingConfig {
                api_key,
                model,
                dimensions: dims,
                fallback_to_hash: true,
            };
            ConfigurableEmbeddingProvider::new(EmbeddingBackend::Qwen3, None, Some(config), None)
        }
        "ollama" => {
            let config = OllamaEmbeddingConfig::default();
            ConfigurableEmbeddingProvider::new(EmbeddingBackend::Ollama, None, None, Some(config))
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
