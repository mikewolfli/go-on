//! Shared autonomy runtime helpers for CLI/ACP execution loops.
//!
//! This module centralizes token protocol parsing and tool-observation
//! follow-up message construction so all entrypoints keep the same behavior.

pub const TOKEN_TOOL_CALL_PREFIX: &str = "__tool_call__:";
pub const TOKEN_MODEL_USED_PREFIX: &str = "__model_used__:";
pub const TOKEN_THINKING_PREFIX: &str = "__thinking__";
pub const TOKEN_FINISH_REASON_PREFIX: &str = "__finish_reason__:";
pub const TOKEN_USAGE_PREFIX: &str = "__usage__:";

/// Reasoning/thinking start marker — sent as a single-character token
/// to mark the beginning of a reasoning block in streaming output.
/// Uses ASCII Record Separator (0x1E).
pub const REASONING_START: &str = "\u{1E}";
/// Reasoning/thinking end marker — sent as a single-character token
/// to mark the end of a reasoning block in streaming output.
/// Uses ASCII Unit Separator (0x1F).
pub const REASONING_END: &str = "\u{1F}";

pub const TOOL_EXECUTION_RESULTS_OPEN: &str = "[Tool execution results]";
pub const TOOL_EXECUTION_RESULTS_CLOSE: &str = "[/Tool execution results]";

pub fn build_tool_call_token(tool_name: &str, arguments_json: &str) -> String {
    let mut token = String::with_capacity(
        TOKEN_TOOL_CALL_PREFIX.len() + tool_name.len() + 1 + arguments_json.len(),
    );
    token.push_str(TOKEN_TOOL_CALL_PREFIX);
    token.push_str(tool_name);
    token.push(':');
    token.push_str(arguments_json);
    token
}

pub fn build_model_used_token(model_name: &str) -> String {
    let mut token = String::with_capacity(TOKEN_MODEL_USED_PREFIX.len() + model_name.len());
    token.push_str(TOKEN_MODEL_USED_PREFIX);
    token.push_str(model_name);
    token
}

pub fn build_thinking_token(thinking: &str) -> String {
    let mut token = String::with_capacity(TOKEN_THINKING_PREFIX.len() + thinking.len());
    token.push_str(TOKEN_THINKING_PREFIX);
    token.push_str(thinking);
    token
}

pub fn parse_tool_call_token(token: &str) -> Option<(&str, &str)> {
    let payload = token.strip_prefix(TOKEN_TOOL_CALL_PREFIX)?;
    let (tool_name, args) = payload.split_once(':')?;
    let tool_name = tool_name.trim();
    if tool_name.is_empty() {
        return None;
    }
    Some((tool_name, args))
}

/// Classification of a single streamed agent token against the shared
/// token vocabulary (single source; the CLI/ACP collection loops previously
/// re-implemented this chain three times).
#[derive(Debug)]
pub enum AgentToken {
    /// Visible response content.
    Content(String),
    /// Reasoning text (the `__thinking__` prefix is stripped).
    Reasoning(String),
    /// A tool-call token `(tool_name, arguments_json)`.
    ToolCall(String, String),
    /// Model-used announcement (model id).
    ModelUsed(String),
    /// Finish-reason / usage telemetry control token.
    Telemetry,
    /// Structured reasoning start/end marker (control char).
    ReasoningMarker,
}

/// Classify a streamed agent token.
pub fn classify_agent_token(token: &str) -> AgentToken {
    if let Some(model_id) = token.strip_prefix(TOKEN_MODEL_USED_PREFIX) {
        return AgentToken::ModelUsed(model_id.trim().to_string());
    }
    if let Some((tool_name, tool_args)) = parse_tool_call_token(token) {
        return AgentToken::ToolCall(tool_name.to_string(), tool_args.to_string());
    }
    if token == REASONING_START || token == REASONING_END {
        return AgentToken::ReasoningMarker;
    }
    if let Some(reasoning_token) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
        return AgentToken::Reasoning(reasoning_token.to_string());
    }
    if token.starts_with(TOKEN_FINISH_REASON_PREFIX) || token.starts_with(TOKEN_USAGE_PREFIX) {
        return AgentToken::Telemetry;
    }
    AgentToken::Content(token.to_string())
}

#[cfg(test)]
pub fn parse_model_used_token(token: &str) -> Option<&str> {
    token
        .strip_prefix(TOKEN_MODEL_USED_PREFIX)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
pub fn parse_thinking_token(token: &str) -> Option<&str> {
    token.strip_prefix(TOKEN_THINKING_PREFIX)
}

pub fn build_tool_result_block(tool_name: &str, payload: &str, is_error: bool) -> String {
    let prefix = if is_error {
        "[Tool error: "
    } else {
        "[Tool result: "
    };
    let close = if is_error {
        "\n[/Tool error]"
    } else {
        "\n[/Tool result]"
    };
    let mut s =
        String::with_capacity(prefix.len() + tool_name.len() + 2 + payload.len() + close.len());
    s.push_str(prefix);
    s.push_str(tool_name);
    s.push_str("]\n");
    s.push_str(payload);
    s.push_str(close);
    s
}

pub fn build_tool_execution_followup_message(
    tool_results: &[String],
    final_answer_only: bool,
) -> String {
    let final_clause = if final_answer_only {
        "If the task is complete, provide only the final answer."
    } else {
        "If the task is complete, provide a summary."
    };

    // Avoid intermediate allocation from `join`: push parts into a single String.
    let mut msg = String::with_capacity(256 + tool_results.iter().map(|s| s.len()).sum::<usize>());
    msg.push_str(TOOL_EXECUTION_RESULTS_OPEN);
    msg.push('\n');
    for result in tool_results {
        msg.push_str(result);
        msg.push('\n');
    }
    msg.push_str(TOOL_EXECUTION_RESULTS_CLOSE);
    msg.push_str("\n\nPlease continue based on the tool results above. ");
    msg.push_str(final_clause);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_call_supports_colons_inside_json() {
        let token = r#"__tool_call__:read_file:{"path":"a:b:c.txt"}"#;
        let (tool, args) = parse_tool_call_token(token).expect("tool call");
        assert_eq!(tool, "read_file");
        assert_eq!(args, r#"{"path":"a:b:c.txt"}"#);
    }

    #[test]
    fn followup_message_contract_is_stable() {
        let blocks = vec!["[Tool result: read_file]\nok\n[/Tool result]".to_string()];
        let message = build_tool_execution_followup_message(&blocks, true);
        assert!(message.contains(TOOL_EXECUTION_RESULTS_OPEN));
        assert!(message.contains("provide only the final answer"));
    }

    #[test]
    fn build_tool_call_token_uses_shared_prefix() {
        let token = build_tool_call_token("read_file", r#"{"path":"a.txt"}"#);
        assert_eq!(token, r#"__tool_call__:read_file:{"path":"a.txt"}"#);
    }

    #[test]
    fn parse_model_and_thinking_tokens() {
        let model_token = build_model_used_token("gpt-4.1");
        assert_eq!(parse_model_used_token(&model_token), Some("gpt-4.1"));

        let thinking_token = build_thinking_token("reasoning...");
        assert_eq!(parse_thinking_token(&thinking_token), Some("reasoning..."));
    }
}
