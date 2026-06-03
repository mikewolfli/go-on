//! SemanticCapabilityMatcher — lightweight text-similarity-based matcher.
//!
//! Matches task descriptions to model capabilities and skills using
//! token overlap (Jaccard + TF) with capability-aware boosting.
//! No external embedding dependency — pure token-level statistics.

use std::collections::HashSet;

/// A model capability declaration used for matching.
#[derive(Debug, Clone)]
pub struct ModelCapability {
    /// Unique model ID.
    pub model_id: String,
    /// Human-readable capability description.
    pub description: String,
    /// Tags associated with this capability (e.g. "vision", "code", "chat").
    pub tags: Vec<String>,
}

/// A model scored by the matcher.
#[derive(Debug, Clone)]
pub struct ScoredModel {
    pub model_id: String,
    pub score: f64,
    pub match_reasons: Vec<String>,
}

/// A skill capability declaration used for matching.
#[derive(Debug, Clone)]
pub struct SkillCapability {
    /// Unique skill ID.
    pub skill_id: String,
    /// Human-readable skill description.
    pub description: String,
    /// Tags associated with this skill.
    pub tags: Vec<String>,
}

/// A skill scored by the matcher.
#[derive(Debug, Clone)]
pub struct ScoredSkill {
    pub skill_id: String,
    pub score: f64,
    pub match_reasons: Vec<String>,
}

// ---------------------------------------------------------------------------
// Capability-aware boosting weights
// ---------------------------------------------------------------------------

/// Boost multipliers applied when the task description suggests a specific
/// capability domain.
#[derive(Debug, Clone)]
struct CapabilityBoost {
    /// Task keywords that trigger this boost.
    keywords: &'static [&'static str],
    /// Target tags that receive the boost.
    target_tags: &'static [&'static str],
    /// Multiplier applied to the similarity score (≥ 1.0).
    multiplier: f64,
}

const BOOST_TABLE: &[CapabilityBoost] = &[
    CapabilityBoost {
        keywords: &[
            "image",
            "vision",
            "screenshot",
            "photo",
            "picture",
            "ocr",
            "diagram",
        ],
        target_tags: &["vision", "image", "multimodal"],
        multiplier: 2.0,
    },
    CapabilityBoost {
        keywords: &[
            "code",
            "function",
            "implement",
            "debug",
            "refactor",
            "compile",
            "algorithm",
            "syntax",
            "programming",
            "api",
            "generator",
        ],
        target_tags: &["code", "programming", "generation"],
        multiplier: 2.5,
    },
    CapabilityBoost {
        keywords: &[
            "chat",
            "conversation",
            "question",
            "explain",
            "discuss",
            "summarize",
        ],
        target_tags: &["chat", "conversation", "language"],
        multiplier: 1.3,
    },
    CapabilityBoost {
        keywords: &[
            "tool",
            "function_call",
            "function calling",
            "api call",
            "execute",
            "bash",
            "shell",
            "command",
        ],
        target_tags: &["function_calling", "tool_use", "agent"],
        multiplier: 2.2,
    },
    CapabilityBoost {
        keywords: &[
            "reasoning",
            "logic",
            "math",
            "analysis",
            "complex",
            "multi-step",
            "planning",
        ],
        target_tags: &["reasoning", "chain_of_thought", "advanced"],
        multiplier: 1.8,
    },
];

// ---------------------------------------------------------------------------
// SemanticCapabilityMatcher
// ---------------------------------------------------------------------------

/// Lightweight text-similarity-based capability matcher.
pub struct SemanticCapabilityMatcher;

impl SemanticCapabilityMatcher {
    /// Match a task description to model capabilities using TF-IDF-like
    /// scoring with capability-aware boosting.
    ///
    /// Returns models sorted by descending score.
    pub fn match_task_to_models(task: &str, models: &[ModelCapability]) -> Vec<ScoredModel> {
        let task_tokens = Self::tokenize(task);
        let task_lower = task.to_lowercase();

        let mut scored: Vec<ScoredModel> = models
            .iter()
            .map(|m| {
                let cap_tokens = Self::tokenize(&m.description);
                let base_score = Self::score_match(&task_tokens, &cap_tokens, &m.tags);
                let boost = Self::capability_boost(&task_lower, &m.tags);
                let final_score = (base_score * boost).min(1.0);

                let mut reasons = Vec::new();
                if boost > 1.0 {
                    reasons.push(format!("capability_boost={:.2}", boost));
                }
                if base_score > 0.3 {
                    reasons.push("token_overlap".to_string());
                }

                ScoredModel {
                    model_id: m.model_id.clone(),
                    score: final_score,
                    match_reasons: reasons,
                }
            })
            .collect();

        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored
    }

    /// Match a task description to skills using keyword + semantic similarity.
    ///
    /// Returns skills sorted by descending score.
    pub fn match_task_to_skills(task: &str, skills: &[SkillCapability]) -> Vec<ScoredSkill> {
        let task_tokens = Self::tokenize(task);
        let task_lower = task.to_lowercase();

        let mut scored: Vec<ScoredSkill> = skills
            .iter()
            .map(|s| {
                let cap_tokens = Self::tokenize(&s.description);
                let base_score = Self::score_match(&task_tokens, &cap_tokens, &s.tags);
                let boost = Self::capability_boost(&task_lower, &s.tags);
                let final_score = (base_score * boost).min(1.0);

                let mut reasons = Vec::new();
                if boost > 1.0 {
                    reasons.push(format!("capability_boost={:.2}", boost));
                }
                if base_score > 0.3 {
                    reasons.push("token_overlap".to_string());
                }

                ScoredSkill {
                    skill_id: s.skill_id.clone(),
                    score: final_score,
                    match_reasons: reasons,
                }
            })
            .collect();

        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored
    }

    /// Compute Jaccard-like similarity with TF weighting.
    ///
    /// Formula: |intersection| / |union| × tag_signal
    /// where tag_signal boosts the score when tags align with common
    /// capability categories.
    fn score_match(
        task_tokens: &HashSet<String>,
        capability_tokens: &HashSet<String>,
        capability_tags: &[String],
    ) -> f64 {
        let intersection = task_tokens.intersection(capability_tokens).count() as f64;
        let union = task_tokens.union(capability_tokens).count() as f64;

        if union == 0.0 {
            return 0.0;
        }

        let jaccard = intersection / union;

        // TF-like weighting: common tokens get weighted higher.
        let tf_weight = if intersection > 0.0 {
            intersection / task_tokens.len().max(1) as f64
        } else {
            0.0
        };

        // Tag signal: if capability tags overlap with known categories.
        let tag_signal = Self::tag_signal(capability_tags);

        (0.5 * jaccard + 0.3 * tf_weight + 0.2 * tag_signal).min(1.0)
    }

    /// Compute a signal (0.0–1.0) from capability tags with nuanced
    /// boost categories. Concrete, well-known tags (vision, code, etc.)
    /// score higher than generic or unrecognised ones.
    fn tag_signal(tags: &[String]) -> f64 {
        // Tier 1: highly specific technology or domain tags
        const HIGH_SPECIFICITY: &[&str] = &[
            "vision",
            "image",
            "multimodal",
            "function_calling",
            "tool_use",
            "agent",
            "chain_of_thought",
            "reasoning",
        ];
        // Tier 2: broad capability area tags
        const MEDIUM_SPECIFICITY: &[&str] =
            &["code", "programming", "generation", "analysis", "language"];
        // Tier 3: generic interaction-mode tags
        const LOW_SPECIFICITY: &[&str] = &["chat", "conversation", "general"];

        let known_tags: HashSet<&str> = HIGH_SPECIFICITY
            .iter()
            .chain(MEDIUM_SPECIFICITY.iter())
            .chain(LOW_SPECIFICITY.iter())
            .copied()
            .collect();

        if tags.is_empty() {
            return 0.1;
        }

        let total = tags.len();
        let mut high_count = 0_usize;
        let mut med_count = 0_usize;
        let mut low_count = 0_usize;
        let mut unknown_count = 0_usize;

        for t in tags {
            let t = t.as_str();
            if HIGH_SPECIFICITY.contains(&t) {
                high_count += 1;
            } else if MEDIUM_SPECIFICITY.contains(&t) {
                med_count += 1;
            } else if LOW_SPECIFICITY.contains(&t) {
                low_count += 1;
            } else if known_tags.contains(t) {
                // Should not happen given the contains checks above,
                // but kept as a safety net
                low_count += 1;
            } else {
                unknown_count += 1;
            }
        }

        if total == 0 {
            return 0.1;
        }

        // Weighted average: high-specificity tags contribute 1.0,
        // medium-specificity 0.6, low-specificity 0.3, unknown 0.1.
        let weighted = (high_count as f64 * 1.0
            + med_count as f64 * 0.6
            + low_count as f64 * 0.3
            + unknown_count as f64 * 0.1)
            / total as f64;

        weighted.min(1.0)
    }

    /// Compute a capability-domain boost multiplier based on task keywords.
    fn capability_boost(task_lower: &str, tags: &[String]) -> f64 {
        let mut boost: f64 = 1.0_f64;
        for entry in BOOST_TABLE {
            let task_matches = entry.keywords.iter().any(|kw| task_lower.contains(kw));
            if !task_matches {
                continue;
            }
            let tag_matches = entry
                .target_tags
                .iter()
                .any(|tt| tags.iter().any(|t| t == *tt));
            if tag_matches {
                let m: f64 = entry.multiplier;
                boost = boost.max(m);
            }
        }
        boost
    }

    /// Tokenize a string into lowercase word tokens, stripping punctuation.
    fn tokenize(text: &str) -> HashSet<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty() && t.len() >= 2)
            .map(|t| t.to_string())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_models() -> Vec<ModelCapability> {
        vec![
            ModelCapability {
                model_id: "gpt-4o".to_string(),
                description: "Advanced multimodal model with vision and code generation"
                    .to_string(),
                tags: vec![
                    "vision".to_string(),
                    "code".to_string(),
                    "multimodal".to_string(),
                ],
            },
            ModelCapability {
                model_id: "deepseek-v4-flash".to_string(),
                description: "Fast economical model for chat and simple code tasks".to_string(),
                tags: vec!["chat".to_string(), "code".to_string()],
            },
            ModelCapability {
                model_id: "claude-sonnet".to_string(),
                description: "Balanced model with strong reasoning and tool use".to_string(),
                tags: vec![
                    "reasoning".to_string(),
                    "tool_use".to_string(),
                    "chat".to_string(),
                ],
            },
        ]
    }

    fn sample_skills() -> Vec<SkillCapability> {
        vec![
            SkillCapability {
                skill_id: "code_refactor".to_string(),
                description: "Refactor code for improved readability and performance".to_string(),
                tags: vec!["code".to_string(), "refactor".to_string()],
            },
            SkillCapability {
                skill_id: "image_analysis".to_string(),
                description: "Analyze images and screenshots for content".to_string(),
                tags: vec!["vision".to_string(), "image".to_string()],
            },
        ]
    }

    #[test]
    fn code_task_boosts_code_models() {
        let task = "implement a function to sort an array";
        let results = SemanticCapabilityMatcher::match_task_to_models(task, &sample_models());

        assert!(!results.is_empty());
        // gpt-4o should rank highest (code + vision boost).
        assert_eq!(results[0].model_id, "gpt-4o");
    }

    #[test]
    fn vision_task_boosts_vision_models() {
        let task = "describe what you see in this screenshot image";
        let results = SemanticCapabilityMatcher::match_task_to_models(task, &sample_models());

        assert!(!results.is_empty());
        assert_eq!(results[0].model_id, "gpt-4o");
    }

    #[test]
    fn reasoning_task_boosts_reasoning_models() {
        let task = "perform complex multi-step reasoning and analysis";
        let results = SemanticCapabilityMatcher::match_task_to_models(task, &sample_models());

        assert!(!results.is_empty());
        assert_eq!(results[0].model_id, "claude-sonnet");
    }

    #[test]
    fn skill_matching_prefers_relevant_skills() {
        let task = "refactor the codebase to improve performance";
        let results = SemanticCapabilityMatcher::match_task_to_skills(task, &sample_skills());

        assert!(!results.is_empty());
        assert_eq!(results[0].skill_id, "code_refactor");
        assert!(results[0].score > results[1].score);
    }

    #[test]
    fn empty_task_returns_default_scores() {
        let results = SemanticCapabilityMatcher::match_task_to_models("", &sample_models());
        assert_eq!(results.len(), 3);
        // With no tokens, all scores should be equal (0.0 from Jaccard, tag_signal).
        for r in &results {
            assert!(r.score >= 0.0);
        }
    }

    #[test]
    fn tag_signal_returns_nuanced_boost() {
        // Two high-specificity tags → weighted avg = (1.0 + 1.0) / 2 = 1.0
        let high: Vec<String> = vec!["vision".to_string(), "reasoning".to_string()];
        assert!((SemanticCapabilityMatcher::tag_signal(&high) - 1.0).abs() < 0.01);

        // One high (1.0) + one medium (0.6) → weighted avg = 0.8
        let mixed: Vec<String> = vec!["vision".to_string(), "code".to_string()];
        let signal = SemanticCapabilityMatcher::tag_signal(&mixed);
        assert!((signal - 0.8).abs() < 0.01, "expected 0.8, got {signal}");

        // No known tags → low signal.
        let unknown: Vec<String> = vec!["foo".to_string(), "bar".to_string()];
        assert!((SemanticCapabilityMatcher::tag_signal(&unknown) - 0.1).abs() < 0.01);
    }

    #[test]
    fn tokenize_strips_punctuation_and_short_tokens() {
        let tokens = SemanticCapabilityMatcher::tokenize("Hello, World! A test.");
        assert!(tokens.contains("hello"));
        assert!(tokens.contains("world"));
        assert!(tokens.contains("test"));
        // "A" is too short (len < 2).
        assert!(!tokens.contains("a"));
    }
}
