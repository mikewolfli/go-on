//! Skill Discovery and Capability Matching (Step 10 / Full-Auto)
//!
//! Provides a semantic skill index with search, scoring, and caching
//! capabilities. Enables the full-auto flow and the `skill-finder` MCP tool
//! to automatically identify the best-matching skills for a given task.
//!
//! Scoring formula (weighted composite):
//!   - Name token overlap: 35%  (`WEIGHT_NAME`)
//!   - Description token overlap: 40%  (`WEIGHT_DESCRIPTION`)
//!   - Runtime success rate: 25%  (`WEIGHT_RUNTIME`)
//!
//! Design:
//! - `SkillIndex` maintains an in-memory index of registered skills
//!   with token-based similarity search.
//! - `SkillDiscovery` wraps the index and provides task-to-skill
//!   matching with scoring, ranking, and result caching.
//!
//! Types are consumed through a global OnceLock static in tools_pack.rs.
// F-GAP-51: dead_code allowed on items below in non-test builds (consumed via OnceLock)

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::orchestration::skill::{SkillDescriptor, SkillRegistry};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum composite score for a skill to be considered a match.
const MIN_MATCH_SCORE: f64 = 0.40;

/// Weight for name similarity in composite scoring.
const WEIGHT_NAME: f64 = 0.35;

/// Weight for description semantic similarity.
const WEIGHT_DESCRIPTION: f64 = 0.40;

/// Weight for runtime score (historical success rate).
const WEIGHT_RUNTIME: f64 = 0.25;

/// Default TTL for cached discovery results (5 minutes).
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Maximum number of cached entries.
const MAX_CACHE_ENTRIES: usize = 200;

// ---------------------------------------------------------------------------
// SkillIndexEntry
// ---------------------------------------------------------------------------

/// A single entry in the skill index, holding all metadata needed for
/// semantic search and scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIndexEntry {
    pub name: String,
    pub description: String,
    pub tokens: HashSet<String>,
    pub category: String,
    pub score: f64,
    pub last_used_ms: u64,
}

impl SkillIndexEntry {
    /// Build an index entry from a skill descriptor and its runtime stats.
    pub fn from_descriptor(desc: &SkillDescriptor) -> Self {
        let tokens = tokenize(&format!("{} {}", desc.name, desc.description));
        let runtime_score = if desc.total_calls > 0 {
            desc.success_calls as f64 / desc.total_calls as f64
        } else {
            0.5 // Default baseline matching SkillRuntimeStats::score()
        };
        Self {
            name: desc.name.clone(),
            description: desc.description.clone(),
            tokens,
            category: String::new(),
            score: runtime_score,
            last_used_ms: 0,
        }
    }

    /// Compute similarity score between this entry and a query.
    pub fn similarity(&self, query_tokens: &HashSet<String>) -> f64 {
        if query_tokens.is_empty() {
            return 0.0;
        }

        // Name overlap (weighted at WEIGHT_NAME)
        let name_tokens = tokenize(&self.name);
        let name_overlap = if name_tokens.is_empty() {
            0.0
        } else {
            let intersect = name_tokens.intersection(query_tokens).count() as f64;
            let union = name_tokens.union(query_tokens).count() as f64;
            if union > 0.0 {
                intersect / union
            } else {
                0.0
            }
        };

        // Description overlap (weighted at WEIGHT_DESCRIPTION)
        let desc_tokens = tokenize(&self.description);
        let desc_overlap = if desc_tokens.is_empty() {
            0.0
        } else {
            let intersect = desc_tokens.intersection(query_tokens).count() as f64;
            let union = desc_tokens.union(query_tokens).count() as f64;
            if union > 0.0 {
                intersect / union
            } else {
                0.0
            }
        };

        // Runtime score (weighted at WEIGHT_RUNTIME) — only contributes when
        // there is meaningful semantic overlap, otherwise it would inflate
        // scores for unrelated skills.
        let has_semantic_overlap = name_overlap > 0.0 || desc_overlap > 0.0;
        let runtime = if has_semantic_overlap {
            self.score
        } else {
            0.0
        };

        // Weighted composite
        name_overlap * WEIGHT_NAME + desc_overlap * WEIGHT_DESCRIPTION + runtime * WEIGHT_RUNTIME
    }
}

// ---------------------------------------------------------------------------
// CachedResult
// ---------------------------------------------------------------------------

/// A cached discovery result with TTL.
#[derive(Debug, Clone)]
struct CachedResult {
    results: Vec<ScoredSkill>,
    expires_at: Instant,
}

/// A scored skill match returned from discovery.
#[derive(Debug, Clone, Serialize)]
pub struct ScoredSkill {
    pub name: String,
    pub description: String,
    pub score: f64,
    pub input_schema: Value,
    pub total_calls: u64,
    pub success_calls: u64,
    pub failure_calls: u64,
    pub average_latency_ms: f64,
}

// ---------------------------------------------------------------------------
// SkillIndex
// ---------------------------------------------------------------------------

/// In-memory index of registered skills with token-based similarity search.
#[derive(Debug, Clone)]
pub struct SkillIndex {
    entries: Vec<SkillIndexEntry>,
    last_built: Instant,
}

impl SkillIndex {
    /// Create an empty index.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            last_built: Instant::now(),
        }
    }

    /// Build the index from a skill registry.
    pub fn build(&mut self, registry: &SkillRegistry) {
        self.entries = registry
            .list()
            .into_iter()
            .map(|desc| SkillIndexEntry::from_descriptor(&desc))
            .collect();
        self.last_built = Instant::now();
        tracing::debug!("SkillIndex rebuilt with {} entries", self.entries.len());
    }

    /// Search for skills matching the query, returning top-k results.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<ScoredSkill> {
        if query.is_empty() || self.entries.is_empty() {
            return Vec::new();
        }

        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<ScoredSkill> = self
            .entries
            .iter()
            .map(|entry| {
                let score = entry.similarity(&query_tokens);
                ScoredSkill {
                    name: entry.name.clone(),
                    description: entry.description.clone(),
                    score: (score * 100.0).round() / 100.0,
                    input_schema: Value::Object(serde_json::Map::new()),
                    total_calls: 0,
                    success_calls: 0,
                    failure_calls: 0,
                    average_latency_ms: 0.0,
                }
            })
            .filter(|s| s.score >= MIN_MATCH_SCORE)
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        scored
    }

    /// Number of entries in the index.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SkillIndex {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SkillDiscovery
// ---------------------------------------------------------------------------

/// Wraps `SkillIndex` and provides task-to-skill matching with caching.
///
/// Maintains a result cache keyed by query text. Cache entries expire
/// after `CACHE_TTL` (5 minutes). The index is lazily rebuilt from the
/// registry when `discover()` is called and the index is empty or stale.
pub struct SkillDiscovery {
    index: SkillIndex,
    cache: HashMap<String, CachedResult>,
    /// FIFO queue tracking insertion order for cache eviction.
    insertion_order: VecDeque<String>,
    cache_ttl: Duration,
    max_cache_entries: usize,
    /// Optional reference to the skill registry for index rebuilds.
    registry_ref: Option<Arc<RwLock<SkillRegistry>>>,
}

impl SkillDiscovery {
    /// Create a new discovery engine.
    pub fn new() -> Self {
        Self {
            index: SkillIndex::new(),
            cache: HashMap::new(),
            insertion_order: VecDeque::new(),
            cache_ttl: CACHE_TTL,
            max_cache_entries: MAX_CACHE_ENTRIES,
            registry_ref: None,
        }
    }

    /// Set the skill registry reference used for index rebuilds.
    ///
    /// Call this during server startup so that `discover()` can
    /// rebuild the index from the live registry when needed.
    pub fn set_registry(&mut self, registry: Arc<RwLock<SkillRegistry>>) {
        self.registry_ref = Some(registry);
    }

    /// Discover skills matching the query, using the registry for runtime data.
    ///
    /// Returns scored results sorted by relevance (highest first).
    pub fn discover(
        &mut self,
        query: &str,
        top_k: usize,
        registry: &SkillRegistry,
    ) -> Vec<ScoredSkill> {
        // Rebuild index if empty
        if self.index.is_empty() {
            self.index.build(registry);
        }

        // Check cache
        let cache_key = format!("{}:{}", query.to_ascii_lowercase(), top_k);
        if let Some(cached) = self.cache.get(&cache_key) {
            if cached.expires_at > Instant::now() {
                return cached.results.clone();
            }
        }

        // Search
        let mut results = self.index.search(query, top_k);

        // Enrich with runtime stats from the registry
        for result in &mut results {
            if let Some(desc) = registry.list().into_iter().find(|d| d.name == result.name) {
                result.total_calls = desc.total_calls;
                result.success_calls = desc.success_calls;
                result.failure_calls = desc.failure_calls;
                result.average_latency_ms = desc.average_latency_ms;
                result.input_schema = desc.input_schema.clone();

                // Re-score with actual runtime stats
                let entry = SkillIndexEntry::from_descriptor(&desc);
                let query_tokens = tokenize(query);
                if !query_tokens.is_empty() {
                    result.score = (entry.similarity(&query_tokens) * 100.0).round() / 100.0;
                }
            }
        }

        // Cache the result
        self.evict_if_full();
        self.insertion_order.push_back(cache_key.clone());
        self.cache.insert(
            cache_key,
            CachedResult {
                results: results.clone(),
                expires_at: Instant::now() + self.cache_ttl,
            },
        );

        results
    }

    /// Evict oldest cache entry if at capacity.
    fn evict_if_full(&mut self) {
        while self.cache.len() >= self.max_cache_entries {
            if let Some(oldest_key) = self.insertion_order.pop_front() {
                self.cache.remove(&oldest_key);
            } else {
                break;
            }
        }
    }
}

impl Default for SkillDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Tokenize a string into lowercase word tokens, filtering short/common words.
fn tokenize(text: &str) -> HashSet<String> {
    let stop_words: HashSet<&str> = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "can", "shall",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through",
        "during", "before", "after", "above", "below", "between", "out", "off", "over", "under",
        "again", "further", "then", "once", "here", "there", "when", "where", "why", "how", "all",
        "each", "every", "both", "few", "more", "most", "other", "some", "such", "no", "nor",
        "not", "only", "own", "same", "so", "than", "too", "very", "just", "because", "but", "and",
        "or", "if", "while", "that", "this", "these", "those", "it", "its",
    ]
    .into_iter()
    .collect();

    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 3 && !stop_words.contains(w))
        .map(|w| w.to_ascii_lowercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(name: &str, desc: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: desc.to_string(),
            input_schema: Value::Object(serde_json::Map::new()),
            score: 0.5,
            total_calls: 10,
            success_calls: 8,
            failure_calls: 2,
            average_latency_ms: 100.0,
        }
    }

    #[test]
    fn test_tokenize_filters_stop_words() {
        let tokens = tokenize("the quick brown fox jumps over the lazy dog");
        assert!(!tokens.contains("the"));
        assert!(tokens.contains("quick"));
        assert!(tokens.contains("brown"));
        assert!(tokens.contains("lazy"));
    }

    #[test]
    fn test_tokenize_removes_punctuation() {
        let tokens = tokenize("hello, world! test-case");
        assert!(tokens.contains("hello"));
        assert!(tokens.contains("world"));
        assert!(tokens.contains("test"));
        assert!(tokens.contains("case"));
    }

    #[test]
    fn test_skill_index_search_finds_matching() {
        let mut index = SkillIndex::new();
        let mut skills = Vec::new();

        let desc = make_skill("code-fixer", "Fixes bugs in source code");
        skills.push(desc);

        // Re-create a minimal SkillRegistry with just enough for the test
        // We add entries directly to the index since we can't easily register without EchoSkill
        let entry = SkillIndexEntry::from_descriptor(&skills[0]);
        index.entries.push(entry);

        let results = index.search("fix code bugs", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "code-fixer");
    }

    #[test]
    fn test_skill_index_search_empty_query() {
        let mut index = SkillIndex::new();

        let desc = make_skill("test-skill", "A test skill");
        let entry = SkillIndexEntry::from_descriptor(&desc);
        index.entries.push(entry);

        let results = index.search("", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_skill_index_search_no_match() {
        let index = SkillIndex::new();
        let results = index.search("something completely unrelated", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_skill_discovery_cache() {
        let mut discovery = SkillDiscovery::new();
        let registry = SkillRegistry::default();
        let desc = make_skill("cache-test", "Testing cache behavior");
        let entry = SkillIndexEntry::from_descriptor(&desc);
        discovery.index.entries.push(entry);

        // First call populates cache
        let r1 = discovery.discover("cache test", 5, &registry);
        assert_eq!(r1.len(), 1);

        // Second call should use cache
        let r2 = discovery.discover("cache test", 5, &registry);
        assert_eq!(r2.len(), 1);
    }

    #[test]
    fn test_tokenize_short_words() {
        let tokens = tokenize("a an the of in");
        for t in &tokens {
            assert!(t.len() > 2, "short word '{}' should be filtered", t);
        }
    }

    #[test]
    fn test_entry_similarity_exact_match() {
        let desc = make_skill("code-formatter", "Formats source code");
        let entry = SkillIndexEntry::from_descriptor(&desc);
        let query = tokenize("code formatter");
        let score = entry.similarity(&query);
        assert!(score > 0.0);
    }

    #[test]
    fn test_entry_similarity_no_match() {
        let desc = make_skill("weather-report", "Provides weather data");
        let entry = SkillIndexEntry::from_descriptor(&desc);
        let query = tokenize("database query performance");
        let score = entry.similarity(&query);
        // No semantic overlap nor runtime contribution (has_semantic_overlap=false)
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_rebuild_index() {
        let mut index = SkillIndex::new();
        let registry = SkillRegistry::default();

        // Add entry directly
        let desc = make_skill("skill-one", "First skill");
        let entry = SkillIndexEntry::from_descriptor(&desc);
        index.entries.push(entry);

        assert_eq!(index.len(), 1);

        // Rebuild from registry (which is empty since we didn't register)
        index.build(&registry);
        assert_eq!(index.len(), 0);
    }
}
