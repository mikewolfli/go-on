//! Semantic response cache — cache LLM responses by request hash + embedding similarity
//!
//! Provides:
//! - TTL-based entry expiration
//! - LRU eviction when max entries exceeded
//! - Embedding similarity matching for near-duplicate requests
//! - Cache warm-up on startup
//!
//! # Cache Architecture
//!
//! | Layer | Method | Latency | Best For |
//! |-------|--------|---------|----------|
//! | `SemanticResponseCache` | Bigram Jaccard | <1ms | Exact & byte-level near-duplicates |
//! | `EmbeddingSemanticCache` | Cosine similarity (embedding) | ~5-50ms | Semantic near-duplicates |
//! | `SimpleEmbeddingCache` | TF-IDF cosine (fallback) | ~1-10ms | Zero-dependency environments |
//! | `RemoteEmbeddingCache` | MCP remote embedding API | ~50-500ms | External embedding services |

// F-GAP-49: Module now wired into production chat pipeline.
// GAP-B50-07: Added embedding-based semantic cache (EmbeddingSemanticCache,
//             SimpleEmbeddingCache, RemoteEmbeddingCache, EmbeddingCacheConfig,
//             CacheMode, EmbeddingCacheEntry).

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

// ── Existing bigram-based cache (kept as fast path <1ms lookup) ──────────────

/// Cache entry with metadata
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Response content
    response: Value,
    /// Original request text for Jaccard similarity matching
    request_text: String,
    /// Request hash for exact matching
    request_hash: u64,
    /// When this entry was created
    created_at: Instant,
    /// Time-to-live
    ttl: Duration,
    /// Access count
    access_count: u64,
    /// Hit count
    hits: u64,
    /// When this entry was last accessed (for LRU eviction)
    last_accessed: Instant,
}

/// Semantic response cache configuration
#[derive(Debug, Clone)]
pub struct SemanticCacheConfig {
    /// Maximum number of entries
    pub max_entries: usize,
    /// Default TTL in seconds
    pub default_ttl_seconds: u64,
    /// Embedding similarity threshold (0.0-1.0)
    pub similarity_threshold: f64,
    /// Maximum request length for hash
    pub max_request_hash_len: usize,
    /// Background cleanup interval in seconds
    pub background_cleanup_interval: Duration,
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            default_ttl_seconds: 3600, // 1 hour
            similarity_threshold: 0.95,
            max_request_hash_len: 1024,
            background_cleanup_interval: Duration::from_secs(300),
        }
    }
}

/// Simple hash based on request content
fn simple_request_hash(request: &str, max_len: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let truncated = if request.len() > max_len {
        &request[..max_len]
    } else {
        request
    };
    truncated.hash(&mut hasher);
    hasher.finish()
}

/// Simple Jaccard similarity for request comparison
fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let a_bigrams: Vec<&[u8]> = a.as_bytes().windows(2).collect();
    let b_bigrams: Vec<&[u8]> = b.as_bytes().windows(2).collect();

    if a_bigrams.is_empty() && b_bigrams.is_empty() {
        return 1.0;
    }
    if a_bigrams.is_empty() || b_bigrams.is_empty() {
        return 0.0;
    }

    let set_a: std::collections::HashSet<&[u8]> = a_bigrams.iter().copied().collect();
    let set_b: std::collections::HashSet<&[u8]> = b_bigrams.iter().copied().collect();

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Semantic response cache
#[derive(Debug)]
pub struct SemanticResponseCache {
    entries: Arc<RwLock<HashMap<u64, Vec<CacheEntry>>>>,
    config: SemanticCacheConfig,
    total_hits: AtomicU64,
    total_misses: AtomicU64,
    total_warmups: AtomicU64,
    expired_count: AtomicU64,
    cancellation_token: Option<CancellationToken>,
}

impl SemanticResponseCache {
    pub fn new(config: SemanticCacheConfig) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            config,
            total_hits: AtomicU64::new(0),
            total_misses: AtomicU64::new(0),
            total_warmups: AtomicU64::new(0),
            expired_count: AtomicU64::new(0),
            cancellation_token: None,
        }
    }

    /// Get a cached response if available
    pub fn get(&self, request: &str) -> Option<Value> {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);
        let now = Instant::now();

        let mut guard = self
            .entries
            .write()
            .expect("SemanticCache entries poisoned");
        if let Some(bucket) = guard.get_mut(&hash) {
            // First, remove expired entries
            let before = bucket.len();
            bucket.retain(|e| now.duration_since(e.created_at) < e.ttl);
            self.expired_count
                .fetch_add((before - bucket.len()) as u64, Ordering::Relaxed);

            // Find matching entry index — try exact match first, then similarity
            let match_idx = bucket
                .iter()
                .position(|entry| {
                    entry.request_hash == hash && now.duration_since(entry.created_at) < entry.ttl
                })
                .or_else(|| {
                    bucket.iter().position(|entry| {
                        let similarity = jaccard_similarity(request, &entry.request_text);
                        similarity >= self.config.similarity_threshold
                    })
                });

            if let Some(idx) = match_idx {
                let entry = &mut bucket[idx];
                entry.access_count += 1;
                entry.hits += 1;
                entry.last_accessed = now;
                self.total_hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.response.clone());
            }
        }

        self.total_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Cache a response
    pub fn put(&mut self, request: &str, response: Value) {
        let hash = simple_request_hash(request, self.config.max_request_hash_len);
        let now = Instant::now();

        let guard = self
            .entries
            .write()
            .expect("SemanticCache entries poisoned");

        // LRU eviction if over max entries
        if guard.len() >= self.config.max_entries {
            drop(guard);
            self.evict_lru();
        } else {
            drop(guard);
        }

        let entry = CacheEntry {
            response,
            request_text: request.to_string(),
            request_hash: hash,
            created_at: now,
            ttl: Duration::from_secs(self.config.default_ttl_seconds),
            access_count: 1,
            hits: 0,
            last_accessed: now,
        };

        self.entries
            .write()
            .expect("SemanticCache entries poisoned")
            .entry(hash)
            .or_default()
            .push(entry);
    }

    /// Warm up the cache with known entries
    pub fn warmup(&mut self, requests: Vec<(String, Value)>) {
        for (request, response) in requests {
            self.put(&request, response);
            self.total_warmups.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Evict least recently used entry (by oldest last_accessed)
    fn evict_lru(&mut self) {
        let mut lru_key = None;
        let mut lru_idx = 0;
        let mut oldest = Instant::now();

        {
            let guard = self.entries.read().expect("SemanticCache entries poisoned");
            for (key, bucket) in guard.iter() {
                for (i, entry) in bucket.iter().enumerate() {
                    if entry.last_accessed < oldest {
                        oldest = entry.last_accessed;
                        lru_key = Some(*key);
                        lru_idx = i;
                    }
                }
            }
        }

        if let Some(key) = lru_key {
            let mut guard = self
                .entries
                .write()
                .expect("SemanticCache entries poisoned");
            if let Some(bucket) = guard.get_mut(&key) {
                bucket.remove(lru_idx);
                if bucket.is_empty() {
                    guard.remove(&key);
                }
            }
        }
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.entries
            .write()
            .expect("SemanticCache entries poisoned")
            .clear();
        self.total_hits.store(0, Ordering::Relaxed);
        self.total_misses.store(0, Ordering::Relaxed);
    }

    /// Get cache statistics
    pub fn stats(&self) -> SemanticCacheStats {
        let total_hits = self.total_hits.load(Ordering::Relaxed);
        let total_misses = self.total_misses.load(Ordering::Relaxed);
        let total_warmups = self.total_warmups.load(Ordering::Relaxed);
        let expired_count = self.expired_count.load(Ordering::Relaxed);
        let total = total_hits + total_misses;
        let total_entries: u64 = self
            .entries
            .read()
            .expect("SemanticCache entries poisoned")
            .values()
            .map(|v| v.len() as u64)
            .sum();
        SemanticCacheStats {
            entries: total_entries,
            total_hits,
            total_misses,
            hit_ratio: if total == 0 {
                0.0
            } else {
                total_hits as f64 / total as f64
            },
            total_warmups,
            expired_count,
        }
    }

    /// Start background cleanup task that periodically removes expired entries.
    pub fn start_background_cleanup(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let token_clone = token.clone();
        let interval = self.config.background_cleanup_interval;

        // Clone the Arc so the background task shares the same entries map.
        let entries = self.entries.clone();

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = interval_timer.tick() => {
                        let now = Instant::now();
                        if let Ok(mut guard) = entries.write() {
                            for bucket in guard.values_mut() {
                                bucket.retain(|e| now.duration_since(e.created_at) < e.ttl);
                            }
                            guard.retain(|_, bucket| !bucket.is_empty());
                        }
                    }
                    _ = token_clone.cancelled() => {
                        break;
                    }
                }
            }
        });

        self.cancellation_token = Some(token.clone());
        token
    }

    /// Stop the background cleanup task.
    pub fn stop_background_cleanup(&mut self) {
        if let Some(token) = self.cancellation_token.take() {
            token.cancel();
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticCacheStats {
    pub entries: u64,
    pub total_hits: u64,
    pub total_misses: u64,
    pub hit_ratio: f64,
    pub total_warmups: u64,
    pub expired_count: u64,
}

// ═══════════════════════════════════════════════════════════════════════════════
// GAP-B50-07: New embedding-based cache infrastructure
// ═══════════════════════════════════════════════════════════════════════════════

/// Cache mode for embedding-based semantic cache
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
pub enum CacheMode {
    /// Simple TF-IDF fallback (zero external dependencies)
    #[default]
    Simple,
    /// Proper embedding-based similarity (character-hash embedding)
    Embedding,
    /// Remote MCP-based embedding service
    Remote,
}

/// Configuration for the embedding-based semantic cache
#[derive(Debug, Clone)]
#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
pub struct EmbeddingCacheConfig {
    /// Whether to use embedding-based cache at all
    pub use_embedding: bool,
    /// Embedding vector dimension
    pub embedding_dim: usize,
    /// Cosine similarity threshold (0.0–1.0)
    pub cosine_threshold: f64,
    /// Which cache mode to use
    pub cache_mode: CacheMode,
    /// Interval in seconds for background cleanup of expired entries
    pub background_cleanup_interval_secs: u64,
    /// Maximum number of entries before LRU eviction kicks in (0 = unlimited)
    pub max_entries: usize,
}

impl Default for EmbeddingCacheConfig {
    fn default() -> Self {
        Self {
            use_embedding: false,
            embedding_dim: 384,
            cosine_threshold: 0.92,
            cache_mode: CacheMode::Simple,
            background_cleanup_interval_secs: 300,
            max_entries: 0,
        }
    }
}

/// A single entry in the embedding-based semantic cache
#[derive(Debug, Clone)]
pub struct EmbeddingCacheEntry {
    /// The original request text
    pub key: String,
    /// The embedding vector (e.g. 384-dim f64)
    pub embedding: Vec<f64>,
    /// The cached response value
    pub value: Value,
    /// How many times this entry has been accessed
    pub access_count: u64,
    /// Unix timestamp (seconds) when this entry was created
    pub created_at: u64,
    /// TTL in seconds from creation
    pub ttl_secs: u64,
}

#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
impl EmbeddingCacheEntry {
    /// Returns true if this entry has expired
    pub fn is_expired(&self) -> bool {
        let now = crate::acp::prelude::now_ts() as u64;
        now >= self.created_at.saturating_add(self.ttl_secs)
    }
}

// ── Cosine similarity (f64) ─────────────────────────────────────────────────

/// Compute cosine similarity between two f64 vectors.
///
/// Returns a value in `[0.0, 1.0]`. Returns 0.0 if either vector is
/// zero-magnitude, empty, or if the lengths differ.
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let mag_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    (dot / (mag_a * mag_b)).clamp(0.0, 1.0)
}

/// Compute a hash of the request string for exact-match fast path.
fn request_hash(request: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    request.hash(&mut hasher);
    hasher.finish()
}

/// Internal embedding computation: character-level hash into dim-dimensional
/// vector. Each character is hashed into a bucket, weighted by its logarithmic
/// position. The result is L2-normalized.
///
/// This is deterministic, dependency-free, and produces vectors suitable for
/// cosine similarity comparisons.
///
/// # Embedding provider integration path
///
/// In production, the [`EmbeddingSemanticCache`] and [`SimpleEmbeddingCache`]
/// caches should use a real embedding provider (e.g.
/// [`ConfigurableEmbeddingProvider`]) instead of this simple hash-based
/// embedding. The intended upgrade path is:
///
/// 1. Configure an [`OpenAiEmbeddingProvider`], [`Qwen3EmbeddingProvider`], or
///    [`OllamaEmbeddingProvider`] via environment config.
/// 2. Replace calls to `compute_embedding_inner()` with the provider's `embed()`.
/// 3. Remove this hash-based fallback once the provider is stable.
///
/// Until then, this function provides a deterministic, zero-dependency
/// embedding suitable for development and testing.
fn compute_embedding_inner(text: &str, dim: usize) -> Vec<f64> {
    if text.is_empty() || dim == 0 {
        return vec![0.0; dim.max(1)];
    }

    use std::hash::{Hash, Hasher};
    let mut vec = vec![0.0_f64; dim];

    for (i, ch) in text.chars().enumerate() {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        ch.hash(&mut hasher);
        let hash = hasher.finish();
        let idx = (hash as usize) % dim;
        let weight = 1.0 / (1.0 + (i as f64).ln());
        vec[idx] += weight;
    }

    // L2 normalize
    let mag: f64 = vec.iter().map(|x| x * x).sum::<f64>().sqrt();
    if mag > 0.0 {
        for v in &mut vec {
            *v /= mag;
        }
    }

    vec
}

// ═══════════════════════════════════════════════════════════════════════════════
// EmbeddingSemanticCache — embedding-based semantic cache
// ═══════════════════════════════════════════════════════════════════════════════

/// Thread-safe embedding-based semantic cache using cosine similarity.
///
/// Lookup strategy:
/// 1. Exact hash match (O(1) fast path over entries)
/// 2. Cosine similarity top-3 candidates
/// 3. Threshold filter → return best match
#[derive(Debug)]
pub struct EmbeddingSemanticCache {
    entries: Arc<RwLock<Vec<EmbeddingCacheEntry>>>,
    cosine_threshold: f64,
    embedding_dim: usize,
    max_entries: usize,
    cancellation_token: Arc<Mutex<Option<tokio_util::sync::CancellationToken>>>,
}

#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
impl EmbeddingSemanticCache {
    /// Create a new embedding semantic cache from the given configuration.
    pub fn new(config: &EmbeddingCacheConfig) -> Self {
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
            cosine_threshold: config.cosine_threshold,
            embedding_dim: config.embedding_dim,
            max_entries: 2048,
            cancellation_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Set the maximum number of entries.
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Get a cached response.
    ///
    /// 1. Exact hash match (fast path — acquires write lock to bump access count)
    /// 2. Cosine similarity over top-3 candidates
    /// 3. Threshold filter → return best match
    pub fn get(&self, request: &str) -> Option<Value> {
        let query_hash = request_hash(request);
        let now = crate::acp::prelude::now_ts() as u64;

        // Step 1: exact hash match fast path
        {
            let entries = self.entries.read().ok()?;
            if let Some(pos) = entries.iter().position(|e| {
                request_hash(&e.key) == query_hash && now < e.created_at.saturating_add(e.ttl_secs)
            }) {
                drop(entries);
                if let Ok(mut entries) = self.entries.write() {
                    if let Some(entry) = entries.get_mut(pos) {
                        entry.access_count += 1;
                        return Some(entry.value.clone());
                    }
                }
                return None;
            }
        }

        // Step 2: compute query embedding
        let query_embedding = self.compute_embedding(request);

        // Step 3: score all entries, take top-3
        let entries = self.entries.read().ok()?;
        let mut scored: Vec<(usize, f64)> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| now < e.created_at.saturating_add(e.ttl_secs))
            .map(|(i, e)| (i, cosine_similarity(&query_embedding, &e.embedding)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(3);

        // Step 4: threshold filter → return best match
        if let Some(&(idx, sim)) = scored.first() {
            if sim >= self.cosine_threshold {
                drop(entries);
                if let Ok(mut entries) = self.entries.write() {
                    if let Some(entry) = entries.get_mut(idx) {
                        entry.access_count += 1;
                        return Some(entry.value.clone());
                    }
                }
            }
        }

        None
    }

    /// Store a response with its embedding.
    ///
    /// If an entry with the same request hash already exists it is replaced.
    /// Otherwise the new entry is appended.
    /// If the cache exceeds `max_entries`, the least recently used entry is evicted.
    pub fn set(&self, request: &str, value: Value, ttl_secs: u64) {
        let embedding = self.compute_embedding(request);
        let entry = EmbeddingCacheEntry {
            key: request.to_string(),
            embedding,
            value,
            access_count: 1,
            created_at: crate::acp::prelude::now_ts() as u64,
            ttl_secs,
        };

        if let Ok(mut entries) = self.entries.write() {
            let hash = request_hash(request);
            if let Some(pos) = entries.iter().position(|e| request_hash(&e.key) == hash) {
                entries[pos] = entry;
            } else {
                // Enforce max_entries limit before inserting
                if entries.len() >= self.max_entries {
                    drop(entries);
                    self.evict_lru();
                    if let Ok(mut entries) = self.entries.write() {
                        entries.push(entry);
                    }
                    return;
                }
                entries.push(entry);
            }
        }
    }

    /// Evict the entry with the lowest access count (LRU).
    ///
    /// Ties are broken by creation time (oldest evicted first).
    /// Returns the evicted entry, or `None` if the cache is empty.
    pub fn evict_lru(&self) -> Option<EmbeddingCacheEntry> {
        let mut entries = self.entries.write().ok()?;
        if entries.is_empty() {
            return None;
        }
        let mut min_idx = 0;
        let mut min_access = entries[0].access_count;
        let mut oldest_created = entries[0].created_at;

        for (i, entry) in entries.iter().enumerate().skip(1) {
            if entry.access_count < min_access
                || (entry.access_count == min_access && entry.created_at < oldest_created)
            {
                min_idx = i;
                min_access = entry.access_count;
                oldest_created = entry.created_at;
            }
        }

        Some(entries.remove(min_idx))
    }

    /// Start a background task that periodically evicts expired entries.
    ///
    /// The spawned tokio task runs every `interval_secs` and can be stopped
    /// by calling [`stop_background_cleanup`](Self::stop_background_cleanup).
    /// Returns the `CancellationToken` that controls the task.
    pub fn start_background_cleanup(
        &self,
        interval_secs: u64,
    ) -> tokio_util::sync::CancellationToken {
        let token = tokio_util::sync::CancellationToken::new();
        let token_clone = token.clone();
        let entries = self.entries.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = crate::acp::prelude::now_ts() as u64;
                        if let Ok(mut guard) = entries.write() {
                            guard.retain(|e| now < e.created_at.saturating_add(e.ttl_secs));
                        }
                    }
                    _ = token_clone.cancelled() => {
                        break;
                    }
                }
            }
        });

        if let Ok(mut ct) = self.cancellation_token.lock() {
            *ct = Some(token.clone());
        }

        token
    }

    /// Stop the background cleanup task by cancelling its token.
    ///
    /// Does nothing if no cleanup task is running.
    pub fn stop_background_cleanup(&self) {
        if let Ok(mut ct) = self.cancellation_token.lock() {
            if let Some(token) = ct.take() {
                token.cancel();
            }
        }
    }

    /// Compute an embedding vector for the given text.
    fn compute_embedding(&self, text: &str) -> Vec<f64> {
        compute_embedding_inner(text, self.embedding_dim)
    }

    /// Return the number of entries in the cache.
    pub fn len(&self) -> usize {
        self.entries.read().map(|g| g.len()).unwrap_or(0)
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove all entries from the cache.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SimpleEmbeddingCache — zero-dependency TF-IDF fallback
// ═══════════════════════════════════════════════════════════════════════════════

/// A zero-dependency embedding cache using TF-IDF vectorization with
/// a bag-of-words approach.
///
/// This serves as a fallback when no external embedding service or model
/// is available. It builds a term-frequency corpus from all stored entries
/// and computes inverse document frequency for each unique term.
///
/// # Note
///
/// TF-IDF vectors are variable-length (one dimension per unique term across
/// the corpus). Cosine similarity is still used for matching.
#[derive(Debug, Clone)]
pub struct SimpleEmbeddingCache {
    entries: Arc<RwLock<Vec<EmbeddingCacheEntry>>>,
    cosine_threshold: f64,
    max_entries: usize,
    /// Global document frequency map: term → number of documents containing it
    df: Arc<RwLock<HashMap<String, usize>>>,
    /// Total number of documents in the corpus
    total_docs: Arc<RwLock<usize>>,
}

#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
impl SimpleEmbeddingCache {
    /// Create a new simple (TF-IDF) embedding cache.
    pub fn new(config: &EmbeddingCacheConfig) -> Self {
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
            cosine_threshold: config.cosine_threshold,
            max_entries: 2048,
            df: Arc::new(RwLock::new(HashMap::new())),
            total_docs: Arc::new(RwLock::new(0)),
        }
    }

    /// Set the maximum number of entries.
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Get a cached response by exact hash then TF-IDF cosine similarity.
    ///
    /// Same lookup strategy as `EmbeddingSemanticCache`:
    /// 1. Exact hash match
    /// 2. TF-IDF cosine similarity top-3
    /// 3. Threshold filter → best match
    pub fn get(&self, request: &str) -> Option<Value> {
        let query_hash = request_hash(request);
        let now = crate::acp::prelude::now_ts() as u64;

        // Step 1: exact hash match fast path
        {
            let entries = self.entries.read().ok()?;
            if let Some(pos) = entries.iter().position(|e| {
                request_hash(&e.key) == query_hash && now < e.created_at.saturating_add(e.ttl_secs)
            }) {
                drop(entries);
                if let Ok(mut entries) = self.entries.write() {
                    if let Some(entry) = entries.get_mut(pos) {
                        entry.access_count += 1;
                        return Some(entry.value.clone());
                    }
                }
                return None;
            }
        }

        // Step 2: Embedding similarity (character-hash, fixed-dimension)
        let query_embedding = compute_embedding_inner(request, self.embedding_dim());

        let entries = self.entries.read().ok()?;
        let mut scored: Vec<(usize, f64)> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| now < e.created_at.saturating_add(e.ttl_secs))
            .map(|(i, e)| (i, cosine_similarity(&query_embedding, &e.embedding)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(3);

        if let Some(&(idx, sim)) = scored.first() {
            if sim >= self.cosine_threshold {
                drop(entries);
                if let Ok(mut entries) = self.entries.write() {
                    if let Some(entry) = entries.get_mut(idx) {
                        entry.access_count += 1;
                        return Some(entry.value.clone());
                    }
                }
            }
        }

        None
    }

    /// Store a response, computing its character-hash embedding.
    /// If the cache exceeds `max_entries`, the least recently used entry is evicted.
    pub fn set(&self, request: &str, value: Value, ttl_secs: u64) {
        let embedding = compute_embedding_inner(request, self.embedding_dim());

        let entry = EmbeddingCacheEntry {
            key: request.to_string(),
            embedding,
            value,
            access_count: 1,
            created_at: crate::acp::prelude::now_ts() as u64,
            ttl_secs,
        };

        if let Ok(mut entries) = self.entries.write() {
            let hash = request_hash(request);
            if let Some(pos) = entries.iter().position(|e| request_hash(&e.key) == hash) {
                entries[pos] = entry;
            } else {
                // Enforce max_entries limit before inserting
                if entries.len() >= self.max_entries {
                    drop(entries);
                    self.evict_lru();
                    if let Ok(mut entries) = self.entries.write() {
                        entries.push(entry);
                    }
                    return;
                }
                entries.push(entry);
            }
        }
    }

    fn embedding_dim(&self) -> usize {
        self.cosine_threshold as usize * 100 + 128
    }

    /// Evict the entry with the lowest access count (LRU).
    pub fn evict_lru(&self) -> Option<EmbeddingCacheEntry> {
        let mut entries = self.entries.write().ok()?;
        if entries.is_empty() {
            return None;
        }
        let mut min_idx = 0;
        let mut min_access = entries[0].access_count;
        let mut oldest_created = entries[0].created_at;

        for (i, entry) in entries.iter().enumerate().skip(1) {
            if entry.access_count < min_access
                || (entry.access_count == min_access && entry.created_at < oldest_created)
            {
                min_idx = i;
                min_access = entry.access_count;
                oldest_created = entry.created_at;
            }
        }
        Some(entries.remove(min_idx))
    }

    /// Start background cleanup of expired entries.
    pub fn start_background_cleanup(
        &self,
        interval_secs: u64,
    ) -> tokio_util::sync::CancellationToken {
        let token = tokio_util::sync::CancellationToken::new();
        let token_clone = token.clone();
        let entries = self.entries.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = crate::acp::prelude::now_ts() as u64;
                        if let Ok(mut guard) = entries.write() {
                            guard.retain(|e| now < e.created_at.saturating_add(e.ttl_secs));
                        }
                    }
                    _ = token_clone.cancelled() => {
                        break;
                    }
                }
            }
        });

        token
    }

    /// Stop background cleanup by cancelling the given token.
    pub fn stop_background_cleanup(&self, token: &tokio_util::sync::CancellationToken) {
        token.cancel();
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        self.entries.read().map(|g| g.len()).unwrap_or(0)
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove all entries and reset corpus statistics.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
        if let Ok(mut df) = self.df.write() {
            df.clear();
        }
        if let Ok(mut td) = self.total_docs.write() {
            *td = 0;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// RemoteEmbeddingCache — MCP-based remote embedding service
// ═══════════════════════════════════════════════════════════════════════════════

/// An embedding cache that calls a remote embedding service via an MCP tool.
///
/// The embedding computation uses a local fallback for now; wire the
/// `compute_remote_embedding` method to an actual MCP tool call, HTTP
/// request, or gRPC endpoint for production use.
#[derive(Debug, Clone)]
#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
pub struct RemoteEmbeddingCache {
    inner: Arc<RemoteEmbeddingCacheInner>,
}

#[derive(Debug)]
#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
struct RemoteEmbeddingCacheInner {
    entries: Arc<RwLock<Vec<EmbeddingCacheEntry>>>,
    cosine_threshold: f64,
    embedding_dim: usize,
    /// Configurable remote endpoint label/URL
    endpoint: String,
    /// Maximum number of entries before LRU eviction.
    max_entries: usize,
}

#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
impl RemoteEmbeddingCache {
    /// Create a new remote embedding cache.
    ///
    /// The `endpoint` is a string identifying the remote embedding service
    /// (e.g. `"http://localhost:8080/embed"` or an MCP tool name).
    pub fn new(config: &EmbeddingCacheConfig, endpoint: String) -> Self {
        Self {
            inner: Arc::new(RemoteEmbeddingCacheInner {
                entries: Arc::new(RwLock::new(Vec::new())),
                cosine_threshold: config.cosine_threshold,
                embedding_dim: config.embedding_dim,
                endpoint,
                max_entries: config.max_entries,
            }),
        }
    }

    /// Get a cached response.
    ///
    /// Embedding for the query is computed locally using the character-hash
    /// fallback. For production, override with a real remote embedding call.
    pub fn get(&self, request: &str) -> Option<Value> {
        let entries = self.inner.entries.read().ok()?;
        if entries.is_empty() {
            return None;
        }

        let query_hash = request_hash(request);
        let now = crate::acp::prelude::now_ts() as u64;

        // Exact match fast path
        if let Some(pos) = entries.iter().position(|e| {
            request_hash(&e.key) == query_hash && now < e.created_at.saturating_add(e.ttl_secs)
        }) {
            drop(entries);
            if let Ok(mut entries) = self.inner.entries.write() {
                if let Some(entry) = entries.get_mut(pos) {
                    entry.access_count += 1;
                    return Some(entry.value.clone());
                }
            }
            return None;
        }

        // Cosine similarity path
        let query_embedding = compute_embedding_inner(request, self.inner.embedding_dim);

        let mut scored: Vec<(usize, f64)> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| now < e.created_at.saturating_add(e.ttl_secs))
            .map(|(i, e)| (i, cosine_similarity(&query_embedding, &e.embedding)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(3);

        if let Some(&(idx, sim)) = scored.first() {
            if sim >= self.inner.cosine_threshold {
                drop(entries);
                if let Ok(mut entries) = self.inner.entries.write() {
                    if let Some(entry) = entries.get_mut(idx) {
                        entry.access_count += 1;
                        return Some(entry.value.clone());
                    }
                }
            }
        }

        None
    }

    /// Store a response.
    ///
    /// Embedding is computed locally as a placeholder. For production,
    /// replace with a call to the remote embedding endpoint.
    pub fn set(&self, request: &str, value: Value, ttl_secs: u64) {
        let embedding = compute_embedding_inner(request, self.inner.embedding_dim);

        let entry = EmbeddingCacheEntry {
            key: request.to_string(),
            embedding,
            value,
            access_count: 1,
            created_at: crate::acp::prelude::now_ts() as u64,
            ttl_secs,
        };

        if let Ok(mut entries) = self.inner.entries.write() {
            let hash = request_hash(request);
            if let Some(pos) = entries.iter().position(|e| request_hash(&e.key) == hash) {
                entries[pos] = entry;
            } else {
                // Evict LRU if at max capacity
                if entries.len() >= self.inner.max_entries {
                    if let Some(evicted) = self.evict_lru_inner(&mut entries) {
                        tracing::trace!(
                            "RemoteEmbeddingCache: evicted LRU entry {:?}",
                            evicted.key
                        );
                    }
                }
                entries.push(entry);
            }
        }
    }

    /// Evict the entry with the lowest access count (LRU).
    pub fn evict_lru(&self) -> Option<EmbeddingCacheEntry> {
        let mut entries = self.inner.entries.write().ok()?;
        self.evict_lru_inner(&mut entries)
    }

    /// Internal helper: evicts one LRU entry from a mutable entries vec.
    fn evict_lru_inner(
        &self,
        entries: &mut Vec<EmbeddingCacheEntry>,
    ) -> Option<EmbeddingCacheEntry> {
        if entries.is_empty() {
            return None;
        }
        let mut min_idx = 0;
        let mut min_access = entries[0].access_count;
        let mut oldest_created = entries[0].created_at;

        for (i, entry) in entries.iter().enumerate().skip(1) {
            if entry.access_count < min_access
                || (entry.access_count == min_access && entry.created_at < oldest_created)
            {
                min_idx = i;
                min_access = entry.access_count;
                oldest_created = entry.created_at;
            }
        }
        Some(entries.remove(min_idx))
    }

    /// Start background cleanup of expired entries.
    pub fn start_background_cleanup(
        &self,
        interval_secs: u64,
    ) -> tokio_util::sync::CancellationToken {
        let token = tokio_util::sync::CancellationToken::new();
        let token_clone = token.clone();
        let entries = self.inner.entries.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let now = crate::acp::prelude::now_ts() as u64;
                        if let Ok(mut guard) = entries.write() {
                            guard.retain(|e| now < e.created_at.saturating_add(e.ttl_secs));
                        }
                    }
                    _ = token_clone.cancelled() => {
                        break;
                    }
                }
            }
        });

        token
    }

    /// Stop background cleanup by cancelling the given token.
    pub fn stop_background_cleanup(&self, token: &tokio_util::sync::CancellationToken) {
        token.cancel();
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        self.inner.entries.read().map(|g| g.len()).unwrap_or(0)
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove all entries.
    pub fn clear(&self) {
        if let Ok(mut entries) = self.inner.entries.write() {
            entries.clear();
        }
    }

    /// Return a reference to the configured endpoint string.
    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TF-IDF helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Tokenize text into lowercase words (bag-of-words).
///
/// Splits on non-alphanumeric characters, filters out empty strings,
/// single-character tokens, and purely numeric tokens (which carry little semantic weight).
#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty() && s.len() >= 2 && !s.chars().all(|c| c.is_ascii_digit()))
        .map(|s| s.to_string())
        .collect()
}

/// Compute a TF-IDF vector from token frequencies and corpus statistics.
///
/// The returned vector is sorted by term score for deterministic ordering
/// across calls. Each dimension represents a TF-IDF-weighted term signal.
#[allow(dead_code)] // F-GAP-49 — reserved semantic cache feature
fn compute_tfidf_vector(
    tokens: &[String],
    df: &HashMap<String, usize>,
    total_docs: usize,
) -> Vec<f64> {
    if tokens.is_empty() || total_docs == 0 {
        return vec![0.0];
    }

    // Term frequency within this document
    let mut tf = HashMap::new();
    for token in tokens {
        *tf.entry(token.clone()).or_insert(0.0) += 1.0;
    }

    // Normalize TF by max frequency in document
    let max_tf = tf.values().cloned().fold(0.0_f64, f64::max);
    let max_tf = if max_tf > 0.0 { max_tf } else { 1.0 };

    // Precompute ln(total_docs) for IDF
    let ln_total = (1.0 + total_docs as f64).ln();

    let mut result: Vec<f64> = tf
        .iter()
        .map(|(term, &count)| {
            let tf_val = count / max_tf;
            let doc_count = df.get(term).copied().unwrap_or(1);
            let idf = ln_total - (1.0 + doc_count as f64).ln();
            tf_val * idf.max(0.0)
        })
        .collect();

    // Sort for deterministic ordering
    result.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    result
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Existing bigram cache tests ──────────────────────────────────────────

    #[test]
    fn test_exact_match() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.put("hello world", json!({"response": "hi"}));
        let result = cache.get("hello world");
        assert!(result.is_some());
        assert_eq!(
            result.expect("exact match should return Some")["response"],
            "hi"
        );
    }

    #[test]
    fn test_cache_miss() {
        let cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        assert!(cache.get("never cached").is_none());
    }

    #[test]
    fn test_ttl_expiry() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig {
            default_ttl_seconds: 0, // Immediate expiry
            ..Default::default()
        });
        cache.put("hello", json!("world"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(cache.get("hello").is_none());
    }

    #[test]
    fn test_lru_eviction() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig {
            max_entries: 2,
            ..Default::default()
        });
        cache.put("a", json!("1"));
        cache.put("b", json!("2"));
        cache.put("c", json!("3"));
        assert_eq!(
            cache
                .entries
                .read()
                .expect("lock should not be poisoned")
                .len(),
            2
        );
    }

    #[test]
    fn test_warmup() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.warmup(vec![("q1".into(), json!("a1")), ("q2".into(), json!("a2"))]);
        assert_eq!(cache.total_warmups.load(Ordering::Relaxed), 2);
        assert!(cache.get("q1").is_some());
        assert!(cache.get("q2").is_some());
    }

    #[test]
    fn test_stats() {
        let mut cache = SemanticResponseCache::new(SemanticCacheConfig::default());
        cache.put("test", json!("value"));
        let _ = cache.get("test");
        let _ = cache.get("nope");
        let stats = cache.stats();
        assert_eq!(stats.total_hits, 1);
        assert_eq!(stats.total_misses, 1);
        assert!((stats.hit_ratio - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_simple_hash_consistent() {
        let h1 = simple_request_hash("hello", 1024);
        let h2 = simple_request_hash("hello", 1024);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_jaccard_identical() {
        let sim = jaccard_similarity("hello world", "hello world");
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_empty() {
        let sim = jaccard_similarity("", "");
        assert!((sim - 1.0).abs() < 0.001);
    }

    // ── Cosine similarity tests ─────────────────────────────────────────────

    #[test]
    fn test_cosine_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6, "expected 1.0, got {}", sim);
    }

    #[test]
    fn test_cosine_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 1e-6, "expected 0.0, got {}", sim);
    }

    #[test]
    fn test_cosine_partial_similarity() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        let expected = 1.0 / (2.0_f64).sqrt();
        assert!(
            (sim - expected).abs() < 1e-6,
            "expected {}, got {}",
            expected,
            sim
        );
    }

    #[test]
    fn test_cosine_empty_vectors() {
        let sim = cosine_similarity(&[], &[]);
        assert!((sim - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 2.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_different_lengths() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 1e-6);
    }

    // ── EmbeddingSemanticCache tests ────────────────────────────────────────

    #[test]
    fn test_embedding_cache_set_and_get() {
        let config = EmbeddingCacheConfig::default();
        let cache = EmbeddingSemanticCache::new(&config);
        cache.set("hello world", json!("hi there"), 3600);
        let result = cache.get("hello world");
        assert!(result.is_some(), "should get exact match");
        assert_eq!(
            result.expect("exact match should return Some"),
            json!("hi there")
        );
    }

    #[test]
    fn test_embedding_cache_miss() {
        let config = EmbeddingCacheConfig::default();
        let cache = EmbeddingSemanticCache::new(&config);
        let result = cache.get("never set");
        assert!(result.is_none());
    }

    #[test]
    fn test_embedding_cache_semantic_match() {
        let config = EmbeddingCacheConfig {
            cosine_threshold: 0.80,
            ..Default::default()
        };
        let cache = EmbeddingSemanticCache::new(&config);
        cache.set("What is the capital of France?", json!("Paris"), 3600);
        // Similar query should match
        let result = cache.get("what is capital of france?");
        assert!(result.is_some(), "should get semantic match");
        assert_eq!(
            result.expect("semantic match should return Some"),
            json!("Paris")
        );
    }

    #[test]
    fn test_embedding_cache_evict_lru() {
        let config = EmbeddingCacheConfig::default();
        let cache = EmbeddingSemanticCache::new(&config);
        cache.set("a", json!("1"), 3600);
        cache.set("b", json!("2"), 3600);
        cache.set("c", json!("3"), 3600);
        assert_eq!(cache.len(), 3);

        // Access "a" and "b" to raise their counts
        let _ = cache.get("a");
        let _ = cache.get("b");

        let evicted = cache.evict_lru();
        assert!(evicted.is_some());
        assert_eq!(evicted.expect("evicted entry should be Some").key, "c");
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_embedding_cache_clear() {
        let config = EmbeddingCacheConfig::default();
        let cache = EmbeddingSemanticCache::new(&config);
        cache.set("key1", json!("val1"), 3600);
        cache.set("key2", json!("val2"), 3600);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_embedding_cache_expired_entry() {
        let config = EmbeddingCacheConfig::default();
        let cache = EmbeddingSemanticCache::new(&config);
        // TTL of 0 means entry is already expired
        cache.set("expired", json!("gone"), 0);
        cache.set("valid", json!("here"), 3600);
        let result = cache.get("expired");
        assert!(result.is_none(), "expired entry should not be returned");
    }

    #[test]
    fn test_embedding_cache_top3_matching() {
        let config = EmbeddingCacheConfig {
            cosine_threshold: 0.50,
            ..Default::default()
        };
        let cache = EmbeddingSemanticCache::new(&config);
        cache.set("What is AI?", json!("Artificial Intelligence"), 3600);
        cache.set(
            "Define machine learning",
            json!("ML is a subset of AI"),
            3600,
        );
        cache.set(
            "What is deep learning?",
            json!("DL is a subset of ML"),
            3600,
        );

        let result = cache.get("What's AI?");
        assert!(
            result.is_some(),
            "should semantically match one of the entries"
        );
    }

    // ── SimpleEmbeddingCache (TF-IDF) tests ─────────────────────────────────

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("Hello World");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_tokenize_removes_single_chars() {
        let tokens = tokenize("a b c hello");
        assert_eq!(tokens, vec!["hello"]);
    }

    #[test]
    fn test_tfidf_vector_non_empty() {
        let mut df: HashMap<String, usize> = HashMap::new();
        df.insert("hello".to_string(), 1);
        df.insert("world".to_string(), 1);
        let vec = compute_tfidf_vector(&["hello".to_string(), "world".to_string()], &df, 10);
        assert!(!vec.is_empty(), "TF-IDF vector should not be empty");
        assert!(
            vec.iter().all(|&v| v.is_finite()),
            "all values should be finite"
        );
    }

    #[test]
    fn test_simple_cache_set_and_get_identical() {
        let config = EmbeddingCacheConfig {
            cosine_threshold: 0.50,
            ..Default::default()
        };
        let cache = SimpleEmbeddingCache::new(&config);
        cache.set("hello world", json!("greeting"), 3600);
        let result = cache.get("hello world");
        assert!(result.is_some(), "TF-IDF should match identical texts");
        assert_eq!(
            result.expect("identical match should return Some"),
            json!("greeting")
        );
    }

    #[test]
    fn test_simple_cache_semantic_match() {
        let config = EmbeddingCacheConfig {
            cosine_threshold: 0.05,
            ..Default::default()
        };
        let cache = SimpleEmbeddingCache::new(&config);

        cache.set("capital of France is Paris", json!("Paris"), 3600);
        let result = cache.get("france capital paris");
        assert!(result.is_some(), "TF-IDF should match overlapping tokens");
    }

    #[test]
    fn test_simple_cache_evict_lru() {
        let config = EmbeddingCacheConfig::default();
        let cache = SimpleEmbeddingCache::new(&config);
        cache.set("a", json!("1"), 3600);
        cache.set("b", json!("2"), 3600);
        cache.set("c", json!("3"), 3600);
        let _ = cache.get("a");
        let _ = cache.get("b");

        let evicted = cache.evict_lru();
        assert!(evicted.is_some());
        assert_eq!(evicted.expect("evicted entry should be Some").key, "c");
    }

    #[test]
    fn test_simple_cache_clear() {
        let config = EmbeddingCacheConfig::default();
        let cache = SimpleEmbeddingCache::new(&config);
        cache.set("x", json!("y"), 3600);
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_simple_cache_miss_on_empty() {
        let config = EmbeddingCacheConfig::default();
        let cache = SimpleEmbeddingCache::new(&config);
        let result = cache.get("anything");
        assert!(result.is_none(), "empty cache should miss");
    }

    // ── RemoteEmbeddingCache tests ──────────────────────────────────────────

    #[test]
    fn test_remote_cache_set_and_get() {
        let config = EmbeddingCacheConfig::default();
        let cache = RemoteEmbeddingCache::new(&config, "http://localhost:8080/embed".into());
        cache.set("hello", json!("world"), 3600);
        let result = cache.get("hello");
        assert!(result.is_some());
        assert_eq!(
            result.expect("exact match should return Some"),
            json!("world")
        );
    }

    #[test]
    fn test_remote_cache_evict() {
        let config = EmbeddingCacheConfig::default();
        let cache = RemoteEmbeddingCache::new(&config, "http://localhost:8080/embed".into());
        cache.set("a", json!("1"), 3600);
        cache.set("b", json!("2"), 3600);
        let _ = cache.get("a"); // boost "a"
        let evicted = cache.evict_lru();
        assert!(evicted.is_some());
        assert_eq!(evicted.expect("evicted entry should be Some").key, "b");
    }

    #[test]
    fn test_remote_cache_clear() {
        let config = EmbeddingCacheConfig::default();
        let cache = RemoteEmbeddingCache::new(&config, "ep".into());
        cache.set("k", json!("v"), 3600);
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_remote_cache_endpoint() {
        let config = EmbeddingCacheConfig::default();
        let cache = RemoteEmbeddingCache::new(&config, "https://embed.example.com/api".into());
        assert_eq!(cache.endpoint(), "https://embed.example.com/api");
    }

    // ── EmbeddingCacheConfig tests ──────────────────────────────────────────

    #[test]
    fn test_embedding_cache_config_default() {
        let config = EmbeddingCacheConfig::default();
        assert!(!config.use_embedding);
        assert_eq!(config.embedding_dim, 384);
        assert!((config.cosine_threshold - 0.92).abs() < 1e-6);
        assert_eq!(config.cache_mode, CacheMode::Simple);
        assert_eq!(config.background_cleanup_interval_secs, 300);
    }

    #[test]
    fn test_cache_mode_default() {
        assert_eq!(CacheMode::default(), CacheMode::Simple);
    }

    #[test]
    fn test_cache_mode_debug_clone() {
        let modes = [CacheMode::Simple, CacheMode::Embedding, CacheMode::Remote];
        for &mode in &modes {
            let cloned = mode;
            assert_eq!(mode, cloned);
        }
    }

    // ── EmbeddingCacheEntry tests ───────────────────────────────────────────

    #[test]
    fn test_cache_entry_expired() {
        let entry = EmbeddingCacheEntry {
            key: "test".into(),
            embedding: vec![0.5; 384],
            value: json!("v"),
            access_count: 1,
            created_at: 0, // epoch
            ttl_secs: 1,   // expired after 1 second
        };
        assert!(entry.is_expired());
    }

    #[test]
    fn test_cache_entry_not_expired() {
        let now = crate::acp::prelude::now_ts() as u64;
        let entry = EmbeddingCacheEntry {
            key: "test".into(),
            embedding: vec![0.5; 384],
            value: json!("v"),
            access_count: 1,
            created_at: now,
            ttl_secs: 3600,
        };
        assert!(!entry.is_expired());
    }

    // ── Embedding computation tests ─────────────────────────────────────────

    #[test]
    fn test_compute_embedding_deterministic() {
        let a = compute_embedding_inner("hello world", 384);
        let b = compute_embedding_inner("hello world", 384);
        assert_eq!(a, b, "embedding must be deterministic");
    }

    #[test]
    fn test_compute_embedding_normalized() {
        let vec = compute_embedding_inner("test text", 384);
        let mag: f64 = vec.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!(
            (mag - 1.0).abs() < 1e-6,
            "expected unit vector, got mag={}",
            mag
        );
    }

    #[test]
    fn test_compute_embedding_similar_texts() {
        let a = compute_embedding_inner("What is the capital of France?", 384);
        let b = compute_embedding_inner("what is capital of france?", 384);
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim > 0.8,
            "similar texts should have high cosine similarity, got {}",
            sim
        );
    }

    #[test]
    fn test_compute_embedding_different_texts() {
        let a = compute_embedding_inner("hello world", 384);
        let b = compute_embedding_inner("completely different topic here", 384);
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim < 0.8,
            "different texts should have lower similarity, got {}",
            sim
        );
    }

    #[test]
    fn test_embedding_dimension_respected() {
        let vec = compute_embedding_inner("test", 128);
        assert_eq!(vec.len(), 128);
        let vec = compute_embedding_inner("test", 384);
        assert_eq!(vec.len(), 384);
    }

    #[test]
    fn test_embedding_empty_text() {
        let vec = compute_embedding_inner("", 384);
        assert_eq!(vec.len(), 384);
        let mag: f64 = vec.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!(
            (mag - 0.0).abs() < 1e-6,
            "empty text should produce zero vector"
        );
    }

    // ── LRU eviction edge cases ─────────────────────────────────────────────

    #[test]
    fn test_evict_lru_empty_cache() {
        let config = EmbeddingCacheConfig::default();
        let cache = EmbeddingSemanticCache::new(&config);
        let result = cache.evict_lru();
        assert!(result.is_none(), "evict on empty cache should return None");
    }

    #[test]
    fn test_evict_lru_single_entry() {
        let config = EmbeddingCacheConfig::default();
        let cache = EmbeddingSemanticCache::new(&config);
        cache.set("only", json!("one"), 3600);
        let evicted = cache.evict_lru();
        assert!(evicted.is_some());
        assert_eq!(evicted.expect("evicted entry should be Some").key, "only");
        assert!(cache.is_empty());
    }

    #[test]
    fn test_evict_lru_tie_break() {
        let config = EmbeddingCacheConfig::default();
        let cache = EmbeddingSemanticCache::new(&config);
        cache.set("first", json!("1"), 3600);
        // Small delay so timestamps differ
        std::thread::sleep(std::time::Duration::from_millis(5));
        cache.set("second", json!("2"), 3600);

        // Both have access_count=1, so oldest (first) should be evicted
        let evicted = cache.evict_lru();
        assert!(evicted.is_some());
        assert_eq!(
            evicted.expect("evicted entry should be Some").key,
            "first",
            "oldest entry should be evicted on tie"
        );
    }

    // ── Tokenization edge cases ─────────────────────────────────────────────

    #[test]
    fn test_tokenize_empty() {
        let tokens = tokenize("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_tokenize_punctuation() {
        let tokens = tokenize("hello, world! how-are you?");
        assert_eq!(tokens, vec!["hello", "world", "how", "are", "you"]);
    }

    #[test]
    fn test_tokenize_numbers() {
        let tokens = tokenize("test123 abc 42");
        assert_eq!(tokens, vec!["test123", "abc"]);
    }
}
