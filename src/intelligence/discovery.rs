//! F-GAP-11: Solution Discovery Center
//!
//! Centralized registry for discovering agent capabilities, solutions, and
//! patterns across the system.  Indexes solutions by problem pattern, success
//! rate, and applicability.

use crate::i18n::tf;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ── ID generation ────────────────────────────────────────────────────────────

static DISCOVERY_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_id() -> String {
    let n = DISCOVERY_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("disc-{}", n)
}

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
            Self::DuplicatePattern(name) => write!(
                f,
                "{}",
                tf("error.discovery.duplicate_pattern", &[("name", name)])
            ),
            Self::EntryNotFound(id) => write!(
                f,
                "{}",
                tf("error.discovery.entry_not_found", &[("id", id)])
            ),
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
    /// Max entries to retain
    max_entries: usize,
}

impl DiscoveryCenter {
    /// Create a new `DiscoveryCenter` with default settings.
    ///
    /// The default maximum number of retained entries is 10 000.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            max_entries: 10_000,
        }
    }

    /// Record a problem-solution entry.
    ///
    /// Returns the auto-generated entry id on success.  When the centre is at
    /// capacity the oldest entry (by `last_used_ms`) is evicted first.
    pub fn record_solution(&self, entry: DiscoveryEntry) -> Result<String> {
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!(target: "discovery", "entries Mutex poisoned – recovering");
                poisoned.into_inner()
            }
        };

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
        let now = crate::shared::timestamps::now_ts_ms() as u64;
        let mut entry = entry;
        entry.id = id.clone();
        entry.created_ms = now;
        entry.last_used_ms = now;

        entries.push(entry);
        Ok(id)
    }

    /// Search for solutions matching the given query.
    ///
    /// When no limit is specified in the query, up to 20 results are returned.
    pub fn search(&self, query: &DiscoveryQuery) -> DiscoveryResult {
        let start = crate::shared::timestamps::now_ts_ms() as u64;

        // Hold the lock and filter in-place to avoid cloning the entire entries vec;
        // only matching entries are cloned out.
        let mut entries_guard = match self.entries.lock() {
            Ok(e) => e,
            Err(poisoned) => {
                tracing::warn!(target: "discovery", "entries Mutex poisoned – recovering in search");
                poisoned.into_inner()
            }
        };

        let mut matches: Vec<DiscoveryEntry> = entries_guard
            .iter()
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
            .cloned()
            .collect();

        // Touch matched entries so eviction is true LRU: mark the hit time
        // before releasing the lock (search() is the only read path).
        let now = crate::shared::timestamps::now_ts_ms() as u64;
        if !matches.is_empty() {
            for entry in entries_guard.iter_mut() {
                if matches.iter().any(|m| m.id == entry.id) {
                    entry.last_used_ms = now;
                }
            }
        }
        // Release the lock before sorting / truncating.
        drop(entries_guard);

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
            query_duration_ms: crate::shared::timestamps::now_ts_ms() as u64 - start,
            best_match,
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
        c.record_solution(sample_entry(1))
            .expect("should record sample entry");

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
    fn test_eviction_when_full() {
        let c = DiscoveryCenter {
            max_entries: 3,
            ..Default::default()
        };
        c.record_solution(sample_entry(1))
            .expect("should record first entry for eviction test");
        c.record_solution(sample_entry(2))
            .expect("should record second entry for eviction test");
        c.record_solution(sample_entry(3))
            .expect("should record third entry for eviction test");
        // This should evict one (the oldest by last_used_ms).
        c.record_solution(sample_entry(4))
            .expect("should record fourth entry to trigger eviction");
        assert_eq!(c.entries.lock().expect("Mutex should be unlocked").len(), 3);
    }

    #[test]
    fn test_search_limit() {
        let c = DiscoveryCenter::new();
        for i in 0..10 {
            let mut e = sample_entry(i);
            e.problem_pattern = "common".to_string();
            c.record_solution(e)
                .expect("should record entry for search_limit test");
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
        let msg = format!("{err}");
        // Accept either the i18n-key fallback or the resolved translation
        assert!(
            msg == "duplicate pattern: x" || msg.starts_with("error.discovery."),
            "unexpected display: {}",
            msg
        );

        let err = DiscoveryError::EntryNotFound("disc-42".to_string());
        let msg = format!("{err}");
        assert!(
            msg == "entry not found: disc-42" || msg.starts_with("error.discovery."),
            "unexpected display: {}",
            msg
        );
    }

    #[test]
    fn test_generate_id_monotonic() {
        let a = generate_id();
        let b = generate_id();
        // IDs are monotonic.
        let a_num: u64 = a[5..].parse().expect("ID suffix should be a valid u64");
        let b_num: u64 = b[5..].parse().expect("ID suffix should be a valid u64");
        assert!(a_num < b_num);
    }
}
