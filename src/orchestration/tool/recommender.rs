//! Dynamic tool recommendation engine.
//!
//! Recommends tools based on task descriptions, historical usage statistics,
//! and co‑occurrence patterns learned from the DiscoveryCenter.

use serde_json::Value;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// ToolUsageStats
// ---------------------------------------------------------------------------

/// Historical usage statistics for a single tool.
#[derive(Debug, Clone)]
pub struct ToolUsageStats {
    /// Name of the tool.
    pub _tool_name: String,
    /// Total number of calls to this tool.
    pub total_calls: u64,
    /// Number of calls that completed successfully.
    pub success_calls: u64,
    /// Average execution duration in milliseconds.
    pub _avg_duration_ms: f64,
    /// Timestamp (ms) of the most recent call.
    pub last_used_ms: u64,
    /// Map of other tool names to co‑occurrence counts.
    /// E.g. if "grep" was called 10 times in the same session as "read_file",
    /// `co_occurrence["read_file"] == 10`.
    pub co_occurrence: HashMap<String, u64>,
}

impl ToolUsageStats {
    /// Success rate in [0.0, 1.0].
    pub fn success_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.success_calls as f64 / self.total_calls as f64
        }
    }
}

// ---------------------------------------------------------------------------
// TaskToolPattern
// ---------------------------------------------------------------------------

/// A learned mapping from task keywords to relevant tools.
#[derive(Debug, Clone)]
pub struct TaskToolPattern {
    /// Keywords that trigger this pattern (lowercased for matching).
    pub keywords: Vec<String>,
    /// Tools that are relevant when these keywords match.
    pub tools: Vec<String>,
    /// Base relevance weight for this pattern.
    pub weight: f64,
}

// ---------------------------------------------------------------------------
// ToolRecommendation
// ---------------------------------------------------------------------------

/// A single tool recommendation with relevance scoring.
#[derive(Debug, Clone)]
pub struct ToolRecommendation {
    /// Name of the recommended tool.
    pub tool_name: String,
    /// Relevance score (higher = more relevant).  No strict upper bound.
    pub relevance_score: f64,
    /// Human‑readable explanation for why this tool was recommended.
    pub reason: String,
    /// Suggested argument shape, if the recommender can infer defaults.
    pub _suggested_args: Option<Value>,
}

// ---------------------------------------------------------------------------
// ToolRecommender
// ---------------------------------------------------------------------------

/// Dynamic tool recommender that combines task‑pattern matching with
/// historical usage statistics and co‑occurrence analysis.
pub struct ToolRecommender {
    /// Per‑tool usage statistics updated over time.
    pub tool_stats: HashMap<String, ToolUsageStats>,
    /// Task‑to‑tool mapping patterns (from DiscoveryCenter or config).
    pub task_patterns: Vec<TaskToolPattern>,
}

impl ToolRecommender {
    /// Create a new `ToolRecommender` with empty stats and no patterns.
    pub fn new() -> Self {
        Self {
            tool_stats: HashMap::new(),
            task_patterns: Vec::new(),
        }
    }

    /// Register a task pattern for keyword‑based matching.
    pub fn add_pattern(&mut self, pattern: TaskToolPattern) {
        self.task_patterns.push(pattern);
    }

    /// Update (or insert) usage stats for a tool after a call completes.
    pub fn record_usage(
        &mut self,
        tool_name: &str,
        success: bool,
        duration_ms: u64,
        timestamp_ms: u64,
        co_used_tools: &[String],
    ) {
        let entry = self
            .tool_stats
            .entry(tool_name.to_string())
            .or_insert_with(|| ToolUsageStats {
                _tool_name: tool_name.to_string(),
                total_calls: 0,
                success_calls: 0,
                _avg_duration_ms: 0.0,
                last_used_ms: 0,
                co_occurrence: HashMap::new(),
            });

        // Update rolling average duration.
        let total = entry.total_calls as f64;
        entry._avg_duration_ms =
            (entry._avg_duration_ms * total + duration_ms as f64) / (total + 1.0);

        entry.total_calls += 1;
        if success {
            entry.success_calls += 1;
        }
        entry.last_used_ms = timestamp_ms;

        for other in co_used_tools {
            *entry.co_occurrence.entry(other.clone()).or_insert(0) += 1;
        }
    }

    /// Generate tool recommendations for a given task description.
    ///
    /// The algorithm:
    /// 1. Tokenize the task description into lowercased keywords.
    /// 2. Match keywords against registered task patterns.
    /// 3. Score tools by: keyword match strength × success rate × recency bonus.
    /// 4. For each recommended tool, append co‑occurrence suggestions.
    ///
    /// `context` is an optional list of already‑planned tool names used to
    /// boost co‑occurrence signals.
    pub fn recommend(&self, task_description: &str, context: &[String]) -> Vec<ToolRecommendation> {
        let keywords: Vec<String> = task_description
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|w| w.len() >= 2)
            .collect();

        if keywords.is_empty() {
            return Vec::new();
        }

        let mut scores: HashMap<String, (f64, String)> = HashMap::new();

        // --- Phase 1: score by pattern matching ---
        for pattern in &self.task_patterns {
            let match_count = pattern
                .keywords
                .iter()
                .filter(|kw| keywords.iter().any(|w| w.contains(kw.as_str())))
                .count();

            if match_count == 0 {
                continue;
            }

            let match_ratio = match_count as f64 / pattern.keywords.len().max(1) as f64;

            for tool in &pattern.tools {
                let stats_boost = self
                    .tool_stats
                    .get(tool)
                    .map(|s| s.success_rate() * 0.5 + 0.5) // map [0,1] to [0.5, 1.0]
                    .unwrap_or(0.7); // default for unknown tools

                let base_score = pattern.weight * match_ratio * stats_boost;

                let entry = scores
                    .entry(tool.clone())
                    .or_insert_with(|| (0.0, String::new()));

                if base_score > entry.0 {
                    entry.0 = base_score;
                    entry.1 = format!(
                        "matches task pattern with keywords: {}",
                        pattern
                            .keywords
                            .iter()
                            .filter(|kw| keywords.iter().any(|w| w.contains(kw.as_str())))
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }

        // --- Phase 2: recency bonus ---
        let now_ms = self
            .tool_stats
            .values()
            .map(|s| s.last_used_ms)
            .max()
            .unwrap_or_default();

        for (tool, (score, _)) in scores.iter_mut() {
            if let Some(stats) = self.tool_stats.get(tool) {
                // Recency: 0 if last used within 5 min, decaying to 1 over 60 min.
                let age_ms = now_ms.saturating_sub(stats.last_used_ms);
                let recency = if age_ms <= 300_000 {
                    1.0
                } else if age_ms >= 3_600_000 {
                    0.0
                } else {
                    1.0 - (age_ms - 300_000) as f64 / (3_600_000 - 300_000) as f64
                };
                *score += recency * 0.15;

                // Frequency bonus: more calls = slightly higher.
                let freq = (stats.total_calls as f64).min(100.0) / 100.0;
                *score += freq * 0.05;
            }
        }

        // --- Phase 3: co‑occurrence suggestions ---
        // For each already‑recommended tool, look at its co‑occurrence map
        // and surface tools that frequently appear together.
        let mut co_suggestions: HashMap<String, (f64, String, String)> = HashMap::new();

        for tool in scores.keys() {
            if let Some(stats) = self.tool_stats.get(tool) {
                for (co_tool, count) in &stats.co_occurrence {
                    if scores.contains_key(co_tool) {
                        continue; // already recommended
                    }
                    let co_score = (*count as f64).min(20.0) / 20.0 * 0.3;
                    let entry = co_suggestions
                        .entry(co_tool.clone())
                        .or_insert_with(|| (0.0, String::new(), String::new()));
                    if co_score > entry.0 {
                        entry.0 = co_score;
                        entry.1 = format!(
                            "often used together with '{}' ({} co-occurrences)",
                            tool, count
                        );
                        entry.2 = tool.clone();
                    }
                }
            }
        }

        // Boost co‑occurrence if the partner tool is already in `context`.
        for (_co_tool, (score, reason, partner)) in co_suggestions.iter_mut() {
            if context.iter().any(|c| c == partner) {
                *score += 0.2;
                *reason = format!("{} — and '{}' is in the current plan", reason, partner);
            }
        }

        // --- Build final recommendation list ---
        let mut recommendations: Vec<ToolRecommendation> = Vec::new();

        for (tool, (score, reason)) in &scores {
            recommendations.push(ToolRecommendation {
                tool_name: tool.clone(),
                relevance_score: *score,
                reason: reason.clone(),
                _suggested_args: None,
            });
        }

        for (tool, (score, reason, _partner)) in &co_suggestions {
            recommendations.push(ToolRecommendation {
                tool_name: tool.clone(),
                relevance_score: *score,
                reason: reason.clone(),
                _suggested_args: None,
            });
        }

        // Sort descending by relevance_score.
        recommendations.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        recommendations
    }
}

impl Default for ToolRecommender {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Format a single recommendation for display.
#[cfg(test)]
pub fn format_recommendation(rec: &ToolRecommendation) -> String {
    format!(
        "{} (score={:.3}): {}",
        rec.tool_name, rec.relevance_score, rec.reason
    )
}

/// Look up usage stats for a tool, returning a reference.
#[cfg(test)]
pub fn get_tool_stats<'a>(
    recommender: &'a ToolRecommender,
    tool_name: &str,
) -> Option<&'a ToolUsageStats> {
    recommender.tool_stats.get(tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_recommender() -> ToolRecommender {
        let mut rec = ToolRecommender::new();

        // Register a few task patterns.
        rec.add_pattern(TaskToolPattern {
            keywords: vec![
                "search".to_string(),
                "find".to_string(),
                "grep".to_string(),
                "code".to_string(),
            ],
            tools: vec!["grep".to_string(), "read_file".to_string()],
            weight: 1.0,
        });

        rec.add_pattern(TaskToolPattern {
            keywords: vec![
                "write".to_string(),
                "create".to_string(),
                "edit".to_string(),
                "file".to_string(),
            ],
            tools: vec!["write_file".to_string(), "edit_file".to_string()],
            weight: 1.0,
        });

        // Seed usage stats.
        let ts = 1_000_000;
        rec.record_usage("grep", true, 50, ts, &["read_file".to_string()]);
        rec.record_usage("grep", true, 45, ts + 1, &["read_file".to_string()]);
        rec.record_usage("read_file", true, 30, ts + 2, &["grep".to_string()]);
        rec.record_usage("write_file", true, 100, ts + 3, &[]);
        rec.record_usage("write_file", false, 200, ts + 4, &[]);

        rec
    }

    #[test]
    fn recommends_tools_for_search_task() {
        let rec = build_recommender();
        let results = rec.recommend("search the codebase for error handling", &[]);
        assert!(!results.is_empty(), "should recommend at least one tool");

        let grep_rec = results.iter().find(|r| r.tool_name == "grep");
        assert!(grep_rec.is_some(), "should recommend grep for search task");
        assert!(
            grep_rec.unwrap().relevance_score > 0.0,
            "grep should have positive relevance"
        );
    }

    #[test]
    fn recommends_tools_for_write_task() {
        let rec = build_recommender();
        let results = rec.recommend("write a new file", &[]);
        assert!(!results.is_empty());

        let write_rec = results.iter().find(|r| r.tool_name == "write_file");
        assert!(
            write_rec.is_some(),
            "should recommend write_file for write task"
        );
    }

    #[test]
    fn co_occurrence_suggests_read_file_with_grep() {
        let rec = build_recommender();
        let results = rec.recommend("search the code", &["grep".to_string()]);

        // grep should be the primary recommendation.
        let grep_rec = results.iter().find(|r| r.tool_name == "grep");
        assert!(grep_rec.is_some());

        // read_file should also appear as co‑occurrence suggestion.
        let read_rec = results.iter().find(|r| r.tool_name == "read_file");
        assert!(read_rec.is_some(), "co-occurrence should suggest read_file");
    }

    #[test]
    fn empty_description_returns_empty() {
        let rec = build_recommender();
        let results = rec.recommend("", &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn success_rate_affects_scoring() {
        let rec = build_recommender();

        // write_file has 1 success + 1 failure = 50% success rate.
        // grep has 2 success / 2 total = 100%.
        let results = rec.recommend("search and write", &[]);

        let grep_score = results
            .iter()
            .find(|r| r.tool_name == "grep")
            .map(|r| r.relevance_score)
            .unwrap_or(0.0);
        let write_score = results
            .iter()
            .find(|r| r.tool_name == "write_file")
            .map(|r| r.relevance_score)
            .unwrap_or(0.0);

        // grep should have higher score due to better success rate.
        assert!(
            grep_score >= write_score,
            "grep (100% success) should score >= write_file (50% success); grep={}, write={}",
            grep_score,
            write_score
        );
    }
}
