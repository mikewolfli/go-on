//! Scenario Matcher (F-GAP-12)
//!
//! Matches incoming tasks against known scenarios to provide pre-configured
//! routing decisions, tool selections, and execution strategies.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A scenario definition with matching rules and routing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub name: String,
    pub description: String,
    pub priority: u32, // Higher = matched first
    pub match_rules: MatchRules,
    pub routing: ScenarioRouting,
    pub created_ms: u64,
    pub is_active: bool,
}

/// Rules for matching a task against this scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchRules {
    /// Required keywords in task description (any match).
    pub keywords: Vec<String>,
    /// Required task types (any match).
    pub task_types: Vec<String>,
    /// Required tags on the agent (any match).
    pub agent_tags: Vec<String>,
    /// Complexity range: (min, max), 0.0–1.0.
    pub complexity_range: Option<(f64, f64)>,
    /// Risk score range: (min, max), 0.0–1.0.
    pub risk_range: Option<(f64, f64)>,
}

/// Routing configuration when a scenario matches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioRouting {
    /// Agent to route to (if any).
    pub preferred_agent: Option<String>,
    /// Execution mode to use.
    pub recommended_mode: String,
    /// Tools to make available.
    pub enabled_tools: Vec<String>,
    /// Tags to add to the task.
    pub add_tags: Vec<String>,
}

/// Result returned by a matching attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchResult {
    pub matched: bool,
    pub scenario: Option<Scenario>,
    pub confidence: f64,
    pub match_reasons: Vec<String>,
    pub alternatives: Vec<Scenario>,
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// ScenarioMatcher
// ---------------------------------------------------------------------------

/// Matches incoming tasks against known scenarios to provide pre-configured
/// routing decisions, tool selections, and execution strategies.
/// Maximum number of registered scenarios before evicting the oldest.
const MAX_SCENARIOS: usize = 1000;

pub struct ScenarioMatcher {
    /// Registered scenarios.
    scenarios: Arc<RwLock<Vec<Scenario>>>,
}

impl Default for ScenarioMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ScenarioMatcher {
    /// Create a new empty `ScenarioMatcher`.
    pub fn new() -> Self {
        Self {
            scenarios: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register a scenario. If a scenario with the same `id` already exists,
    /// it is replaced (including statistics reset for that id).
    pub fn register_scenario(&self, scenario: Scenario) {
        let mut scenarios = crate::write_or_recover!(self.scenarios.as_ref(), "intelligence");

        // Replace existing scenario with the same id, or push new.
        if let Some(pos) = scenarios.iter().position(|s| s.id == scenario.id) {
            scenarios[pos] = scenario;
        } else {
            // Enforce max_scenarios cap: evict the oldest (front of the vec)
            // when at capacity to prevent unbounded memory growth.
            if scenarios.len() >= MAX_SCENARIOS {
                let evicted = scenarios.remove(0);
                tracing::warn!(
                    "scenario cap reached ({}): evicting oldest scenario '{}'",
                    MAX_SCENARIOS,
                    evicted.name,
                );
            }
            scenarios.push(scenario);
        }
    }

    /// Find the best matching scenario for the given task attributes.
    ///
    /// The algorithm:
    /// 1. Score each **active** scenario by how many rule categories it matches.
    /// 2. Keywords match against `description` (case-insensitive contains).
    /// 3. Task types must have at least one match.
    /// 4. Agent tags must have at least one match.
    /// 5. Complexity and risk must be within their declared ranges (if set).
    /// 6. Highest **priority** wins; ties are broken by more matched rules.
    /// 7. The top match is returned, along with up to 3 alternatives.
    pub fn match_task(
        &self,
        task_type: &str,
        description: &str,
        complexity: f64,
        risk_score: f64,
        agent_tags: &[String],
    ) -> MatchResult {
        let start = Instant::now();

        let scenarios = crate::read_or_recover!(self.scenarios.as_ref(), "intelligence");

        // Score each active scenario.
        struct Scored {
            scenario: Scenario,
            matched_rule_count: u32,
            reasons: Vec<String>,
        }

        let mut scored: Vec<Scored> = scenarios
            .iter()
            .filter(|s| s.is_active)
            .filter_map(|scenario| {
                let mut matched_rule_count: u32 = 0;
                let mut reasons: Vec<String> = Vec::new();

                // --- Keywords (case-insensitive contains) ---
                if !scenario.match_rules.keywords.is_empty() {
                    let desc_lower = description.to_lowercase();
                    let any_keyword_match = scenario
                        .match_rules
                        .keywords
                        .iter()
                        .any(|kw| desc_lower.contains(&kw.to_lowercase()));
                    if any_keyword_match {
                        matched_rule_count += 1;
                        reasons.push("keywords".to_string());
                    }
                }

                // --- Task types (any match) ---
                if !scenario.match_rules.task_types.is_empty() {
                    if scenario
                        .match_rules
                        .task_types
                        .iter()
                        .any(|tt| tt.eq_ignore_ascii_case(task_type))
                    {
                        matched_rule_count += 1;
                        reasons.push("task_type".to_string());
                    } else {
                        // Task type is mandatory – at least one must match.
                        return None;
                    }
                }

                // --- Agent tags (any match) ---
                if !scenario.match_rules.agent_tags.is_empty() {
                    let any_tag_match = agent_tags.iter().any(|tag| {
                        scenario
                            .match_rules
                            .agent_tags
                            .iter()
                            .any(|st| st.eq_ignore_ascii_case(tag))
                    });
                    if any_tag_match {
                        matched_rule_count += 1;
                        reasons.push("agent_tags".to_string());
                    } else {
                        // Agent tag is mandatory – at least one must match.
                        return None;
                    }
                }

                // --- Complexity range ---
                if let Some((c_min, c_max)) = scenario.match_rules.complexity_range {
                    if complexity >= c_min && complexity <= c_max {
                        matched_rule_count += 1;
                        reasons.push("complexity".to_string());
                    } else {
                        return None;
                    }
                }

                // --- Risk score range ---
                if let Some((r_min, r_max)) = scenario.match_rules.risk_range {
                    if risk_score >= r_min && risk_score <= r_max {
                        matched_rule_count += 1;
                        reasons.push("risk".to_string());
                    } else {
                        return None;
                    }
                }

                if matched_rule_count == 0 {
                    // Not a single rule triggered — don't consider this as a
                    // match.  (A scenario with *all* rules empty would match
                    // vacuously, but that is usually not intended.)
                    return None;
                }

                Some(Scored {
                    scenario: scenario.clone(),
                    matched_rule_count,
                    reasons,
                })
            })
            .collect();

        // Sort: highest priority first, then most matched rules.
        scored.sort_by(|a, b| {
            b.scenario
                .priority
                .cmp(&a.scenario.priority)
                .then(b.matched_rule_count.cmp(&a.matched_rule_count))
        });

        let duration_ms = start.elapsed().as_millis() as u64;

        // Build alternatives list (up to 3) from the remaining scored list.
        let (top, alternatives) = if scored.is_empty() {
            (None, Vec::new())
        } else {
            let top_scored = scored.remove(0);
            let alt_scenarios: Vec<Scenario> =
                scored.into_iter().take(3).map(|s| s.scenario).collect();
            (Some(top_scored), alt_scenarios)
        };

        let (matched, scenario, confidence, match_reasons) = match top {
            Some(scored) => {
                let reasons = scored.reasons;
                let confidence =
                    scored.matched_rule_count as f64 / max_rule_categories(&scored.scenario) as f64;
                (true, Some(scored.scenario), confidence, reasons)
            }
            None => (false, None, 0.0, Vec::new()),
        };

        MatchResult {
            matched,
            scenario,
            confidence,
            match_reasons,
            alternatives,
            duration_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Returns the number of rule categories that a scenario *could* match.
/// Used to normalise the confidence score.
fn max_rule_categories(scenario: &Scenario) -> u32 {
    let rules = &scenario.match_rules;
    let mut count = 0u32;
    if !rules.keywords.is_empty() {
        count += 1;
    }
    if !rules.task_types.is_empty() {
        count += 1;
    }
    if !rules.agent_tags.is_empty() {
        count += 1;
    }
    if rules.complexity_range.is_some() {
        count += 1;
    }
    if rules.risk_range.is_some() {
        count += 1;
    }
    count
}

use std::time::Instant;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_scenario(id: &str, priority: u32, keywords: Vec<&str>) -> Scenario {
        Scenario {
            id: id.to_string(),
            name: format!("Scenario {id}"),
            description: String::new(),
            priority,
            match_rules: MatchRules {
                keywords: keywords.into_iter().map(String::from).collect(),
                task_types: vec!["inference".to_string()],
                agent_tags: vec!["gpu".to_string()],
                complexity_range: Some((0.0, 1.0)),
                risk_range: Some((0.0, 0.8)),
            },
            routing: ScenarioRouting {
                preferred_agent: None,
                recommended_mode: "auto".to_string(),
                enabled_tools: vec![],
                add_tags: vec![],
            },
            created_ms: crate::shared::timestamps::now_ts_ms() as u64,
            is_active: true,
        }
    }

    #[test]
    fn test_register_and_deactivate() {
        let matcher = ScenarioMatcher::new();
        let mut s1 = sample_scenario("s1", 10, vec!["hello"]);
        s1.is_active = false;
        matcher.register_scenario(s1);

        let result = matcher.match_task("inference", "hello", 0.5, 0.3, &["gpu".to_string()]);
        assert!(!result.matched, "inactive scenario must not match");
    }

    #[test]
    fn test_match_keywords() {
        let matcher = ScenarioMatcher::new();
        let s1 = sample_scenario("s1", 10, vec!["urgent", "critical"]);
        matcher.register_scenario(s1);

        let result = matcher.match_task(
            "inference",
            "This is an urgent request",
            0.5,
            0.3,
            &["gpu".to_string()],
        );
        assert!(result.matched);
        assert_eq!(result.scenario.as_ref().unwrap().id, "s1");
    }

    #[test]
    fn test_no_match_wrong_task_type() {
        let matcher = ScenarioMatcher::new();
        let s1 = sample_scenario("s1", 10, vec!["urgent"]);
        matcher.register_scenario(s1);

        let result =
            matcher.match_task("training", "urgent request", 0.5, 0.3, &["gpu".to_string()]);
        assert!(!result.matched);
    }

    #[test]
    fn test_priority_tiebreak() {
        let matcher = ScenarioMatcher::new();
        let s_high = sample_scenario("high", 20, vec!["urgent"]);
        let s_low = sample_scenario("low", 5, vec!["urgent"]);
        matcher.register_scenario(s_high);
        matcher.register_scenario(s_low);

        let result = matcher.match_task(
            "inference",
            "urgent request",
            0.5,
            0.3,
            &["gpu".to_string()],
        );
        assert!(result.matched);
        assert_eq!(result.scenario.as_ref().unwrap().id, "high");
    }

    #[test]
    fn test_alternatives() {
        let matcher = ScenarioMatcher::new();
        for i in 0..5 {
            let s = sample_scenario(&format!("s{i}"), 10, vec!["urgent"]);
            matcher.register_scenario(s);
        }

        let result = matcher.match_task(
            "inference",
            "urgent request",
            0.5,
            0.3,
            &["gpu".to_string()],
        );
        assert!(result.matched);
        // Should have up to 3 alternatives (plus the winner = 4 scenarios total
        // matched).
        assert_eq!(result.alternatives.len(), 3);
    }

    #[test]
    fn test_match_priority_used_for_tiebreak() {
        // Verifies that when multiple scenarios match, higher priority wins.
        let matcher = ScenarioMatcher::new();
        matcher.register_scenario(sample_scenario("low", 5, vec!["urgent"]));
        matcher.register_scenario(sample_scenario("high", 20, vec!["urgent"]));

        let result = matcher.match_task("inference", "urgent", 0.5, 0.3, &["gpu".to_string()]);
        assert!(result.matched);
        assert_eq!(result.scenario.as_ref().unwrap().id, "high");
    }

    #[test]
    fn test_no_active_scenarios() {
        let matcher = ScenarioMatcher::new();
        let mut s1 = sample_scenario("s1", 10, vec!["urgent"]);
        s1.is_active = false;
        matcher.register_scenario(s1);

        let result = matcher.match_task("inference", "urgent", 0.5, 0.3, &["gpu".to_string()]);
        assert!(!result.matched);
    }

    #[test]
    fn test_complexity_out_of_range() {
        let matcher = ScenarioMatcher::new();
        let mut s1 = sample_scenario("s1", 10, vec!["urgent"]);
        s1.match_rules.complexity_range = Some((0.0, 0.3));
        matcher.register_scenario(s1);

        let result = matcher.match_task(
            "inference",
            "urgent",
            0.9, // out of range
            0.3,
            &["gpu".to_string()],
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_update_existing_scenario() {
        let matcher = ScenarioMatcher::new();
        let s1 = sample_scenario("s1", 10, vec!["hello"]);
        matcher.register_scenario(s1);

        let s1_updated = Scenario {
            id: "s1".to_string(),
            priority: 99,
            ..sample_scenario("s1", 10, vec!["hello"])
        };
        matcher.register_scenario(s1_updated);

        let result = matcher.match_task("inference", "hello", 0.5, 0.3, &["gpu".to_string()]);
        assert!(result.matched);
        assert_eq!(result.scenario.as_ref().unwrap().priority, 99);
    }
}
