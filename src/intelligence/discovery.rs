//! F-GAP-11: Solution Discovery Center
//!
//! Centralized registry for discovering agent capabilities, solutions, and
//! patterns across the system.  Indexes solutions by problem pattern, success
//! rate, and applicability.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tracing;

use crate::intelligence::now_ms;

// ── ID generation ────────────────────────────────────────────────────────────

static DISCOVERY_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_id() -> String {
    let n = DISCOVERY_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("disc-{}", n)
}

// Use `crate::intelligence::now_ms()` instead — shared utility in mod.rs

// ── Data types ──────────────────────────────────────────────────────────────

/// A single problem-solution entry recorded in the discovery center.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryEntry {
    pub id: String,
    pub problem_pattern: String,
    pub solution_summary: String,
    pub solution_detail: serde_json::Value,
    pub applicability_tags: Vec<String>,
    pub success_rate: f64,
    pub total_attempts: u64,
    pub successful_attempts: u64,
    pub discovered_by: String,
    pub created_ms: u64,
    pub last_used_ms: u64,
}

/// A known solution pattern that can be registered and matched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionPattern {
    pub name: String,
    pub description: String,
    /// One of `"code"`, `"config"`, `"debug"`, `"test"`, `"deploy"`
    pub category: String,
    /// Complexity score in the range 0.0 – 1.0
    pub complexity: f64,
    pub tags: Vec<String>,
}

/// Aggregate profile information for the discovery center.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryProfile {
    pub enabled: bool,
    pub total_entries: u32,
    pub total_patterns: u32,
    pub categories: u32,
    pub avg_success_rate: f64,
    pub top_pattern: String,
}

/// Query used to search the discovery centre.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryQuery {
    pub problem_pattern: Option<String>,
    pub tags: Option<Vec<String>>,
    pub category: Option<String>,
    pub min_success_rate: Option<f64>,
    pub limit: Option<usize>,
}

/// Result container returned by a discovery search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResult {
    pub entries: Vec<DiscoveryEntry>,
    pub total_matches: usize,
    pub query_duration_ms: u64,
    pub best_match: Option<DiscoveryEntry>,
}

// ── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur during discovery operations.
#[derive(Debug, Clone)]
pub enum DiscoveryError {
    /// The provided pattern name already exists.
    DuplicatePattern(String),
    /// The entry id was not found.
    EntryNotFound(String),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicatePattern(name) => write!(f, "duplicate pattern: {name}"),
            Self::EntryNotFound(id) => write!(f, "entry not found: {id}"),
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Convenience result alias for discovery operations.
pub type Result<T> = std::result::Result<T, DiscoveryError>;

// ── Discovery Center ────────────────────────────────────────────────────────

/// Central registry for discovering agent capabilities, solutions and patterns.
pub struct DiscoveryCenter {
    /// Indexed problem-solution entries
    entries: Arc<Mutex<Vec<DiscoveryEntry>>>,
    /// Known solution patterns
    patterns: Arc<RwLock<HashMap<String, SolutionPattern>>>,
    /// Max entries to retain
    max_entries: usize,
    /// Profile metrics
    profile: Arc<Mutex<DiscoveryProfile>>,
}

impl DiscoveryCenter {
    /// Create a new `DiscoveryCenter` with default settings.
    ///
    /// The default maximum number of retained entries is 10 000.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            patterns: Arc::new(RwLock::new(HashMap::new())),
            max_entries: 10_000,
            profile: Arc::new(Mutex::new(DiscoveryProfile {
                enabled: true,
                total_entries: 0,
                total_patterns: 0,
                categories: 0,
                avg_success_rate: 0.0,
                top_pattern: String::new(),
            })),
        }
    }

    /// Register a new solution pattern.
    ///
    /// Returns `Err(DiscoveryError::DuplicatePattern)` when a pattern with the
    /// same name already exists.
    pub fn register_pattern(&self, pattern: SolutionPattern) -> Result<()> {
        let mut patterns = self
            .patterns
            .write()
            .map_err(|_| DiscoveryError::DuplicatePattern(pattern.name.clone()))?;

        if patterns.contains_key(&pattern.name) {
            return Err(DiscoveryError::DuplicatePattern(pattern.name));
        }

        patterns.insert(pattern.name.clone(), pattern);
        drop(patterns); // release write lock before refresh_profile
        self.refresh_profile();
        Ok(())
    }

    /// Record a problem-solution entry.
    ///
    /// Returns the auto-generated entry id on success.  When the centre is at
    /// capacity the oldest entry (by `last_used_ms`) is evicted first.
    pub fn record_solution(&self, entry: DiscoveryEntry) -> Result<String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| DiscoveryError::EntryNotFound("lock poisoned".to_string()))?;

        // Evict the least-recently-used entry when at capacity.
        if entries.len() >= self.max_entries {
            if let Some(pos) = entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used_ms)
                .map(|(i, _)| i)
            {
                entries.swap_remove(pos);
            }
        }

        let id = generate_id();
        let now = now_ms();
        let mut entry = entry;
        entry.id = id.clone();
        entry.created_ms = now;
        entry.last_used_ms = now;

        entries.push(entry);
        drop(entries); // release lock before refresh_profile
        self.refresh_profile();
        Ok(id)
    }

    /// Search for solutions matching the given query.
    ///
    /// When no limit is specified in the query, up to 20 results are returned.
    pub fn search(&self, query: &DiscoveryQuery) -> DiscoveryResult {
        let start = now_ms();

        let entries = match self.entries.lock() {
            Ok(e) => e.clone(),
            Err(_) => {
                tracing::warn!("DiscoveryCenter lock poisoned in search");
                return DiscoveryResult {
                    entries: vec![],
                    total_matches: 0,
                    query_duration_ms: now_ms().saturating_sub(start),
                    best_match: None,
                };
            }
        };

        let mut matches: Vec<DiscoveryEntry> = entries
            .into_iter()
            .filter(|e| {
                // Filter by problem pattern (substring match, case-insensitive)
                if let Some(ref pat) = query.problem_pattern {
                    if !e
                        .problem_pattern
                        .to_lowercase()
                        .contains(&pat.to_lowercase())
                    {
                        return false;
                    }
                }

                // Filter by tags (any overlap)
                if let Some(ref req_tags) = query.tags {
                    if !req_tags.is_empty()
                        && !req_tags.iter().any(|t| e.applicability_tags.contains(t))
                    {
                        return false;
                    }
                }

                // Filter by category (exact match, case-insensitive)
                if let Some(ref cat) = query.category {
                    if !e
                        .applicability_tags
                        .iter()
                        .any(|t| t.to_lowercase() == cat.to_lowercase())
                    {
                        return false;
                    }
                }

                // Filter by minimum success rate
                if let Some(min_rate) = query.min_success_rate {
                    if e.success_rate < min_rate {
                        return false;
                    }
                }

                true
            })
            .collect();

        // Sort by success rate descending, then by last_used_ms descending
        matches.sort_by(|a, b| {
            b.success_rate
                .partial_cmp(&a.success_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.last_used_ms.cmp(&a.last_used_ms))
        });

        let total_matches = matches.len();
        let limit = query.limit.unwrap_or(20).min(matches.len());
        let best_match = matches.first().cloned();

        matches.truncate(limit);

        DiscoveryResult {
            entries: matches,
            total_matches,
            query_duration_ms: now_ms().saturating_sub(start),
            best_match,
        }
    }

    /// Automatically extract and register solution patterns from high-performing entries.
    ///
    /// Scans all entries with success_rate above `min_success_rate`, clusters them
    /// by shared tags/problem_pattern, and creates/updates SolutionPattern entries
    /// for clusters that meet the `min_occurrences` threshold.
    pub fn extract_patterns(&self, min_success_rate: f64, min_occurrences: usize) -> Vec<String> {
        let entries = match self.entries.lock() {
            Ok(e) => e.clone(),
            Err(_) => {
                tracing::warn!("DiscoveryCenter lock poisoned in extract_patterns");
                return Vec::new();
            }
        };

        // Filter high-quality entries
        let candidates: Vec<_> = entries
            .iter()
            .filter(|e| e.success_rate >= min_success_rate && e.total_attempts > 0)
            .collect();

        if candidates.len() < min_occurrences {
            return Vec::new();
        }

        // Cluster by shared tags (entries sharing 2+ tags form a cluster)
        let mut clusters: Vec<Vec<&DiscoveryEntry>> = Vec::new();
        for candidate in &candidates {
            let mut added = false;
            for cluster in clusters.iter_mut() {
                let shared_tags: usize = cluster[0]
                    .applicability_tags
                    .iter()
                    .filter(|t| candidate.applicability_tags.contains(t))
                    .count();
                if shared_tags >= 2 {
                    cluster.push(candidate);
                    added = true;
                    break;
                }
            }
            if !added {
                clusters.push(vec![candidate]);
            }
        }

        // Generate patterns for clusters above threshold
        let mut generated: Vec<String> = Vec::new();
        for cluster in clusters.iter().filter(|c| c.len() >= min_occurrences) {
            // Find the most common problem pattern
            let mut pattern_counts: HashMap<&str, usize> = HashMap::new();
            let mut all_tags: Vec<&str> = Vec::new();
            for entry in cluster {
                *pattern_counts
                    .entry(entry.problem_pattern.as_str())
                    .or_insert(0) += 1;
                for tag in &entry.applicability_tags {
                    if !all_tags.contains(&tag.as_str()) {
                        all_tags.push(tag.as_str());
                    }
                }
            }
            let best_pattern = pattern_counts
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(p, _)| p.to_string())
                .unwrap_or_else(|| "auto_extracted".to_string());

            let avg_success =
                cluster.iter().map(|e| e.success_rate).sum::<f64>() / cluster.len() as f64;
            let category = cluster[0]
                .applicability_tags
                .first()
                .cloned()
                .unwrap_or_else(|| "general".to_string());

            let pattern = SolutionPattern {
                name: format!("auto_{}_{}", best_pattern, generated.len()),
                description: format!(
                    "Auto-extracted from {} entries (avg success: {:.2})",
                    cluster.len(),
                    avg_success
                ),
                category,
                complexity: 1.0 - avg_success,
                tags: all_tags.iter().map(|t| t.to_string()).collect(),
            };

            // Register if not duplicate
            let patterns = self.patterns.read().map(|p| p.clone()).unwrap_or_default();
            if !patterns.contains_key(&pattern.name) && self.register_pattern(pattern).is_ok() {
                generated.push(best_pattern);
            }
        }

        generated
    }

    /// Generate abstract knowledge by cross-referencing patterns across categories.
    ///
    /// Returns a human-readable summary of discovered cross-domain insights.
    ///
    /// TODO: Reserved for future cross-category insight mining. Currently unused.
    #[allow(dead_code)]
    pub fn abstract_knowledge(&self) -> Vec<String> {
        let patterns = match self.patterns.read() {
            Ok(p) => p.clone(),
            Err(_) => {
                tracing::warn!("DiscoveryCenter RwLock poisoned in abstract_knowledge");
                return Vec::new();
            }
        };

        let mut insights: Vec<String> = Vec::new();

        // Group patterns by category
        let mut by_category: HashMap<String, Vec<&SolutionPattern>> = HashMap::new();
        for pattern in patterns.values() {
            by_category
                .entry(pattern.category.clone())
                .or_default()
                .push(pattern);
        }

        // Cross-category insight: patterns with similar tags across categories
        let all_patterns: Vec<&SolutionPattern> = patterns.values().collect();
        for (i, pa) in all_patterns.iter().enumerate() {
            for pb in all_patterns.iter().skip(i + 1) {
                if pa.category != pb.category {
                    let shared: Vec<&String> =
                        pa.tags.iter().filter(|t| pb.tags.contains(t)).collect();
                    if shared.len() >= 2 {
                        insights.push(format!(
                            "Cross-domain insight: patterns '{}' ({}) and '{}' ({}) share tags: {:?}",
                            pa.name, pa.category, pb.name, pb.category, shared
                        ));
                    }
                }
            }
        }

        // Category-level abstraction
        for (category, pats) in &by_category {
            if pats.len() >= 3 {
                let avg_complexity: f64 =
                    pats.iter().map(|p| p.complexity).sum::<f64>() / pats.len() as f64;
                insights.push(format!(
                    "Category '{}' has {} patterns (avg complexity: {:.2})",
                    category,
                    pats.len(),
                    avg_complexity
                ));
            }
        }

        insights
    }

    /// Record the outcome of an attempt associated with a discovery entry.
    ///
    /// Updates the entry's success rate (`successful_attempts / total_attempts`)
    /// and its `last_used_ms` timestamp.
    pub fn record_outcome(&self, entry_id: &str, success: bool) {
        let mut entries = match self.entries.lock() {
            Ok(e) => e,
            Err(_) => {
                tracing::warn!("DiscoveryCenter lock poisoned in record_outcome");
                return;
            }
        };

        if let Some(entry) = entries.iter_mut().find(|e| e.id == entry_id) {
            entry.total_attempts = entry.total_attempts.saturating_add(1);
            if success {
                entry.successful_attempts = entry.successful_attempts.saturating_add(1);
            }
            entry.success_rate = if entry.total_attempts > 0 {
                entry.successful_attempts as f64 / entry.total_attempts as f64
            } else {
                0.0
            };
            entry.last_used_ms = now_ms();
            drop(entries); // release lock before refresh_profile
            self.refresh_profile();
        }
    }

    /// Return the most successful entries for the given category.
    ///
    /// Matching is based on the `applicability_tags` of each entry (a tag must
    /// contain the category string as a case-insensitive substring or exact
    /// match).  Results are sorted by success rate descending.
    pub fn most_successful(&self, category: &str, limit: usize) -> Vec<DiscoveryEntry> {
        let entries = match self.entries.lock() {
            Ok(e) => e.clone(),
            Err(_) => {
                tracing::warn!("DiscoveryCenter lock poisoned in most_successful");
                return vec![];
            }
        };

        let cat_lower = category.to_lowercase();
        let mut results: Vec<DiscoveryEntry> = entries
            .into_iter()
            .filter(|e| {
                e.applicability_tags
                    .iter()
                    .any(|t| t.to_lowercase().contains(&cat_lower))
            })
            .collect();

        results.sort_by(|a, b| {
            b.success_rate
                .partial_cmp(&a.success_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.last_used_ms.cmp(&a.last_used_ms))
        });

        results.truncate(limit);
        results
    }

    /// Return a snapshot of the current discovery centre profile.
    pub fn profile(&self) -> DiscoveryProfile {
        match self.profile.lock() {
            Ok(p) => p.clone(),
            Err(_) => {
                tracing::warn!("DiscoveryCenter lock poisoned in profile");
                DiscoveryProfile {
                    enabled: true,
                    total_entries: 0,
                    total_patterns: 0,
                    categories: 0,
                    avg_success_rate: 0.0,
                    top_pattern: String::new(),
                }
            }
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    /// Recompute the cached profile metrics from live data.
    fn refresh_profile(&self) {
        let entries = match self.entries.lock() {
            Ok(e) => e.clone(),
            Err(_) => {
                tracing::warn!("DiscoveryCenter lock poisoned in refresh_profile (entries)");
                return;
            }
        };
        let patterns = match self.patterns.read() {
            Ok(p) => p.clone(),
            Err(_) => {
                tracing::warn!("DiscoveryCenter RwLock poisoned in refresh_profile (patterns)");
                return;
            }
        };

        let total_entries = entries.len() as u32;
        let total_patterns = patterns.len() as u32;

        // Count unique categories from pattern list.
        let mut cat_set: Vec<String> = Vec::new();
        for p in patterns.values() {
            let cat_lower = p.category.to_lowercase();
            if !cat_set.iter().any(|c| c == &cat_lower) {
                cat_set.push(cat_lower);
            }
        }
        let categories = cat_set.len() as u32;

        let avg_success_rate = if !entries.is_empty() {
            entries.iter().map(|e| e.success_rate).sum::<f64>() / entries.len() as f64
        } else {
            0.0
        };

        let top_pattern = patterns
            .values()
            .max_by(|a, b| {
                a.complexity
                    .partial_cmp(&b.complexity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|p| p.name.clone())
            .unwrap_or_default();

        if let Ok(mut profile) = self.profile.lock() {
            profile.total_entries = total_entries;
            profile.total_patterns = total_patterns;
            profile.categories = categories;
            profile.avg_success_rate = avg_success_rate;
            profile.top_pattern = top_pattern;
        }
    }
}

impl Default for DiscoveryCenter {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(_n: u32) -> DiscoveryEntry {
        DiscoveryEntry {
            id: String::new(), // will be overwritten by record_solution
            problem_pattern: "test-failure".to_string(),
            solution_summary: "Retry with backoff".to_string(),
            solution_detail: serde_json::json!({"retry_delay_ms": 500}),
            applicability_tags: vec!["test".to_string(), "network".to_string()],
            success_rate: 0.0,
            total_attempts: 0,
            successful_attempts: 0,
            discovered_by: "test-agent".to_string(),
            created_ms: 0,
            last_used_ms: 0,
        }
    }

    #[test]
    fn test_new_center_is_empty() {
        let c = DiscoveryCenter::new();
        let p = c.profile();
        assert!(p.enabled);
        assert_eq!(p.total_entries, 0);
        assert_eq!(p.total_patterns, 0);
    }

    #[test]
    fn test_record_and_search() {
        let c = DiscoveryCenter::new();
        let id = c.record_solution(sample_entry(1)).expect("should record");
        assert!(id.starts_with("disc-"));

        let query = DiscoveryQuery {
            problem_pattern: Some("test".to_string()),
            tags: None,
            category: None,
            min_success_rate: None,
            limit: None,
        };
        let result = c.search(&query);
        assert_eq!(result.total_matches, 1);
        assert_eq!(result.entries.len(), 1);
        assert!(result.best_match.is_some());
    }

    #[test]
    fn test_search_filters_by_tags() {
        let c = DiscoveryCenter::new();
        c.record_solution(sample_entry(1)).unwrap();

        let query = DiscoveryQuery {
            problem_pattern: None,
            tags: Some(vec!["database".to_string()]),
            category: None,
            min_success_rate: None,
            limit: None,
        };
        let result = c.search(&query);
        assert_eq!(result.total_matches, 0);
    }

    #[test]
    fn test_search_filters_by_min_success_rate() {
        let c = DiscoveryCenter::new();
        let id = c.record_solution(sample_entry(1)).unwrap();
        c.record_outcome(&id, true); // 1/1 = 1.0

        let query = DiscoveryQuery {
            problem_pattern: None,
            tags: None,
            category: None,
            min_success_rate: Some(0.9),
            limit: None,
        };
        let result = c.search(&query);
        assert_eq!(result.total_matches, 1);
    }

    #[test]
    fn test_record_outcome_updates_success_rate() {
        let c = DiscoveryCenter::new();
        let id = c.record_solution(sample_entry(1)).unwrap();

        c.record_outcome(&id, true);
        c.record_outcome(&id, false);
        c.record_outcome(&id, true);

        let entries = c.entries.lock().unwrap();
        let entry = entries.iter().find(|e| e.id == id).unwrap();
        assert_eq!(entry.total_attempts, 3);
        assert_eq!(entry.successful_attempts, 2);
        assert!((entry.success_rate - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn test_most_successful() {
        let c = DiscoveryCenter::new();

        let mut e1 = sample_entry(1);
        e1.applicability_tags = vec!["deploy".to_string()];
        let id1 = c.record_solution(e1).unwrap();
        c.record_outcome(&id1, true);
        c.record_outcome(&id1, true);

        let mut e2 = sample_entry(2);
        e2.applicability_tags = vec!["deploy".to_string()];
        let id2 = c.record_solution(e2).unwrap();
        c.record_outcome(&id2, true);
        c.record_outcome(&id2, false);

        let top = c.most_successful("deploy", 10);
        assert_eq!(top.len(), 2);
        assert!(top[0].success_rate >= top[1].success_rate);
    }

    #[test]
    fn test_register_pattern() {
        let c = DiscoveryCenter::new();
        let pat = SolutionPattern {
            name: "retry-backoff".to_string(),
            description: "Exponential backoff retry".to_string(),
            category: "code".to_string(),
            complexity: 0.3,
            tags: vec!["retry".to_string(), "network".to_string()],
        };
        assert!(c.register_pattern(pat.clone()).is_ok());
        // Duplicate must fail.
        assert!(c.register_pattern(pat).is_err());
    }

    #[test]
    fn test_profile_reflects_state() {
        let c = DiscoveryCenter::new();
        let pat = SolutionPattern {
            name: "my-pattern".to_string(),
            description: "desc".to_string(),
            category: "debug".to_string(),
            complexity: 0.5,
            tags: vec![],
        };
        c.register_pattern(pat).unwrap();
        let id = c.record_solution(sample_entry(1)).unwrap();
        c.record_outcome(&id, true);

        let p = c.profile();
        assert_eq!(p.total_entries, 1);
        assert_eq!(p.total_patterns, 1);
        assert_eq!(p.categories, 1);
        assert!(p.avg_success_rate > 0.0);
        assert_eq!(p.top_pattern, "my-pattern");
    }

    #[test]
    fn test_eviction_when_full() {
        let c = DiscoveryCenter {
            max_entries: 3,
            ..Default::default()
        };
        c.record_solution(sample_entry(1)).unwrap();
        c.record_solution(sample_entry(2)).unwrap();
        c.record_solution(sample_entry(3)).unwrap();
        // This should evict one (the oldest by last_used_ms).
        c.record_solution(sample_entry(4)).unwrap();
        assert_eq!(c.entries.lock().unwrap().len(), 3);
    }

    #[test]
    fn test_search_limit() {
        let c = DiscoveryCenter::new();
        for i in 0..10 {
            let mut e = sample_entry(i);
            e.problem_pattern = "common".to_string();
            c.record_solution(e).unwrap();
        }
        let query = DiscoveryQuery {
            problem_pattern: Some("common".to_string()),
            tags: None,
            category: None,
            min_success_rate: None,
            limit: Some(3),
        };
        let result = c.search(&query);
        assert_eq!(result.entries.len(), 3);
        assert_eq!(result.total_matches, 10);
    }

    #[test]
    fn test_discovery_error_display() {
        let err = DiscoveryError::DuplicatePattern("x".to_string());
        assert_eq!(format!("{err}"), "duplicate pattern: x");

        let err = DiscoveryError::EntryNotFound("disc-42".to_string());
        assert_eq!(format!("{err}"), "entry not found: disc-42");
    }

    #[test]
    fn test_generate_id_monotonic() {
        let a = generate_id();
        let b = generate_id();
        // IDs are monotonic.
        let a_num: u64 = a[5..].parse().unwrap();
        let b_num: u64 = b[5..].parse().unwrap();
        assert!(a_num < b_num);
    }
}
