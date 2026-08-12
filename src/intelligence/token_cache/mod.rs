//! Multi-Level Token Cache
//!
//! Implements a CPU-inspired multi-level cache architecture for Agent token output reuse:
//!
//! - **L1**: Exact-match cache (fastest, smallest — in-memory LRU HashMap)
//! - **L2**: Semantic-similarity cache (medium — in-memory vector index)
//!
//! Each level targets a specific context-length class:
//! - **Short** (0-500 tokens):  L1 optimized
//! - **Medium/Long** (500+ tokens): L2 optimized
//!
//! The cache integrates with the Agent trait via `CachedAgentWrapper` and feeds
//! statistics back into the reinforcement-learning loop for adaptive eviction.

// All sub-module code is inlined in this file.
// L1, L2, stats, and wrapper are in the same module.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing;

use crate::agent::{Agent, Message, StreamingSender};
use crate::core::error::Result as AppResult;

// All types are defined inline in this module (not in sub-modules).
// Re-exports are not needed since the types are already public in this module.

// ─── Persistent cache backend (L3) ───────────────────────────────────────

/// A cached response loaded from the persistent layer.
#[derive(Debug, Clone)]
pub struct PersistentCachedResponse {
    /// The cached response text.
    pub response_text: String,
    /// Agent that generated the response, if recorded.
    pub agent_name: Option<String>,
}

/// Trait implemented by durable cache backends (e.g. `ResponseCache` on
/// SQLite/PostgreSQL). The token cache consults this layer on L1/L2 miss and
/// writes back on store, giving cross-restart / cross-instance cache reuse.
#[async_trait]
pub trait PersistentCacheBackend: Send + Sync {
    /// Fetch a cached response by its exact key.
    async fn get_cached(&self, key: &str) -> anyhow::Result<Option<PersistentCachedResponse>>;

    /// Store a response under the given exact key.
    async fn put_cached(
        &self,
        key: &str,
        response_text: &str,
        agent_name: Option<&str>,
    ) -> anyhow::Result<()>;
}

// ─── Shared types ─────────────────────────────────────────────────────────

/// Context-length classification (mimics CPU cache hierarchy)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextLengthClass {
    /// 0-500 tokens → L1 cache targets this class
    Short,
    /// 500+ tokens → L2 cache targets this class
    Medium,
    /// 2000+ tokens — legacy long-input class; served by L2 like Medium
    Long,
}

impl ContextLengthClass {
    pub fn from_token_count(count: usize) -> Self {
        match count {
            0..=500 => ContextLengthClass::Short,
            501..=2000 => ContextLengthClass::Medium,
            _ => ContextLengthClass::Long,
        }
    }
}

/// A single cached entry shared across all cache levels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Cache key (L1: input text hash, L2: semantic hash, L3: pattern signature)
    pub key: String,
    /// Input text that generated this entry
    pub input: String,
    /// Full output text (cached response)
    pub output: String,
    /// Estimated token count of the output
    pub token_count: usize,
    /// Context length class at time of creation
    pub context_class: ContextLengthClass,
    /// Number of times this entry has been hit
    pub hit_count: u64,
    /// Unix timestamp of creation
    pub created_at: i64,
    /// Unix timestamp of last access
    pub last_access_at: i64,
    /// Agent name that generated this response (if known)
    pub agent_name: Option<String>,
    /// Model name used (if known)
    pub model: Option<String>,
}

impl CacheEntry {
    pub fn new(key: String, input: String, output: String, token_count: usize) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Self {
            key,
            context_class: ContextLengthClass::from_token_count(token_count),
            hit_count: 1,
            created_at: now,
            last_access_at: now,
            agent_name: None,
            model: None,
            input,
            output,
            token_count,
        }
    }
}

/// Multi-level token cache orchestrator.
///
/// Routes lookup requests through L1 → L2, tracks hit rates per level,
/// and reports savings statistics.
pub struct TokenMultiLevelCache {
    /// L1: Exact-match cache
    pub l1: RwLock<L1ExactCache>,
    /// L2: Semantic-similarity cache
    pub l2: RwLock<L2SemanticCache>,
    /// Aggregate statistics across all levels
    pub stats: RwLock<TokenCacheStats>,
    /// Whether the cache is enabled (lock-free atomic flag)
    pub enabled: AtomicBool,
    /// Optional durable backend (L3). When set, `lookup` falls through to it
    /// on L1/L2 miss and `store` writes back asynchronously. This gives
    /// cross-restart and cross-instance (shared SQLite/PG) cache reuse.
    persistent: Option<Arc<dyn PersistentCacheBackend>>,
}

impl TokenMultiLevelCache {
    /// Create a new multi-level cache with default capacities.
    pub fn new(l1_capacity: usize, l2_capacity: usize) -> Self {
        Self {
            l1: RwLock::new(L1ExactCache::new(l1_capacity)),
            l2: RwLock::new(L2SemanticCache::new(l2_capacity)),
            stats: RwLock::new(TokenCacheStats::default()),
            enabled: AtomicBool::new(true),
            persistent: None,
        }
    }

    /// Attach a durable backend as the L3 layer (e.g. the SQLite/Postgres
    /// `ResponseCache`). The returned handle is the same cache with the
    /// backend attached; call sites typically build via
    /// `TokenMultiLevelCache::new(a, b).with_persistent_backend(arc)`.
    pub fn with_persistent_backend(mut self, backend: Arc<dyn PersistentCacheBackend>) -> Self {
        self.persistent = Some(backend);
        self
    }

    /// Look up a cache entry by input text.
    ///
    /// Routes: L1 (exact) → L2 (semantic) → L3 (persistent, optional).
    /// Uses **read** locks for the lookup path to avoid unnecessary contention.
    /// Returns the best matching entry, which level it was found at, and the
    /// match confidence (1.0 for exact/durable hits, the L2 cosine score
    /// otherwise) so callers do not re-embed the input.
    pub async fn lookup(
        &self,
        input: &str,
        context_class: ContextLengthClass,
    ) -> Option<(CacheLevel, CacheEntry, f32)> {
        if !self.enabled.load(Ordering::Acquire) {
            return None;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // L1: Exact match (fastest path) — read-only peek, then best-effort
        // LRU touch (moves the key to the back of the recency order) so the
        // eviction policy is a real LRU instead of insertion-order FIFO.
        let l1_key = hash_input(input);
        let l1_hit = {
            let guard = self.l1.read().await;
            guard.peek(&l1_key)
        };
        if let Some(entry) = l1_hit {
            if entry.output.len() > 10 {
                if let Ok(mut l1) = self.l1.try_write() {
                    l1.touch(&l1_key);
                }
                self.stats
                    .write()
                    .await
                    .record_hit(CacheLevel::L1, entry.token_count);
                return Some((CacheLevel::L1, entry, 1.0));
            }
        }

        // L2: Semantic match (medium/long inputs) — read-only peek, then
        // best-effort hit-count touch so "least-hit" eviction is real.
        // The embedding is computed once and reused by the L3 promotion path
        // below (L2 miss → L3 hit adds the same vector to L2), avoiding a
        // duplicate 256-dim embedding per lookup.
        let query_vec = if context_class != ContextLengthClass::Short {
            Some(simple_embedding(input))
        } else {
            None
        };
        if let Some(query_vec) = query_vec.as_ref() {
            let l2_hit = {
                let guard = self.l2.read().await;
                guard.peek_similar(query_vec)
            };
            if let Some((idx, entry, score)) = l2_hit {
                if entry.output.len() > 10 {
                    if let Ok(mut l2) = self.l2.try_write() {
                        l2.touch(idx);
                    }
                    self.stats
                        .write()
                        .await
                        .record_hit(CacheLevel::L2, entry.token_count);
                    return Some((CacheLevel::L2, entry, score));
                }
            }
        }

        // L3: Persistent backend (optional) — exact key lookup on L1/L2 miss.
        // The backend (SQLite/Postgres `ResponseCache`) performs its own
        // `spawn_blocking` + TTL handling; a hit is promoted back into L1/L2
        // so subsequent identical inputs skip the disk read.
        if let Some(ref backend) = self.persistent {
            if let Ok(Some(found)) = backend.get_cached(&l1_key).await {
                if found.response_text.len() > 10 {
                    let token_count = estimate_token_count(&found.response_text);
                    let entry = CacheEntry {
                        key: l1_key.clone(),
                        input: input.to_string(),
                        output: found.response_text.clone(),
                        token_count,
                        context_class: ContextLengthClass::from_token_count(token_count),
                        hit_count: 1,
                        created_at: (now_ms / 1000) as i64,
                        last_access_at: (now_ms / 1000) as i64,
                        agent_name: found.agent_name,
                        model: None,
                    };
                    // Promote into L1 (and L2 for non-short inputs) so the
                    // durable hit is served from memory on the next request.
                    self.l1.write().await.put(l1_key.clone(), entry.clone());
                    if let Some(query_vec) = query_vec.as_ref() {
                        self.l2.write().await.add(query_vec.clone(), entry.clone());
                    }
                    self.stats
                        .write()
                        .await
                        .record_hit(CacheLevel::L3, entry.token_count);
                    return Some((CacheLevel::L3, entry, 1.0));
                }
            }
        }

        self.stats.write().await.record_miss(context_class);
        None
    }

    /// Store a new entry in the cache (stores in all applicable levels).
    pub async fn store(
        &self,
        input: &str,
        output: &str,
        token_count: usize,
        agent_name: Option<String>,
        model: Option<String>,
    ) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }

        let l1_key = hash_input(input);
        let entry = CacheEntry {
            key: l1_key.clone(),
            input: input.to_string(),
            output: output.to_string(),
            token_count,
            context_class: ContextLengthClass::from_token_count(token_count),
            // A freshly stored entry was just served (it was produced by a
            // miss), so it starts with one hit — consistent with
            // `CacheEntry::new` and the L3 promotion path.
            hit_count: 1,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            last_access_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            agent_name,
            model,
        };

        // Always store in L1
        self.l1.write().await.put(l1_key.clone(), entry.clone());

        // Store in L2 if medium or long
        if token_count > 500 {
            let vec = simple_embedding(input);
            self.l2.write().await.add(vec, entry.clone());
        }

        self.stats.write().await.total_entries += 1;

        // L3: Asynchronously persist so the write never blocks the request
        // hot path. Uses the exact L1 key so a later `lookup` on L1/L2 miss
        // can recover the entry from the durable backend.
        if let Some(ref backend) = self.persistent {
            let backend = Arc::clone(backend);
            let key = l1_key;
            let out = entry.output.clone();
            let agent = entry.agent_name.clone();
            tokio::spawn(async move {
                let _ = backend.put_cached(&key, &out, agent.as_deref()).await;
            });
        }
    }
}

/// Which cache level produced a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheLevel {
    L1,
    L2,
    /// Durable backend hit (SQLite/Postgres `ResponseCache`), promoted into
    /// L1/L2 for subsequent in-memory hits.
    L3,
}

impl std::fmt::Display for CacheLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheLevel::L1 => write!(f, "L1"),
            CacheLevel::L2 => write!(f, "L2"),
            CacheLevel::L3 => write!(f, "L3"),
        }
    }
}

/// Estimate token count from text using the canonical CJK/ASCII-weighted
/// estimator (see [`crate::shared::token_estimator::estimate_tokens`]).
pub fn estimate_token_count(text: &str) -> usize {
    crate::shared::token_estimator::estimate_tokens(text)
}

/// Estimate token count from a list of messages.
pub fn estimate_messages_token_count(messages: &[crate::agent::Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            crate::shared::token_estimator::estimate_tokens(&m.content)
                + crate::shared::token_estimator::estimate_tokens(&m.role)
        })
        .sum::<usize>()
        .max(1)
}

/// Convert messages to a single canonical text string for caching.
pub fn messages_to_text(messages: &[crate::agent::Message]) -> String {
    // Avoid intermediate Vec<String> allocation — build directly into a String.
    let mut result = String::new();
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        result.push_str(m.role.as_str());
        result.push_str(": ");
        result.push_str(m.content.as_str());
    }
    result
}

/// Detect when the last user message repeats the content of an *earlier* user
/// message in the same conversation.
///
/// Cache gates call this to bypass the cache so the agent produces a fresh
/// response instead of a stale cached answer when the user intentionally
/// repeats a question.
pub fn last_user_message_is_duplicate(messages: &[crate::agent::Message]) -> bool {
    let user_contents: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    match user_contents.len() {
        0 | 1 => false,
        n => user_contents[..n - 1].contains(&user_contents[n - 1]),
    }
}

// L1: Exact-Match Cache
//
// Fastest tier. Uses a HashMap with LRU eviction.
// Targets short context (0-500 tokens) for maximum reuse of
// frequently-asked identical questions.

/// Simple 64-bit hash for cache keys.
pub fn hash_input(input: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// L1 exact-match cache with LRU eviction.
pub struct L1ExactCache {
    map: HashMap<String, CacheEntry>,
    /// LRU order: front = least recently used
    order: VecDeque<String>,
    capacity: usize,
}

impl L1ExactCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    /// Read-only peek — looks up an entry without updating hit count or LRU order.
    /// Use this for read-locked lookup paths to avoid unnecessary write contention.
    pub fn peek(&self, key: &str) -> Option<CacheEntry> {
        self.map.get(key).cloned()
    }

    /// Best-effort LRU touch: moves the key to the back of the recency order
    /// and bumps its hit count. Returns `false` when the key is absent.
    /// Callers invoke this through `try_write` so a contended lock simply
    /// skips the touch instead of blocking the lookup hot path.
    pub fn touch(&mut self, key: &str) -> bool {
        let Some(entry) = self.map.get_mut(key) else {
            return false;
        };
        entry.hit_count = entry.hit_count.saturating_add(1);
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(pos).expect("pos was just found");
            self.order.push_back(k);
        }
        true
    }

    /// Insert or update an entry. Evicts LRU if at capacity.
    pub fn put(&mut self, key: String, entry: CacheEntry) {
        if self.map.contains_key(&key) {
            // Update existing
            if let Some(pos) = self.order.iter().position(|k| *k == key) {
                let k = self
                    .order
                    .remove(pos)
                    .expect("pos is valid because we just found it via position()");
                self.order.push_back(k);
            }
            self.map.insert(key, entry);
            return;
        }

        // Evict LRU if at capacity
        if self.map.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }

        self.order.push_back(key.clone());
        self.map.insert(key, entry);
    }

    /// Remove an entry by key.
    pub fn remove(&mut self, key: &str) {
        self.map.remove(key);
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// Number of entries in L1.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// L2: Semantic-Similarity Cache
//
// Middle tier. Uses a simple vector embedding and cosine similarity
// to find semantically similar past queries. Targets medium-length
// contexts (500-2000 tokens).

/// Embed text for the in-memory L2 semantic cache.
///
/// Thin adapter over the single canonical embedding implementation
/// (`crate::memory::embedding_provider::local_hash_embed`). Note that the
/// dimensionality differs per consumer: L2 uses 256 dims here, the semantic
/// response cache uses 128 (`semantic_cache::request_embedding`), and the
/// vector store uses the provider's configured dims — so vectors are only
/// interchangeable across consumers that share the SAME dimensionality.
pub fn simple_embedding(text: &str) -> Vec<f32> {
    crate::memory::embedding_provider::local_hash_embed(text, 256)
}

/// L2 semantic-similarity cache.
pub struct L2SemanticCache {
    entries: Vec<CacheEntry>,
    vectors: Vec<Vec<f32>>,
    max_entries: usize,
    /// Similarity threshold (0.0-1.0). Higher = stricter match.
    pub similarity_threshold: f32,
}

impl L2SemanticCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            vectors: Vec::new(),
            max_entries,
            similarity_threshold: 0.92,
        }
    }

    /// Read-only peek for semantic similarity — finds similar entries without
    /// updating hit counts. Returns the matched entry's index, a clone, and the
    /// best similarity score so the caller avoids re-embedding the query.
    pub fn peek_similar(&self, query_vec: &[f32]) -> Option<(usize, CacheEntry, f32)> {
        let mut best_score = 0.0f32;
        let mut best_entry = None;

        for (i, vec) in self.vectors.iter().enumerate() {
            let score = crate::shared::math::cosine_similarity_f32(query_vec, vec);
            if score > best_score && score >= self.similarity_threshold {
                best_score = score;
                best_entry = Some((i, self.entries[i].clone(), score));
            }
        }

        best_entry
    }

    /// Best-effort hit recording: bumps the hit count of the entry at `idx` so
    /// eviction prefers entries that have actually been served.
    pub fn touch(&mut self, idx: usize) -> bool {
        if let Some(entry) = self.entries.get_mut(idx) {
            entry.hit_count = entry.hit_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Add a new entry to the cache. Evicts oldest if at capacity.
    pub fn add(&mut self, vec: Vec<f32>, entry: CacheEntry) {
        if self.entries.len() >= self.max_entries {
            // Evict least-hit entry
            let mut min_idx = 0;
            let mut min_hits = u64::MAX;
            for (i, e) in self.entries.iter().enumerate() {
                if e.hit_count < min_hits {
                    min_hits = e.hit_count;
                    min_idx = i;
                }
            }
            self.entries.remove(min_idx);
            self.vectors.remove(min_idx);
        }

        self.entries.push(entry);
        self.vectors.push(vec);
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.vectors.clear();
    }

    /// Number of entries in L2.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// Cache statistics tracking and reporting.

// CacheLevel and ContextLengthClass are already in scope from the shared types above.

/// Aggregate cache statistics across all levels.
#[derive(Debug, Clone, Serialize, Default)]
pub struct TokenCacheStats {
    // L1 stats
    pub l1_hits: u64,
    pub l1_misses: u64,
    // L2 stats
    pub l2_hits: u64,
    pub l2_misses: u64,
    // L3 (durable backend) stats
    pub l3_hits: u64,
    // Total tracking
    pub total_entries: usize,
    pub total_tokens_saved: u64,
    // Per-class breakdown
    pub short_hits: u64,
    pub medium_hits: u64,
    pub long_hits: u64,
    pub short_misses: u64,
    pub medium_misses: u64,
    pub long_misses: u64,
}

impl TokenCacheStats {
    /// Record a cache hit at the given level.
    pub fn record_hit(&mut self, level: CacheLevel, tokens_saved: usize) {
        match level {
            CacheLevel::L1 => self.l1_hits += 1,
            CacheLevel::L2 => self.l2_hits += 1,
            CacheLevel::L3 => self.l3_hits += 1,
        }
        self.total_tokens_saved += tokens_saved as u64;
    }

    /// Record a cache miss for a context class.
    pub fn record_miss(&mut self, class: ContextLengthClass) {
        match class {
            ContextLengthClass::Short => {
                self.l1_misses += 1;
                self.short_misses += 1;
            }
            ContextLengthClass::Medium => {
                self.l2_misses += 1;
                self.medium_misses += 1;
            }
            ContextLengthClass::Long => {
                self.l2_misses += 1;
                self.long_misses += 1;
            }
        }
    }

    /// Hit rate for a given level (0.0 - 1.0).
    pub fn hit_rate(&self) -> f64 {
        let total_hits = self.l1_hits + self.l2_hits + self.l3_hits;
        let total_misses = self.l1_misses + self.l2_misses;
        let total = total_hits + total_misses;
        if total == 0 {
            0.0
        } else {
            total_hits as f64 / total as f64
        }
    }

    /// Convert to JSON for reporting.
    pub fn to_json(&self) -> serde_json::Value {
        let total_hits = self.l1_hits + self.l2_hits + self.l3_hits;
        let total_misses = self.l1_misses + self.l2_misses;
        let total = total_hits + total_misses;
        let overall_hit_rate = if total == 0 {
            0.0
        } else {
            total_hits as f64 / total as f64
        };

        serde_json::json!({
            "l1": {
                "hits": self.l1_hits,
                "misses": self.l1_misses,
                "hit_rate": if self.l1_hits + self.l1_misses == 0 { 0.0 } else { self.l1_hits as f64 / (self.l1_hits + self.l1_misses) as f64 },
            },
            "l2": {
                "hits": self.l2_hits,
                "misses": self.l2_misses,
                "hit_rate": if self.l2_hits + self.l2_misses == 0 { 0.0 } else { self.l2_hits as f64 / (self.l2_hits + self.l2_misses) as f64 },
            },
            "l3": {
                "hits": self.l3_hits,
                "hit_rate": if self.l3_hits == 0 { 0.0 } else { 1.0 },
            },
            "overall": {
                "hit_rate": overall_hit_rate,
                "total_tokens_saved": self.total_tokens_saved,
                "total_entries": self.total_entries,
            },
            "by_context": {
                "short": { "hits": self.short_hits, "misses": self.short_misses },
                "medium": { "hits": self.medium_hits, "misses": self.medium_misses },
                "long": { "hits": self.long_hits, "misses": self.long_misses },
            },
            "estimated_cost_saved_usd": self.total_tokens_saved as f64 * 0.000002,
        })
    }
}

// ─── CachedAgentWrapper ───────────────────────────────────────────────────

/// An Agent wrapper that checks a `TokenMultiLevelCache` before invoking the
/// underlying agent, and stores the result on a cache miss.
///
/// This wrapper can be placed around any `Agent` implementation so that all
/// chat requests benefit from token caching regardless of protocol mode.
pub struct CachedAgentWrapper {
    /// The underlying (wrapped) agent – called on cache miss.
    inner: Arc<dyn Agent>,
    /// Shared reference to the multi-level token cache.
    cache: Arc<TokenMultiLevelCache>,
}

impl CachedAgentWrapper {
    /// Wrap an existing agent with a token cache.
    pub fn new(inner: Arc<dyn Agent>, cache: Arc<TokenMultiLevelCache>) -> Self {
        Self { inner, cache }
    }
}

#[async_trait]
impl Agent for CachedAgentWrapper {
    /// Chat with caching: hash messages, check cache, skip LLM on hit,
    /// otherwise delegate to inner agent and store the result.
    ///
    /// **Duplicate user message detection**: When the last user message repeats
    /// the content of a *previous* user message in the same conversation,
    /// the cache is bypassed so the AI generates a fresh response. This prevents
    /// the GUI chat from silently returning stale answers when the user
    /// intentionally repeats a question.
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: StreamingSender,
    ) -> AppResult<()> {
        // --- Bypass cache when the last user message is a duplicate ---
        // If the user is asking the same thing they already asked before,
        // bypass the cache so they get a *fresh* AI response instead of the
        // cached previous answer.
        let is_duplicate_user = last_user_message_is_duplicate(&messages);

        if is_duplicate_user {
            tracing::debug!(
                target = "token_cache",
                "CachedAgentWrapper: last user message is a duplicate — bypassing cache"
            );
            // Go directly to inner agent without cache lookup or storage.
            return self.inner.chat(messages, principles, options, sender).await;
        }

        // Derive a canonical input string for caching purposes.
        let input_text = messages_to_text(&messages);

        // Compute token count estimate for context classification.
        let estimated_tokens = estimate_messages_token_count(&messages);
        let context_class = ContextLengthClass::from_token_count(estimated_tokens);

        // Extract agent_name and model from options if present.
        let agent_name = options
            .as_ref()
            .and_then(|opts| opts.get("agent_name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let model = options
            .as_ref()
            .and_then(|opts| opts.get("model"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // --- Cache lookup ---
        if let Some((level, entry, confidence)) =
            self.cache.lookup(&input_text, context_class).await
        {
            // Apply the same execution-like gate as act_phase so secondary paths
            // (phase summaries, review gates) never serve cached answers for
            // requests that may have side effects.
            let cache_bypassed =
                crate::acp::helpers::cache_strategy::should_bypass_for_execution("chat", &messages);
            match crate::acp::helpers::cache_strategy::CacheStrategy::decide_from_entry(
                &format!("{level}"),
                &entry,
                confidence,
                cache_bypassed,
            ) {
                crate::acp::helpers::cache_strategy::CacheDecision::Hit { response } => {
                    tracing::debug!(
                        target = "token_cache",
                        level = %level,
                        "CachedAgentWrapper: cache HIT, returning cached output"
                    );
                    // Cache hit – send the cached response through the stream sender.
                    let _ = sender.send(response);
                    return Ok(());
                }
                crate::acp::helpers::cache_strategy::CacheDecision::Refused { .. }
                | crate::acp::helpers::cache_strategy::CacheDecision::Miss => {
                    // Fall through to the inner agent for a fresh response.
                }
            }
        }

        // --- Cache miss – delegate to the inner agent ---
        //
        // Use a tee/broadcast pattern: tokens are streamed to the caller
        // immediately while a background task collects the full response
        // for asynchronous cache storage. This avoids blocking the caller
        // on cache write and eliminates unnecessary intermediate channels.
        let (collect_tx, mut collect_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let inner_sender = StreamingSender::from(collect_tx);

        let inner = self.inner.clone();
        let inner_handle = tokio::spawn(async move {
            inner
                .chat(messages, principles, options, inner_sender)
                .await
        });

        // Spawn a task that forwards each token to the caller's sender
        // while simultaneously collecting the full response for caching.
        let fwd_sender = sender.clone();
        let collect_handle = tokio::spawn(async move {
            let mut response = String::new();
            let mut truncated = false;
            while let Some(token) = collect_rx.recv().await {
                // Forward each token to the caller immediately.
                if fwd_sender.send(token.clone()).is_err() {
                    // The caller dropped the receiver — stop collecting and
                    // mark the response as truncated so it is NOT cached (a
                    // partial answer would be served to the next identical
                    // request as if it were complete).
                    truncated = true;
                    break;
                }
                response.push_str(&token);
            }
            (response, truncated)
        });

        // Await the inner agent's completion.
        match inner_handle.await {
            Ok(Ok(())) => {
                // Inner agent finished; the channel sender is dropped,
                // so collect_rx will drain and collect_handle completes.
            }
            Ok(Err(err)) => {
                return Err(err);
            }
            Err(join_err) => {
                return Err(crate::core::error::AppError::Proxy(
                    crate::core::error::ProxyError::Internal(format!(
                        "cached agent wrapper: inner agent panicked: {join_err}"
                    )),
                ))
            }
        }

        // Collect the full response (channel is drained by the collect task).
        // A panicked collect task defaults to `(empty, truncated=true)` so a
        // partial/empty answer is never cached (the next identical request
        // would be served an empty response).
        let (output, truncated) = collect_handle
            .await
            .unwrap_or_else(|_| (String::new(), true));
        if truncated {
            return Ok(());
        }

        let token_count = estimate_token_count(&output);

        // --- Store result in cache asynchronously ---
        let cache = self.cache.clone();
        let input_text = input_text.clone();
        tokio::spawn(async move {
            cache
                .store(&input_text, &output, token_count, agent_name, model)
                .await;
        });

        Ok(())
    }

    fn available_models(&self) -> Vec<crate::agent::ModelInfo> {
        self.inner.available_models()
    }

    fn default_model(&self) -> Option<crate::agent::ModelInfo> {
        self.inner.default_model()
    }

    fn supports_model_override(&self) -> bool {
        self.inner.supports_model_override()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory durable backend for exercising the L3 fall-through path
    /// without a SQLite/Postgres dependency in unit tests.
    #[derive(Default)]
    struct MockPersistent {
        store: Mutex<HashMap<String, PersistentCachedResponse>>,
    }

    #[async_trait]
    impl PersistentCacheBackend for MockPersistent {
        async fn get_cached(&self, key: &str) -> anyhow::Result<Option<PersistentCachedResponse>> {
            Ok(self.store.lock().unwrap().get(key).cloned())
        }

        async fn put_cached(
            &self,
            key: &str,
            response_text: &str,
            agent_name: Option<&str>,
        ) -> anyhow::Result<()> {
            self.store.lock().unwrap().insert(
                key.to_string(),
                PersistentCachedResponse {
                    response_text: response_text.to_string(),
                    agent_name: agent_name.map(str::to_string),
                },
            );
            Ok(())
        }
    }

    #[tokio::test]
    async fn store_writes_through_to_persistent_backend() {
        let backend = Arc::new(MockPersistent::default());
        let cache = TokenMultiLevelCache::new(10, 10)
            .with_persistent_backend(Arc::clone(&backend) as Arc<dyn PersistentCacheBackend>);

        cache
            .store("hello world", "cached reply", 4, None, None)
            .await;
        // Give the fire-and-forget persist task a moment to run.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let key = hash_input("hello world");
        let stored = backend.store.lock().unwrap().get(&key).cloned();
        assert!(
            stored.is_some(),
            "L3 backend must receive the stored response"
        );
        assert_eq!(stored.unwrap().response_text, "cached reply");
    }

    #[tokio::test]
    async fn lookup_falls_through_to_persistent_backend_on_l1_l2_miss() {
        let backend = Arc::new(MockPersistent::default());
        // Seed the durable layer directly (simulates a previous process's write).
        let key = hash_input("question");
        backend.store.lock().unwrap().insert(
            key,
            PersistentCachedResponse {
                response_text: "durable answer".to_string(),
                agent_name: Some("deepseek".to_string()),
            },
        );

        let cache = TokenMultiLevelCache::new(10, 10)
            .with_persistent_backend(Arc::clone(&backend) as Arc<dyn PersistentCacheBackend>);

        let hit = cache
            .lookup("question", ContextLengthClass::Short)
            .await
            .expect("durable hit must resolve");
        assert_eq!(hit.0, CacheLevel::L3, "expected an L3 durable hit");
        assert_eq!(hit.1.output, "durable answer");
        assert_eq!(hit.1.agent_name.as_deref(), Some("deepseek"));

        // A second lookup should now be served from L1 (promoted) without
        // touching the durable layer again.
        let second = cache
            .lookup("question", ContextLengthClass::Short)
            .await
            .expect("promoted entry must resolve");
        assert_eq!(second.0, CacheLevel::L1, "promoted hit should be L1");
    }

    #[tokio::test]
    async fn without_backend_lookup_returns_none() {
        let cache = TokenMultiLevelCache::new(10, 10);
        let hit = cache.lookup("anything", ContextLengthClass::Short).await;
        assert!(hit.is_none(), "no durable backend => no L3 hit");
    }
}
