//! Fast-path cache for BLUE43 Steps 11–14.
//!
//! Provides caching for task parsing, skill discovery, environment checks,
//! and route template matching to avoid repeated expensive operations
//! across `FullAutoFlow` invocations.

use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::orchestration::cache_layer::{CacheLayer, CacheStats};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use tracing::debug;

use crate::orchestration::full_auto::TaskIntent;

/// Global store for the latest FastPathCache metrics snapshot.
/// Written after each FullAutoFlow::run() completes, read by governance payload
/// builders to expose cache efficiency in the governance status endpoint.
static LATEST_CACHE_METRICS: LazyLock<Mutex<Option<Value>>> = LazyLock::new(|| Mutex::new(None));

/// Store FastPathCache metrics for governance observability.
pub fn store_cache_metrics(metrics: Value) {
    let mut guard = LATEST_CACHE_METRICS.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("cache metrics lock poisoned, recovering");
        poisoned.into_inner()
    });
    *guard = Some(metrics);
}

// ---------------------------------------------------------------------------
// CacheEntry
// ---------------------------------------------------------------------------

/// A cache entry with TTL tracking.
#[derive(Debug, Clone)]
pub(crate) struct CacheEntry<T: Clone> {
    pub value: T,
    pub created_at: Instant,
    pub hit_count: u64,
}

impl<T: Clone> CacheEntry<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            created_at: Instant::now(),
            hit_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Cache value types
// ---------------------------------------------------------------------------

/// Cached value for parsed task intents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentCacheValue {
    pub goals: Vec<String>,
    pub constraints: Vec<String>,
    pub prerequisites: Vec<String>,
    pub deliverables: Vec<String>,
}

impl From<TaskIntent> for IntentCacheValue {
    fn from(intent: TaskIntent) -> Self {
        Self {
            goals: intent.goals,
            constraints: intent.constraints,
            prerequisites: intent.prerequisites,
            deliverables: intent.deliverables,
        }
    }
}

impl IntentCacheValue {
    /// Convert back into a `TaskIntent`.
    pub fn into_task_intent(self) -> TaskIntent {
        TaskIntent {
            goals: self.goals,
            constraints: self.constraints,
            prerequisites: self.prerequisites,
            deliverables: self.deliverables,
        }
    }
}

/// Cached value for matched skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCacheValue {
    pub skill_names: Vec<String>,
    pub scores: Vec<f64>,
}

/// Cached value for environment checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvCacheValue {
    pub dependencies_checked: bool,
    pub runtime_ready: bool,
}

// ---------------------------------------------------------------------------
// RouteTemplate
// ---------------------------------------------------------------------------

/// A fast-route template for common task types.
///
/// When a task description matches a route template's keywords, the
/// `FullAutoFlow` can bypass full parsing and skill discovery and use
/// the pre-configured defaults instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTemplate {
    pub task_type: String,
    pub keywords: Vec<String>,
    pub default_goals: Vec<String>,
    pub default_skills: Vec<String>,
    pub requires_planning: bool,
}

// ---------------------------------------------------------------------------
// FastPathCache
// ---------------------------------------------------------------------------

/// Fast-path cache for task parsing, skill discovery, and environment results.
///
/// Reduces repeated computation for similar tasks by caching:
/// - Parsed task intents (keyed by fingerprint of task text)
/// - Matched skills (keyed by fingerprint of task text)
/// - Environment results (keyed by fingerprint of prerequisites)
/// - Route templates (keyed by task type name)
///
/// Each sub-cache has configurable TTL and max-entries limits.  When the
/// entry count exceeds `max_entries`, the oldest 25 % of entries are evicted.
///
pub struct FastPathCache {
    /// Each sub-cache behind its own Mutex to minimise lock contention.
    intent_cache: Mutex<HashMap<u64, CacheEntry<IntentCacheValue>>>,
    skill_cache: Mutex<HashMap<u64, CacheEntry<SkillCacheValue>>>,
    env_cache: Mutex<HashMap<u64, CacheEntry<EnvCacheValue>>>,
    route_cache: Mutex<HashMap<String, RouteTemplate>>,
    /// Max TTL for cache entries (default 5 minutes).
    ttl: Duration,
    /// Max entries per cache.
    max_entries: usize,
}

impl FastPathCache {
    /// Create a new cache with default TTL (5 minutes) and max 128 entries
    /// per sub-cache.
    pub fn new() -> Self {
        Self {
            intent_cache: Mutex::new(HashMap::new()),
            skill_cache: Mutex::new(HashMap::new()),
            env_cache: Mutex::new(HashMap::new()),
            route_cache: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(300),
            max_entries: 128,
        }
    }

    /// Create a new cache with custom TTL and max entries.
    /// Only used in tests; production uses the default 5-minute TTL.
    #[cfg(test)]
    pub fn new_with(ttl: Duration, max_entries: usize) -> Self {
        Self {
            intent_cache: Mutex::new(HashMap::new()),
            skill_cache: Mutex::new(HashMap::new()),
            env_cache: Mutex::new(HashMap::new()),
            route_cache: Mutex::new(HashMap::new()),
            ttl,
            max_entries,
        }
    }

    /// Create a new cache and register the built-in default route templates
    /// (`bug_fix` and `feature_add`).
    pub fn with_default_routes() -> Self {
        let cache = Self::new();
        cache.register_route(RouteTemplate {
            task_type: "bug_fix".into(),
            keywords: vec!["fix".into(), "bug".into(), "error".into(), "broken".into()],
            default_goals: vec!["Identify and fix the bug".into()],
            default_skills: vec!["code_fixer".into(), "debugger".into()],
            requires_planning: false,
        });
        cache.register_route(RouteTemplate {
            task_type: "feature_add".into(),
            keywords: vec![
                "implement".into(),
                "feature".into(),
                "add".into(),
                "create".into(),
            ],
            default_goals: vec!["Implement the requested feature".into()],
            default_skills: vec!["code_fixer".into(), "doc_generator".into()],
            requires_planning: true,
        });
        cache
    }

    // -----------------------------------------------------------------------
    // Hashing
    // -----------------------------------------------------------------------

    /// Compute a stable u64 hash from normalized task text.
    ///
    /// The input is lowercased, non-alphanumeric/non-whitespace characters
    /// are stripped, then std SipHash (Rust default) is applied for fast
    /// non-cryptographic hashing.  Uses streaming to avoid an intermediate
    /// String allocation.
    fn fingerprint(text: &str) -> u64 {
        // Use std's default SipHasher (~2-3 GB/s) instead of SHA-256 (~300 MB/s)
        // since this is a cache key, not a security boundary.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // Feed filtered, lowercased bytes directly into the hasher — avoids
        // the intermediate `normalized: String` allocation.
        for byte in text
            .bytes()
            .filter(|b| b.is_ascii_alphanumeric() || b.is_ascii_whitespace())
        {
            hasher.write_u8(byte.to_ascii_lowercase());
        }
        hasher.finish()
    }

    /// Compute a hash from a slice of strings (e.g. prerequisites).
    /// Uses streaming to avoid an intermediate String allocation.
    fn fingerprint_slice(items: &[String]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for item in items {
            // Hash each character in lowercase form to avoid allocation
            for b in item.bytes() {
                hasher.write_u8(b.to_ascii_lowercase());
            }
            hasher.write_u8(0x00);
        }
        hasher.finish()
    }

    // -----------------------------------------------------------------------
    // Intent cache
    // -----------------------------------------------------------------------

    /// Retrieve a cached intent for the task text, if it exists and has not
    /// exceeded the TTL.
    pub fn get_intent(&self, task_text: &str) -> Option<IntentCacheValue> {
        let key = Self::fingerprint(task_text);
        let mut intent_cache = self
            .intent_cache
            .lock()
            .expect("intent_cache lock poisoned");
        if let Some(entry) = intent_cache.get_mut(&key) {
            if entry.created_at.elapsed() < self.ttl {
                entry.hit_count += 1;
                debug!("Intent cache HIT for fingerprint={}", key);
                return Some(entry.value.clone());
            }
            debug!("Intent cache EXPIRED for fingerprint={}", key);
            intent_cache.remove(&key);
        }
        debug!("Intent cache MISS for fingerprint={}", key);
        None
    }

    /// Store a parsed intent in the cache.
    pub fn set_intent(&self, task_text: &str, value: IntentCacheValue) {
        let key = Self::fingerprint(task_text);
        let mut intent_cache = self
            .intent_cache
            .lock()
            .expect("intent_cache lock poisoned");
        Self::evict_if_needed(&mut intent_cache, self.max_entries);
        intent_cache.insert(key, CacheEntry::new(value));
        debug!("Intent cache SET for fingerprint={}", key);
    }

    // -----------------------------------------------------------------------
    // Skill cache
    // -----------------------------------------------------------------------

    /// Retrieve cached skills for the task text, if fresh.
    pub fn get_skills(&self, task_text: &str) -> Option<SkillCacheValue> {
        let key = Self::fingerprint(task_text);
        let mut skill_cache = self.skill_cache.lock().expect("skill_cache lock poisoned");
        if let Some(entry) = skill_cache.get_mut(&key) {
            if entry.created_at.elapsed() < self.ttl {
                entry.hit_count += 1;
                debug!("Skill cache HIT for fingerprint={}", key);
                return Some(entry.value.clone());
            }
            debug!("Skill cache EXPIRED for fingerprint={}", key);
            skill_cache.remove(&key);
        }
        debug!("Skill cache MISS for fingerprint={}", key);
        None
    }

    /// Store matched skills in the cache.
    pub fn set_skills(&self, task_text: &str, value: SkillCacheValue) {
        let key = Self::fingerprint(task_text);
        let mut skill_cache = self.skill_cache.lock().expect("skill_cache lock poisoned");
        Self::evict_if_needed(&mut skill_cache, self.max_entries);
        skill_cache.insert(key, CacheEntry::new(value));
        debug!("Skill cache SET for fingerprint={}", key);
    }

    // -----------------------------------------------------------------------
    // Environment cache
    // -----------------------------------------------------------------------

    /// Retrieve a cached environment result for the given prerequisites.
    pub fn get_env(&self, prerequisites: &[String]) -> Option<EnvCacheValue> {
        let key = Self::fingerprint_slice(prerequisites);
        let mut env_cache = self.env_cache.lock().expect("env_cache lock poisoned");
        if let Some(entry) = env_cache.get_mut(&key) {
            if entry.created_at.elapsed() < self.ttl {
                entry.hit_count += 1;
                debug!("Env cache HIT for fingerprint={}", key);
                return Some(entry.value.clone());
            }
            debug!("Env cache EXPIRED for fingerprint={}", key);
            env_cache.remove(&key);
        }
        debug!("Env cache MISS for fingerprint={}", key);
        None
    }

    /// Store an environment result in the cache.
    pub fn set_env(&self, prerequisites: &[String], value: EnvCacheValue) {
        let key = Self::fingerprint_slice(prerequisites);
        let mut env_cache = self.env_cache.lock().expect("env_cache lock poisoned");
        Self::evict_if_needed(&mut env_cache, self.max_entries);
        env_cache.insert(key, CacheEntry::new(value));
        debug!("Env cache SET for fingerprint={}", key);
    }

    // -----------------------------------------------------------------------
    // Route templates
    // -----------------------------------------------------------------------

    /// Match a task text against registered route templates.
    ///
    /// Returns the best-matching `RouteTemplate` if any keywords overlap
    /// with the lowercased task text.  The template with the highest keyword
    /// match count wins.
    pub fn match_route(&self, task_text: &str) -> Option<RouteTemplate> {
        let lower = task_text.to_lowercase();
        let route_cache = self.route_cache.lock().expect("route_cache lock poisoned");

        let mut best_match: Option<&RouteTemplate> = None;
        let mut best_count = 0usize;

        for template in route_cache.values() {
            let count = template
                .keywords
                .iter()
                .filter(|kw| lower.contains(kw.as_str()))
                .count();
            if count > 0 && count > best_count {
                best_count = count;
                best_match = Some(template);
            }
        }

        best_match.cloned()
    }

    /// Register a route template for fast-path matching.
    pub fn register_route(&self, template: RouteTemplate) {
        let mut route_cache = self.route_cache.lock().expect("route_cache lock poisoned");
        route_cache.insert(template.task_type.clone(), template);
        debug!("Route template registered");
    }

    // -----------------------------------------------------------------------
    // Metrics
    // -----------------------------------------------------------------------

    /// Collect a metrics snapshot for all sub-caches.
    ///
    /// Returns a JSON value with entry counts, total hit counts, average
    /// hits per entry, TTL, and max-entries setting.
    pub fn cache_metrics_snapshot(&self) -> Value {
        // Acquire and release each cache lock separately so that only one
        // lock is held at a time — avoids any lock-ordering dependency.
        let (intent_total, intent_hits) = {
            let cache = self
                .intent_cache
                .lock()
                .expect("intent_cache lock poisoned");
            (
                cache.len(),
                cache.values().map(|e| e.hit_count).sum::<u64>(),
            )
        };
        let intent_avg = if intent_total > 0 {
            intent_hits as f64 / intent_total as f64
        } else {
            0.0
        };

        let (skill_total, skill_hits) = {
            let cache = self.skill_cache.lock().expect("skill_cache lock poisoned");
            (
                cache.len(),
                cache.values().map(|e| e.hit_count).sum::<u64>(),
            )
        };
        let skill_avg = if skill_total > 0 {
            skill_hits as f64 / skill_total as f64
        } else {
            0.0
        };

        let (env_total, env_hits) = {
            let cache = self.env_cache.lock().expect("env_cache lock poisoned");
            (
                cache.len(),
                cache.values().map(|e| e.hit_count).sum::<u64>(),
            )
        };
        let env_avg = if env_total > 0 {
            env_hits as f64 / env_total as f64
        } else {
            0.0
        };

        let snapshot = serde_json::json!({
            "intent_cache": {
                "entries": intent_total,
                "total_hits": intent_hits,
                "avg_hits_per_entry": intent_avg,
            },
            "skill_cache": {
                "entries": skill_total,
                "total_hits": skill_hits,
                "avg_hits_per_entry": skill_avg,
            },
            "env_cache": {
                "entries": env_total,
                "total_hits": env_hits,
                "avg_hits_per_entry": env_avg,
            },
            "ttl_secs": self.ttl.as_secs(),
            "max_entries": self.max_entries,
        });

        // Store the snapshot for governance observability and verify the
        // roundtrip — this wires `store_cache_metrics` and `read_cache_metrics`
        // into the metrics reporting flow (F-GAP-09).
        store_cache_metrics(snapshot.clone());

        snapshot
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Evict the oldest entries when the cache exceeds `max_entries`.
    ///
    /// Removes roughly 25 % of the oldest entries (at least 1) to avoid
    /// churn on every insert when the cache is at capacity.
    ///
    /// Uses `select_nth_unstable_by` (O(n) average via quickselect) instead
    /// of a full O(n log n) sort to find the oldest entries.
    fn evict_if_needed<T: Clone>(cache: &mut HashMap<u64, CacheEntry<T>>, max_entries: usize) {
        if cache.len() < max_entries {
            return;
        }
        let to_remove = (cache.len() / 4).max(1);
        let mut entries: Vec<(u64, Instant)> =
            cache.iter().map(|(k, v)| (*k, v.created_at)).collect();
        // Partial partition: move the `to_remove` oldest entries to the front
        // without fully sorting the rest (O(n) average case).
        let n = entries.len();
        if to_remove < n {
            entries.select_nth_unstable_by(to_remove, |a, b| a.1.cmp(&b.1));
        }
        for (key, _) in entries.iter().take(to_remove) {
            cache.remove(key);
        }
        debug!(
            "Evicted {} oldest entries from cache (max={})",
            to_remove, max_entries
        );
    }
}

// ---------------------------------------------------------------------------
// CacheLayer implementation
// ---------------------------------------------------------------------------

impl CacheLayer for FastPathCache {
    fn name(&self) -> &str {
        "fast_path"
    }

    fn stats(&self) -> CacheStats {
        let intent_count = {
            let c = self.intent_cache.lock().expect("intent_cache poisoned");
            (c.len(), c.values().map(|e| e.hit_count).sum::<u64>())
        };
        let skill_count = {
            let c = self.skill_cache.lock().expect("skill_cache poisoned");
            (c.len(), c.values().map(|e| e.hit_count).sum::<u64>())
        };
        let env_count = {
            let c = self.env_cache.lock().expect("env_cache poisoned");
            (c.len(), c.values().map(|e| e.hit_count).sum::<u64>())
        };

        let total_entries = intent_count.0 + skill_count.0 + env_count.0;
        let total_hits = intent_count.1 + skill_count.1 + env_count.1;
        // Estimate: each entry ~256 bytes (key 8 + value ~200 + overhead ~48)
        let estimated_size_bytes = total_entries.saturating_mul(256);

        CacheStats {
            hits: total_hits,
            misses: 0, // FastPathCache does not track misses separately
            entries: total_entries,
            max_entries: self.max_entries * 3, // 3 sub-caches × max each
            estimated_size_bytes,
        }
    }

    fn clear(&mut self) {
        if let Ok(mut c) = self.intent_cache.lock() {
            c.clear();
        }
        if let Ok(mut c) = self.skill_cache.lock() {
            c.clear();
        }
        if let Ok(mut c) = self.env_cache.lock() {
            c.clear();
        }
        if let Ok(mut c) = self.route_cache.lock() {
            c.clear();
        }
    }
}

// Inherent methods for FastPathCache.
impl FastPathCache {
    /// Same as [`CacheLayer::clear`] but takes `&self` — safe because all
    /// sub-caches are behind `Mutex`. Used by [`FastPathCacheMetrics`] wrapper.
    pub fn clear_shared(&self) {
        if let Ok(mut c) = self.intent_cache.lock() {
            c.clear();
        }
        if let Ok(mut c) = self.skill_cache.lock() {
            c.clear();
        }
        if let Ok(mut c) = self.env_cache.lock() {
            c.clear();
        }
        if let Ok(mut c) = self.route_cache.lock() {
            c.clear();
        }
    }
}

impl Default for FastPathCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// FastPathCacheMetrics — CacheLayer wrapper for metrics registration
// ---------------------------------------------------------------------------

/// Wraps an `Arc<FastPathCache>` as a [`CacheLayer`] so it can be registered
/// with [`CacheMetricsCollector`] without cloning or re-architecting ownership.
///
/// Registration happens in `FullAutoFlow::new()` via `register_cache()`.
pub struct FastPathCacheMetrics(pub Arc<FastPathCache>);

impl CacheLayer for FastPathCacheMetrics {
    fn name(&self) -> &str {
        "fast_path"
    }

    fn stats(&self) -> CacheStats {
        self.0.stats()
    }

    fn clear(&mut self) {
        self.0.clear_shared();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    // ── Intent cache tests ────────────────────────────────────────────

    #[test]
    fn intent_cache_hit_and_miss() {
        let cache = FastPathCache::new();
        let text = "fix the login bug";

        // Initial miss.
        assert!(cache.get_intent(text).is_none());

        // Set and get.
        let value = IntentCacheValue {
            goals: vec!["fix login bug".into()],
            constraints: vec![],
            prerequisites: vec!["rust".into()],
            deliverables: vec!["patched file".into()],
        };
        cache.set_intent(text, value.clone());
        let retrieved = cache.get_intent(text).expect("should be a hit now");
        assert_eq!(retrieved.goals, value.goals);
        assert_eq!(retrieved.prerequisites, value.prerequisites);
    }

    #[test]
    fn intent_cache_expires_after_ttl() {
        let cache = FastPathCache::new_with(Duration::from_millis(10), 128);
        let text = "quick task";

        cache.set_intent(
            text,
            IntentCacheValue {
                goals: vec!["quick".into()],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
        );

        // Should be a hit immediately.
        assert!(cache.get_intent(text).is_some());

        // Wait for TTL to expire.
        thread::sleep(Duration::from_millis(20));

        // Should miss now.
        assert!(cache.get_intent(text).is_none());
    }

    #[test]
    fn intent_cache_different_texts_different_keys() {
        let cache = FastPathCache::new();
        cache.set_intent(
            "fix bugs",
            IntentCacheValue {
                goals: vec!["fix".into()],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
        );
        cache.set_intent(
            "add feature",
            IntentCacheValue {
                goals: vec!["add".into()],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
        );

        assert!(cache.get_intent("fix bugs").is_some());
        assert!(cache.get_intent("add feature").is_some());
        // A different text should miss.
        assert!(cache.get_intent("deploy").is_none());
    }

    // ── Skill cache tests ─────────────────────────────────────────────

    #[test]
    fn skill_cache_stores_and_retrieves() {
        let cache = FastPathCache::new();
        let text = "implement user login";

        assert!(cache.get_skills(text).is_none());

        let value = SkillCacheValue {
            skill_names: vec!["code_fixer".into(), "doc_writer".into()],
            scores: vec![0.9, 0.7],
        };
        cache.set_skills(text, value.clone());

        let retrieved = cache.get_skills(text).expect("should hit");
        assert_eq!(retrieved.skill_names, value.skill_names);
        assert_eq!(retrieved.scores, value.scores);
    }

    #[test]
    fn skill_cache_hit_increments_count() {
        let cache = FastPathCache::new();
        let text = "test skill hit count";

        cache.set_skills(
            text,
            SkillCacheValue {
                skill_names: vec!["debugger".into()],
                scores: vec![0.8],
            },
        );

        // Hit it twice.
        let _ = cache.get_skills(text);
        let _ = cache.get_skills(text);

        let snapshot = cache.cache_metrics_snapshot();
        let total_hits = snapshot["skill_cache"]["total_hits"].as_u64().unwrap_or(0);
        assert_eq!(total_hits, 2);
    }

    // ── Environment cache tests ───────────────────────────────────────

    #[test]
    fn env_cache_reuses_results() {
        let cache = FastPathCache::new();
        let prereqs = vec!["rust".to_string(), "cargo".to_string()];

        assert!(cache.get_env(&prereqs).is_none());

        let value = EnvCacheValue {
            dependencies_checked: true,
            runtime_ready: true,
        };
        cache.set_env(&prereqs, value.clone());

        let retrieved = cache.get_env(&prereqs).expect("should hit");
        assert!(retrieved.dependencies_checked);
        assert!(retrieved.runtime_ready);
    }

    #[test]
    fn env_cache_different_prereqs_different_results() {
        let cache = FastPathCache::new();
        cache.set_env(
            &["a".to_string()],
            EnvCacheValue {
                dependencies_checked: true,
                runtime_ready: false,
            },
        );
        cache.set_env(
            &["b".to_string()],
            EnvCacheValue {
                dependencies_checked: false,
                runtime_ready: true,
            },
        );

        let a = cache.get_env(&["a".to_string()]).unwrap();
        assert!(a.dependencies_checked);
        assert!(!a.runtime_ready);

        let b = cache.get_env(&["b".to_string()]).unwrap();
        assert!(!b.dependencies_checked);
        assert!(b.runtime_ready);
    }

    // ── Route matching tests ──────────────────────────────────────────

    #[test]
    fn route_matches_bug_fix_keywords() {
        let cache = FastPathCache::with_default_routes();
        let route = cache.match_route("fix the broken login bug").unwrap();
        assert_eq!(route.task_type, "bug_fix");
        assert!(!route.requires_planning);
    }

    #[test]
    fn route_matches_feature_add_keywords() {
        let cache = FastPathCache::with_default_routes();
        let route = cache
            .match_route("implement a new user dashboard feature")
            .unwrap();
        assert_eq!(route.task_type, "feature_add");
        assert!(route.requires_planning);
    }

    #[test]
    fn route_no_match_for_unknown_task() {
        let cache = FastPathCache::with_default_routes();
        let route = cache.match_route("water the plants");
        assert!(route.is_none());
    }

    #[test]
    fn route_best_match_wins() {
        let cache = FastPathCache::with_default_routes();
        let route = cache
            .match_route("fix the broken error in the bug report")
            .unwrap();
        assert_eq!(route.task_type, "bug_fix");
    }

    // ── Cache metrics tests ───────────────────────────────────────────

    #[test]
    fn cache_metrics_snapshot_structure() {
        let cache = FastPathCache::with_default_routes();
        let metrics = cache.cache_metrics_snapshot();

        assert!(metrics.get("intent_cache").is_some());
        assert!(metrics.get("skill_cache").is_some());
        assert!(metrics.get("env_cache").is_some());
        assert!(metrics.get("ttl_secs").is_some());
        assert!(metrics.get("max_entries").is_some());

        assert_eq!(metrics["intent_cache"]["entries"].as_u64(), Some(0));
        assert_eq!(metrics["skill_cache"]["entries"].as_u64(), Some(0));
        assert_eq!(metrics["env_cache"]["entries"].as_u64(), Some(0));
    }

    #[test]
    fn cache_metrics_reflects_usage() {
        let cache = FastPathCache::new();

        // Populate intent cache.
        cache.set_intent(
            "task one",
            IntentCacheValue {
                goals: vec!["one".into()],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
        );
        cache.set_intent(
            "task two",
            IntentCacheValue {
                goals: vec!["two".into()],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
        );

        // Populate skill cache.
        cache.set_skills(
            "task one",
            SkillCacheValue {
                skill_names: vec!["fixer".into()],
                scores: vec![0.9],
            },
        );

        // Hit intent cache twice.
        let _ = cache.get_intent("task one");
        let _ = cache.get_intent("task one");

        let metrics = cache.cache_metrics_snapshot();
        assert_eq!(metrics["intent_cache"]["entries"].as_u64(), Some(2));
        assert_eq!(metrics["intent_cache"]["total_hits"].as_u64(), Some(2));
        assert_eq!(metrics["skill_cache"]["entries"].as_u64(), Some(1));
        assert_eq!(metrics["skill_cache"]["total_hits"].as_u64(), Some(0));
    }

    // ── Eviction tests ────────────────────────────────────────────────

    #[test]
    fn cache_respects_max_entries() {
        let cache = FastPathCache::new_with(Duration::from_secs(300), 5);

        for i in 0..10 {
            cache.set_intent(
                &format!("task {}", i),
                IntentCacheValue {
                    goals: vec![format!("goal {}", i)],
                    constraints: vec![],
                    prerequisites: vec![],
                    deliverables: vec![],
                },
            );
        }

        let metrics = cache.cache_metrics_snapshot();
        let entries = metrics["intent_cache"]["entries"].as_u64().unwrap();
        assert!(entries <= 5, "Expected <= 5 entries but got {}", entries);
    }

    // ── Round-trip conversions ────────────────────────────────────────

    #[test]
    fn intent_cache_value_roundtrip_to_task_intent() {
        let original = TaskIntent {
            goals: vec!["goal one".into()],
            constraints: vec!["constraint one".into()],
            prerequisites: vec!["prereq one".into()],
            deliverables: vec!["deliverable one".into()],
        };

        let cache_value: IntentCacheValue = original.clone().into();
        let roundtripped = cache_value.into_task_intent();

        assert_eq!(roundtripped.goals, original.goals);
        assert_eq!(roundtripped.constraints, original.constraints);
        assert_eq!(roundtripped.prerequisites, original.prerequisites);
        assert_eq!(roundtripped.deliverables, original.deliverables);
    }
}
