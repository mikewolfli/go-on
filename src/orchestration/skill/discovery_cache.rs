//! Skill Discovery — caching layer over `SkillRegistry::discover_skills()`
//!
//! Provides a result cache for the `skill-finder` MCP tool to avoid
//! re-indexing skills on every query. The scoring logic is delegated
//! to `SkillRegistry::discover_skills()` which uses token-based similarity:
//!   - Name token overlap: 35%
//!   - Description token overlap: 40%
//!   - Runtime success rate: 25%
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::registry::SkillRegistry;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default TTL for cached discovery results (5 minutes).
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Maximum number of cached entries.
const MAX_CACHE_ENTRIES: usize = 200;

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
#[derive(Debug, Clone)]
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
// SkillDiscovery
// ---------------------------------------------------------------------------

/// Wraps `SkillRegistry::discover_skills()` with a result cache.
///
/// Maintains a result cache keyed by query text. Cache entries expire
/// after `CACHE_TTL` (5 minutes). The scoring logic is delegated to
/// `SkillRegistry::discover_skills()` to avoid duplicating the
/// token-based similarity index.
///
/// ## Integration
///
/// - `tools_pack.rs` uses `SkillDiscovery` for the `skill-finder` tool.
pub struct SkillDiscovery {
    cache: HashMap<String, CachedResult>,
    /// FIFO queue tracking insertion order for cache eviction.
    insertion_order: VecDeque<String>,
    cache_ttl: Duration,
    max_cache_entries: usize,
}

impl SkillDiscovery {
    /// Create a new discovery engine with result caching.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            insertion_order: VecDeque::new(),
            cache_ttl: CACHE_TTL,
            max_cache_entries: MAX_CACHE_ENTRIES,
        }
    }

    /// Discover skills matching the query, using the registry's built-in
    /// `discover_skills()` method for token-based similarity scoring.
    ///
    /// Results are cached with TTL for repeated queries. This is a thin
    /// caching layer over `SkillRegistry::discover_skills()`.
    pub fn discover(
        &mut self,
        query: &str,
        top_k: usize,
        registry: &SkillRegistry,
        _min_score: Option<f64>,
    ) -> Vec<ScoredSkill> {
        // Check cache first
        let cache_key = format!("{}:{}", query.to_ascii_lowercase(), top_k);
        if let Some(cached) = self.cache.get(&cache_key) {
            if cached.expires_at > Instant::now() {
                return cached.results.clone();
            }
        }

        // Delegate scoring to SkillRegistry's unified discover_skills()
        let descriptors = registry.discover_skills(query, top_k);
        let results: Vec<ScoredSkill> = descriptors
            .into_iter()
            .map(|desc| ScoredSkill {
                name: desc.name,
                description: desc.description,
                score: desc.score,
                input_schema: desc.input_schema,
                total_calls: desc.total_calls,
                success_calls: desc.success_calls,
                failure_calls: desc.failure_calls,
                average_latency_ms: desc.average_latency_ms,
            })
            .collect();

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::skill::registry::tokenize;
    use crate::orchestration::skill::SkillRegistry;

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
    fn test_discover_skills_finds_matching() {
        let mut registry = SkillRegistry::default();
        registry
            .register_builtin_skills()
            .expect("register builtins");

        let results = registry.discover_skills("create skill", 5);
        assert!(
            !results.is_empty(),
            "discover_skills should find create-skill"
        );
        assert!(results.iter().any(|s| s.name == "create-skill"));
    }

    #[test]
    fn test_discover_skills_empty_query_returns_sorted() {
        let mut registry = SkillRegistry::default();
        registry
            .register_builtin_skills()
            .expect("register builtins");

        let results = registry.discover_skills("", 5);
        // Empty query returns all skills sorted by name
        assert!(!results.is_empty());
        // Results should be sorted alphabetically
        for i in 1..results.len() {
            assert!(
                results[i - 1].name <= results[i].name,
                "results should be sorted by name"
            );
        }
    }

    #[test]
    fn test_discover_skills_no_match() {
        let registry = SkillRegistry::default();
        let results = registry.discover_skills("xyznonexistent12345", 5);
        // Should return empty for completely unrelated query
        assert!(results.is_empty());
    }

    #[test]
    fn test_tokenize_short_words() {
        let tokens = tokenize("a an the of in");
        for t in &tokens {
            assert!(t.len() > 2, "short word '{}' should be filtered", t);
        }
    }

    #[test]
    fn test_discovery_cache() {
        let mut discovery = SkillDiscovery::new();
        let registry = SkillRegistry::default();

        // First call populates cache
        let r1 = discovery.discover("echo", 5, &registry, None);

        // Second call should use cache (same results expected)
        let r2 = discovery.discover("echo", 5, &registry, None);
        assert_eq!(r1.len(), r2.len());
    }
}
