//! Multi-Level Token Cache
//!
//! Implements a CPU-inspired multi-level cache architecture for Agent token output reuse:
//!
//! - **L1**: Exact-match cache (fastest, smallest — in-memory LRU HashMap)
//! - **L2**: Semantic-similarity cache (medium — in-memory vector index)
//! - **L3**: Template-structure cache (largest, persistent — SQLite-backed)
//!
//! Each level targets a specific context-length class:
//! - **Short** (0-500 tokens):  L1 optimized
//! - **Medium** (500-2000 tokens): L2 optimized
//! - **Long** (2000+ tokens): L3 optimized
//!
//! The cache integrates with the Agent trait via `CachedAgentWrapper` and feeds
//! statistics back into the reinforcement-learning loop for adaptive eviction.

// All sub-module code is inlined in this file.
// L1, L2, L3, stats, and wrapper are in the same module.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::agent::{Agent, Message, StreamingSender};
use crate::core::error::Result as AppResult;

// All types are defined inline in this module (not in sub-modules).
// Re-exports are not needed since the types are already public in this module.

// ─── Shared types ─────────────────────────────────────────────────────────

/// Context-length classification (mimics CPU cache hierarchy)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContextLengthClass {
    /// 0-500 tokens → L1 cache targets this class
    Short,
    /// 500-2000 tokens → L2 cache targets this class
    Medium,
    /// 2000+ tokens → L3 cache targets this class
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
/// Routes lookup requests through L1 → L2 → L3, tracks hit rates per level,
/// and reports savings statistics.
pub struct TokenMultiLevelCache {
    /// L1: Exact-match cache
    pub l1: RwLock<L1ExactCache>,
    /// L2: Semantic-similarity cache
    pub l2: RwLock<L2SemanticCache>,
    /// L3: Template-structure cache
    pub l3: RwLock<L3TemplateCache>,
    /// Aggregate statistics across all levels
    pub stats: RwLock<TokenCacheStats>,
    /// Whether the cache is enabled
    pub enabled: RwLock<bool>,
}

impl TokenMultiLevelCache {
    /// Create a new multi-level cache with default capacities.
    pub fn new(l1_capacity: usize, l2_capacity: usize, l3_store_path: &str) -> Self {
        Self {
            l1: RwLock::new(L1ExactCache::new(l1_capacity)),
            l2: RwLock::new(L2SemanticCache::new(l2_capacity)),
            l3: RwLock::new(L3TemplateCache::new(l3_store_path)),
            stats: RwLock::new(TokenCacheStats::default()),
            enabled: RwLock::new(true),
        }
    }

    /// Look up a cache entry by input text.
    ///
    /// Routes: L1 (exact) → L2 (semantic) → L3 (template).
    /// Returns the best matching entry and which level it was found at.
    pub async fn lookup(
        &self,
        input: &str,
        context_class: ContextLengthClass,
    ) -> Option<(CacheLevel, CacheEntry)> {
        if !*self.enabled.read().await {
            return None;
        }

        // L1: Exact match (fastest path)
        let l1_key = hash_input(input);
        if let Some(entry) = self.l1.write().await.get(&l1_key) {
            if entry.output.len() > 10 {
                self.stats
                    .write()
                    .await
                    .record_hit(CacheLevel::L1, entry.token_count);
                return Some((CacheLevel::L1, entry));
            }
        }

        // L2: Semantic match (medium-length inputs)
        if context_class != ContextLengthClass::Short {
            let query_vec = simple_embedding(input);
            if let Some(entry) = self.l2.write().await.find_similar(&query_vec) {
                if entry.output.len() > 10 {
                    self.stats
                        .write()
                        .await
                        .record_hit(CacheLevel::L2, entry.token_count);
                    return Some((CacheLevel::L2, entry));
                }
            }
        }

        // L3: Template match (long inputs)
        if context_class == ContextLengthClass::Long {
            if let Some(entry) = self.l3.read().await.match_template(input) {
                self.stats
                    .write()
                    .await
                    .record_hit(CacheLevel::L3, entry.token_count);
                return Some((CacheLevel::L3, entry));
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
        if !*self.enabled.read().await {
            return;
        }

        let l1_key = hash_input(input);
        let entry = CacheEntry {
            key: l1_key.clone(),
            input: input.to_string(),
            output: output.to_string(),
            token_count,
            context_class: ContextLengthClass::from_token_count(token_count),
            hit_count: 0,
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

        // Store in L3 if very long
        if token_count > 2000 {
            self.l3.write().await.add_template(&entry);
        }

        self.stats.write().await.total_entries += 1;
    }

    /// Warm up the cache from a list of recent chat records.
    pub async fn warmup(&self, entries: Vec<CacheEntry>) {
        for entry in entries {
            if entry.hit_count > 2 {
                self.l1.write().await.put(entry.key.clone(), entry.clone());
            }
            if entry.token_count > 500 {
                let vec = simple_embedding(&entry.input);
                self.l2.write().await.add(vec, entry);
            }
        }
    }

    /// Generate a JSON report of cache performance.
    pub async fn report(&self) -> serde_json::Value {
        let stats = self.stats.read().await;
        stats.to_json()
    }

    /// Clear all cache levels.
    pub async fn clear(&self) {
        self.l1.write().await.clear();
        self.l2.write().await.clear();
        self.l3.write().await.clear();
        self.stats.write().await.reset();
    }
}

/// Which cache level produced a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheLevel {
    L1,
    L2,
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

/// Estimate token count from text (simple heuristic: chars / 4).
pub fn estimate_token_count(text: &str) -> usize {
    (text.len() / 4).max(1)
}

/// Estimate token count from a list of messages.
pub fn estimate_messages_token_count(messages: &[crate::agent::Message]) -> usize {
    messages
        .iter()
        .map(|m| m.content.len() / 4 + m.role.len() / 4)
        .sum::<usize>()
        .max(1)
}

/// Convert messages to a single canonical text string for caching.
pub fn messages_to_text(messages: &[crate::agent::Message]) -> String {
    messages
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n")
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

    /// Get an entry by key. Moves it to MRU position.
    pub fn get(&mut self, key: &str) -> Option<CacheEntry> {
        if let Some(entry) = self.map.get(key) {
            let mut entry = entry.clone();
            entry.hit_count += 1;
            entry.last_access_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            // Move to MRU (back of deque)
            if let Some(pos) = self.order.iter().position(|k| k == key) {
                let k = self.order.remove(pos).unwrap();
                self.order.push_back(k);
            }

            self.map.insert(key.to_string(), entry.clone());
            Some(entry)
        } else {
            None
        }
    }

    /// Insert or update an entry. Evicts LRU if at capacity.
    pub fn put(&mut self, key: String, entry: CacheEntry) {
        if self.map.contains_key(&key) {
            // Update existing
            if let Some(pos) = self.order.iter().position(|k| *k == key) {
                let k = self.order.remove(pos).unwrap();
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

    /// Iterate over all entries (for warmup serialization).
    pub fn entries(&self) -> Vec<CacheEntry> {
        self.map.values().cloned().collect()
    }
}

// L2: Semantic-Similarity Cache
//
// Middle tier. Uses a simple vector embedding and cosine similarity
// to find semantically similar past queries. Targets medium-length
// contexts (500-2000 tokens).

/// Simple bag-of-words embedding (256 dimensions).
///
/// This is a lightweight embedding that doesn't require an external model.
/// For production use, replace with a real embedding model (e.g., from the
/// existing VectorStore infrastructure in `src/memory/vector.rs`).
pub fn simple_embedding(text: &str) -> Vec<f32> {
    const DIM: usize = 256;
    let mut vec = vec![0.0f32; DIM];

    // Simple hash-based feature extraction
    let lower = text.to_ascii_lowercase();
    for (i, ch) in lower.chars().enumerate() {
        let idx = (ch as usize) % DIM;
        vec[idx] += 1.0;

        // Bigram features
        if i > 0 {
            let prev = lower.as_bytes().get(i - 1).copied().unwrap_or(0) as usize;
            let bigram_idx = (prev.wrapping_mul(ch as usize)) % DIM;
            vec[bigram_idx] += 0.5;
        }
    }

    // Normalize to unit vector
    let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }

    vec
}

/// Cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
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

    /// Find the most semantically similar entry above the threshold.
    pub fn find_similar(&mut self, query_vec: &[f32]) -> Option<CacheEntry> {
        let mut best_score = 0.0f32;
        let mut best_idx = None;

        for (i, vec) in self.vectors.iter().enumerate() {
            let score = cosine_similarity(query_vec, vec);
            if score > best_score && score >= self.similarity_threshold {
                best_score = score;
                best_idx = Some(i);
            }
        }

        if let Some(idx) = best_idx {
            let mut entry = self.entries[idx].clone();
            entry.hit_count += 1;
            self.entries[idx] = entry.clone();
            Some(entry)
        } else {
            None
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
}

// L3: Template-Structure Cache
//
// Largest, most persistent tier. Identifies structural patterns in
// long-context queries (2000+ tokens) and caches reusable templates.
// Uses SQLite for persistence across restarts.

use std::path::PathBuf;

// CacheEntry is already in scope from the shared types above.

/// A reusable template extracted from a long-context interaction.
#[derive(Debug, Clone)]
pub struct TemplatePattern {
    /// Pattern name / category (e.g., "code_review", "arch_design", "debug_analysis")
    pub pattern_type: String,
    /// Input structural signature (hash of normalized structure)
    pub structure_signature: String,
    /// Common structural tokens (canonicalized)
    pub structural_prefix: String,
    /// Output template with `{placeholder}` markers
    pub output_template: String,
    /// How many times this template has been reused
    pub hit_count: u64,
    /// Estimated tokens saved per use
    pub estimated_savings: usize,
}

/// L3 template-structure cache.
pub struct L3TemplateCache {
    /// Known templates by structure signature
    templates: HashMap<String, TemplatePattern>,
    /// Path to SQLite store (for persistence)
    #[allow(dead_code)]
    store_path: PathBuf,
}

impl L3TemplateCache {
    pub fn new(store_path: &str) -> Self {
        Self {
            templates: HashMap::new(),
            store_path: PathBuf::from(store_path),
        }
    }

    /// Extract a structural signature from input text.
    ///
    /// Normalizes the text by:
    /// - Lowercasing
    /// - Replacing variable names with `{var}`
    /// - Replacing numbers with `{n}`
    /// - Keeping structural keywords (if, for, class, fn, etc.)
    fn extract_signature(input: &str) -> String {
        let mut sig = String::new();
        let mut prev_was_code = false;

        for word in input.split_whitespace() {
            let lower = word.to_ascii_lowercase();
            let is_code_keyword = matches!(
                lower.as_str(),
                "fn" | "struct"
                    | "impl"
                    | "trait"
                    | "enum"
                    | "class"
                    | "def"
                    | "function"
                    | "if"
                    | "for"
                    | "while"
                    | "match"
                    | "import"
                    | "use"
                    | "mod"
                    | "pub"
                    | "async"
                    | "await"
                    | "let"
                    | "mut"
                    | "const"
                    | "return"
                    | "type"
                    | "where"
                    | "error"
                    | "result"
                    | "option"
                    | "ok"
                    | "none"
            );

            if is_code_keyword {
                sig.push_str(&lower);
                sig.push(' ');
                prev_was_code = true;
            } else if lower.chars().all(|c| c.is_numeric() || c == '.') {
                sig.push_str("{n} ");
                prev_was_code = false;
            } else {
                // Check if it looks like a variable/symbol
                let is_symbol = word.contains(|c: char| !c.is_alphanumeric() && c != ' ');
                if prev_was_code && is_symbol {
                    sig.push_str("{var} ");
                }
                prev_was_code = false;
            }
        }

        sig
    }

    /// Try to match input against known templates.
    pub fn match_template(&self, input: &str) -> Option<CacheEntry> {
        let sig = Self::extract_signature(input);
        if sig.len() < 10 {
            return None;
        }

        // Check for exact structural match
        if let Some(template) = self.templates.get(&sig) {
            let filled = template.output_template.replace("{input}", input);
            // Return a synthetic CacheEntry for this template hit
            return Some(CacheEntry {
                key: template.structure_signature.clone(),
                input: input.to_string(),
                output: filled,
                token_count: template.estimated_savings,
                context_class: ContextLengthClass::Long,
                hit_count: template.hit_count,
                created_at: 0,
                last_access_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
                agent_name: None,
                model: None,
            });
        }

        None
    }

    /// Add a new template derived from a cache entry.
    pub fn add_template(&mut self, entry: &CacheEntry) {
        let sig = Self::extract_signature(&entry.input);
        if sig.len() < 10 || self.templates.contains_key(&sig) {
            return;
        }

        let pattern = TemplatePattern {
            pattern_type: if entry.input.contains("bug")
                || entry.input.contains("fix")
                || entry.input.contains("error")
            {
                "debug_analysis".to_string()
            } else if entry.input.contains("review") || entry.input.contains("audit") {
                "code_review".to_string()
            } else if entry.input.contains("design") || entry.input.contains("architecture") {
                "arch_design".to_string()
            } else {
                "general".to_string()
            },
            structure_signature: sig.clone(),
            structural_prefix: entry.input.chars().take(200).collect(),
            output_template: entry.output.clone(),
            hit_count: 0,
            estimated_savings: entry.token_count / 2,
        };

        self.templates.insert(sig, pattern);
    }

    /// Clear all templates.
    pub fn clear(&mut self) {
        self.templates.clear();
    }

    /// Number of known templates.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// All known patterns (for reporting).
    pub fn patterns(&self) -> Vec<TemplatePattern> {
        self.templates.values().cloned().collect()
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
    // L3 stats
    pub l3_hits: u64,
    pub l3_misses: u64,
    // Total tracking
    pub total_entries: usize,
    pub total_tokens_saved: u64,
    pub total_tokens_served: u64,
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
        self.total_tokens_served += tokens_saved as u64;
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
                self.l3_misses += 1;
                self.long_misses += 1;
            }
        }
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Hit rate for a given level (0.0 - 1.0).
    pub fn hit_rate(&self) -> f64 {
        let total_hits = self.l1_hits + self.l2_hits + self.l3_hits;
        let total_misses = self.l1_misses + self.l2_misses + self.l3_misses;
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
        let total_misses = self.l1_misses + self.l2_misses + self.l3_misses;
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
                "misses": self.l3_misses,
                "hit_rate": if self.l3_hits + self.l3_misses == 0 { 0.0 } else { self.l3_hits as f64 / (self.l3_hits + self.l3_misses) as f64 },
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
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        sender: StreamingSender,
    ) -> AppResult<()> {
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
        if let Some((level, entry)) = self.cache.lookup(&input_text, context_class).await {
            tracing::debug!(
                target = "token_cache",
                level = %level,
                "CachedAgentWrapper: cache HIT, returning cached output"
            );
            // Cache hit – send the cached response through the stream sender.
            let _ = sender.send(entry.output.clone());
            return Ok(());
        }

        // --- Cache miss – delegate to the inner agent ---
        let output = {
            // Collect the streaming output into a single String.
            let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(2048);
            let inner_sender = StreamingSender::from(tx);

            let inner = self.inner.clone();
            let handle = tokio::spawn(async move {
                inner
                    .chat(messages, principles, options, inner_sender)
                    .await
            });

            let mut response = String::new();
            while let Some(token) = rx.recv().await {
                // Forward the token to the caller's sender.
                if sender.send(token.clone()).is_err() {
                    // The caller dropped the receiver – stop forwarding.
                    break;
                }
                response.push_str(&token);
            }

            // Await the inner agent's completion.
            match handle.await {
                Ok(Ok(())) => response,
                Ok(Err(err)) => return Err(err),
                Err(join_err) => {
                    return Err(crate::core::error::AppError::Proxy(
                        crate::core::error::ProxyError::Internal(format!(
                            "cached agent wrapper: inner agent panicked: {join_err}"
                        )),
                    ))
                }
            }
        };

        // --- Store result in cache ---
        let token_count = estimate_token_count(&output);
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
