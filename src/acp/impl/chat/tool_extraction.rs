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

/// Detect repeated task patterns across user messages.
///
/// Analyzes all user messages for common keyword clusters that indicate
/// the same type of task is being requested multiple times.
/// Returns `true` when a repeated task pattern appears 3+ times.
pub(crate) fn detect_repeated_task_pattern(messages: &[&str]) -> bool {
    if messages.len() < 3 {
        return false;
    }

    // Define keyword clusters for common task types
    let task_clusters: Vec<Vec<&str>> = vec![
        vec![
            "refactor",
            "restructure",
            "reorganize",
            "clean up",
            "cleanup",
            "technical debt",
        ],
        vec![
            "test",
            "unit test",
            "integration test",
            "e2e",
            "test coverage",
            "assert",
        ],
        vec![
            "document",
            "readme",
            "docstring",
            "comment",
            "documentation",
            "docs",
        ],
        vec![
            "debug", "fix", "bug", "issue", "error", "crash", "failing", "broken",
        ],
        vec![
            "optimize",
            "performance",
            "slow",
            "bottleneck",
            "speed up",
            "faster",
        ],
        vec!["api", "endpoint", "route", "rest", "graphql", "grpc"],
        vec!["review", "code review", "audit", "inspect", "check quality"],
        vec![
            "deploy", "ci/cd", "pipeline", "release", "rollout", "rollback",
        ],
        vec![
            "migrate",
            "migration",
            "upgrade",
            "port",
            "convert",
            "transpile",
        ],
        vec![
            "config",
            "configure",
            "setup",
            "install",
            "initialize",
            "bootstrap",
        ],
    ];

    // Count how many messages match each cluster
    let mut cluster_hits: Vec<usize> = task_clusters
        .into_iter()
        .map(|keywords| {
            messages
                .iter()
                .filter(|msg| {
                    let lower = msg.to_lowercase();
                    keywords.iter().any(|kw| lower.contains(kw))
                })
                .count()
        })
        .collect();

    // Sort by hit count descending
    cluster_hits.sort_by_key(|b| std::cmp::Reverse(*b));

    // Return true if the best match appears 3+ times
    !cluster_hits.is_empty() && cluster_hits[0] >= 3
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
        assert!(!result);
    }

    #[test]
    fn test_detect_refactor_pattern() {
        let messages = &[
            "please refactor this code",
            "restructure the module",
            "clean up the technical debt",
        ];
        let result = detect_repeated_task_pattern(messages);
        assert!(result);
    }
}
