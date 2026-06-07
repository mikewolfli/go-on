//! Tool extraction from model responses
//!
//! Extracts tool calls from various formats including JSON blocks,
//! inline markers, and structured response formats.

use serde_json::Value;

/// Extract model tool calls from response
pub(crate) fn extract_tool_calls_from_response(response: &str, max_calls: usize) -> Vec<String> {
    // Parse only explicit tool-call markers; never synthesize placeholder calls.
    let mut calls: Vec<String> = Vec::with_capacity(max_calls);
    let mut json_block: Vec<String> = Vec::with_capacity(32);
    let mut in_json_block = false;

    let flush_json_block = |json_block: &mut Vec<String>, calls: &mut Vec<String>| {
        if json_block.is_empty() {
            return;
        }

        let block = json_block.join("\n");
        json_block.clear();

        let Ok(value) = serde_json::from_str::<Value>(&block) else {
            return;
        };

        let mut push_call = |call_name: &str| {
            let candidate = call_name.trim();
            if candidate.is_empty() {
                return;
            }

            let valid_name = candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
            if !valid_name {
                return;
            }

            if !calls.iter().any(|name| name == candidate) {
                calls.push(candidate.to_string());
            }
        };

        match value {
            Value::Object(map) => {
                if let Some(tool_call) = map.get("tool_call").and_then(Value::as_str) {
                    push_call(tool_call);
                }

                if let Some(tool_calls) = map.get("tool_calls").and_then(Value::as_array) {
                    for item in tool_calls {
                        match item {
                            Value::String(name) => push_call(name),
                            Value::Object(object) => {
                                if let Some(name) = object.get("name").and_then(Value::as_str) {
                                    push_call(name);
                                } else if let Some(name) =
                                    object.get("tool").and_then(Value::as_str)
                                {
                                    push_call(name);
                                }
                            }
                            _ => {}
                        }
                    }
                }

                if let Some(actions) = map.get("actions").and_then(Value::as_array) {
                    for item in actions {
                        match item {
                            Value::String(name) => push_call(name),
                            Value::Object(object) => {
                                if let Some(name) = object.get("name").and_then(Value::as_str) {
                                    push_call(name);
                                } else if let Some(name) =
                                    object.get("tool").and_then(Value::as_str)
                                {
                                    push_call(name);
                                } else if let Some(name) =
                                    object.get("action").and_then(Value::as_str)
                                {
                                    push_call(name);
                                }
                            }
                            _ => {}
                        }
                    }
                }

                if let Some(action_plan) = map.get("action_plan") {
                    if let Some(action_plan_actions) =
                        action_plan.get("actions").and_then(Value::as_array)
                    {
                        for item in action_plan_actions {
                            match item {
                                Value::String(name) => push_call(name),
                                Value::Object(object) => {
                                    if let Some(name) = object.get("name").and_then(Value::as_str) {
                                        push_call(name);
                                    } else if let Some(name) =
                                        object.get("tool").and_then(Value::as_str)
                                    {
                                        push_call(name);
                                    } else if let Some(name) =
                                        object.get("action").and_then(Value::as_str)
                                    {
                                        push_call(name);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    if let Value::Object(object) = item {
                        if let Some(name) = object.get("name").and_then(Value::as_str) {
                            push_call(name);
                        } else if let Some(name) = object.get("tool").and_then(Value::as_str) {
                            push_call(name);
                        } else if let Some(name) = object.get("action").and_then(Value::as_str) {
                            push_call(name);
                        }
                    }
                }
            }
            _ => {}
        }
    };

    for line in response.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            if in_json_block {
                flush_json_block(&mut json_block, &mut calls);
                in_json_block = false;
                if calls.len() >= max_calls {
                    break;
                }
                continue;
            }

            let fence_lang = trimmed.trim_start_matches("```").trim();
            in_json_block = fence_lang.is_empty() || fence_lang.eq_ignore_ascii_case("json");
            continue;
        }

        if in_json_block {
            json_block.push(trimmed.to_string());
            continue;
        }

        let marker_value = trimmed
            .strip_prefix("__tool_call__")
            .map(|value| value.trim_start_matches(':').trim())
            .or_else(|| trimmed.strip_prefix("tool_call:").map(str::trim))
            .or_else(|| trimmed.strip_prefix("tool:").map(str::trim));

        let Some(raw_name) = marker_value else {
            continue;
        };

        let candidate = raw_name
            .split(|c: char| c == '(' || c == '{' || c == ':' || c.is_whitespace())
            .next()
            .unwrap_or("")
            .trim();

        if candidate.is_empty() {
            continue;
        }

        let valid_name = candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
        if !valid_name {
            continue;
        }

        if !calls.iter().any(|name| name == candidate) {
            calls.push(candidate.to_string());
        }

        if calls.len() >= max_calls {
            break;
        }
    }

    if in_json_block {
        flush_json_block(&mut json_block, &mut calls);
    }

    calls
}

/// Execute model tool calls
#[cfg(test)]
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
fn execute_tool_calls(
    task: &str,
    subtask: &str,
    record_index: usize,
    calls: &[String],
) -> Vec<String> {
    // Simplified tool execution
    calls
        .iter()
        .map(|call| {
            format!(
                "Executed {} for task {} (subtask: {}, index: {})",
                call, task, subtask, record_index
            )
        })
        .collect()
}

/// A detected repeated task pattern in a conversation.
/// Used by P3 to proactively propose skill creation.
#[allow(dead_code)] // F-GAP-49
pub(crate) struct DetectedTaskPattern {
    /// Suggested skill name
    pub(crate) name: String,
    /// Suggested skill description
    pub(crate) description: String,
    /// How many times the pattern was observed
    pub(crate) occurrence_count: usize,
    /// The keyword cluster that identifies this pattern
    pub(crate) keywords: Vec<String>,
}

/// Detect repeated task patterns across user messages.
///
/// Analyzes all user messages for common keyword clusters that indicate
/// the same type of task is being requested multiple times.
/// Returns `Some(DetectedTaskPattern)` when a pattern appears 3+ times.
pub(crate) fn detect_repeated_task_pattern(messages: &[&str]) -> Option<DetectedTaskPattern> {
    if messages.len() < 3 {
        return None;
    }

    // Define keyword clusters for common task types as owned strings
    let task_clusters: Vec<(Vec<&str>, &str, &str)> =
        vec![
        (
            vec!["refactor", "restructure", "reorganize", "clean up", "cleanup", "technical debt"],
            "code-refactoring",
            "Refactors and restructures code to improve maintainability and reduce technical debt",
        ),
        (
            vec!["test", "unit test", "integration test", "e2e", "test coverage", "assert"],
            "testing",
            "Creates and runs tests including unit, integration, and end-to-end tests",
        ),
        (
            vec!["document", "readme", "docstring", "comment", "documentation", "docs"],
            "documentation",
            "Generates and updates documentation including README, docstrings, and technical docs",
        ),
        (
            vec!["debug", "fix", "bug", "issue", "error", "crash", "failing", "broken"],
            "bug-fixing",
            "Diagnoses and fixes bugs, errors, and crashes in the codebase",
        ),
        (
            vec!["optimize", "performance", "slow", "bottleneck", "speed up", "faster"],
            "performance-optimization",
            "Optimizes code performance by identifying and fixing bottlenecks",
        ),
        (
            vec!["api", "endpoint", "route", "rest", "graphql", "grpc"],
            "api-development",
            "Designs, implements, and documents API endpoints and integrations",
        ),
        (
            vec!["review", "code review", "audit", "inspect", "check quality"],
            "code-review",
            "Reviews code for quality, security, and adherence to best practices",
        ),
        (
            vec!["deploy", "ci/cd", "pipeline", "release", "rollout", "rollback"],
            "deployment",
            "Manages deployment, CI/CD pipelines, and release processes",
        ),
        (
            vec!["migrate", "migration", "upgrade", "port", "convert", "transpile"],
            "migration",
            "Migrates code between frameworks, languages, or versions",
        ),
        (
            vec!["config", "configure", "setup", "install", "initialize", "bootstrap"],
            "configuration",
            "Handles configuration, setup, and initialization of projects and tools",
        ),
    ];

    // Count how many messages match each cluster
    let mut cluster_hits: Vec<(usize, Vec<&str>, &str, &str)> = task_clusters
        .into_iter()
        .map(|(keywords, name, description)| {
            let count = messages
                .iter()
                .filter(|msg| {
                    let lower = msg.to_lowercase();
                    keywords.iter().any(|kw| lower.contains(kw))
                })
                .count();
            (count, keywords, name, description)
        })
        .collect();

    // Sort by hit count descending
    cluster_hits.sort_by_key(|b| std::cmp::Reverse(b.0));

    // Return the best match if it appears 3+ times
    if !cluster_hits.is_empty() {
        let (count, keywords, name, description) = cluster_hits.swap_remove(0);
        if count >= 3 {
            return Some(DetectedTaskPattern {
                name: name.to_string(),
                description: description.to_string(),
                occurrence_count: count,
                keywords: keywords.into_iter().map(|s| s.to_string()).collect(),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_empty_response() {
        let calls = extract_tool_calls_from_response("", 5);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_extract_tool_call_marker() {
        let calls = extract_tool_calls_from_response("__tool_call__:search_web", 5);
        assert_eq!(calls, vec!["search_web"]);
    }

    #[test]
    fn test_extract_json_tool_call() {
        let response = r#"```json
{"tool_call": "search_web"}
```"#;
        let calls = extract_tool_calls_from_response(response, 5);
        assert_eq!(calls, vec!["search_web"]);
    }

    #[test]
    fn test_detect_no_pattern() {
        let result = detect_repeated_task_pattern(&["hello", "world"]);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_refactor_pattern() {
        let messages = &[
            "please refactor this code",
            "restructure the module",
            "clean up the technical debt",
        ];
        let result = detect_repeated_task_pattern(messages);
        assert!(result.is_some());
        let pattern = result.unwrap();
        assert_eq!(pattern.name, "code-refactoring");
        assert_eq!(pattern.occurrence_count, 3);
    }
}
