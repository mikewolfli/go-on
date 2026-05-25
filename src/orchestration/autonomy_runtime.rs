//! Shared autonomy runtime helpers for CLI/ACP execution loops.
//!
//! This module centralizes token protocol parsing and tool-observation
//! follow-up message construction so all entrypoints keep the same behavior.

pub const TOKEN_TOOL_CALL_PREFIX: &str = "__tool_call__:";
pub const TOKEN_MODEL_USED_PREFIX: &str = "__model_used__:";
pub const TOKEN_THINKING_PREFIX: &str = "__thinking__";
pub const TOOL_EXECUTION_RESULTS_OPEN: &str = "[Tool execution results]";
pub const TOOL_EXECUTION_RESULTS_CLOSE: &str = "[/Tool execution results]";

pub fn build_tool_call_token(tool_name: &str, arguments_json: &str) -> String {
    format!("{}{}:{}", TOKEN_TOOL_CALL_PREFIX, tool_name, arguments_json)
}

pub fn build_model_used_token(model_name: &str) -> String {
    format!("{}{}", TOKEN_MODEL_USED_PREFIX, model_name)
}

pub fn build_thinking_token(thinking: &str, content: Option<&str>) -> String {
    let tail = content.unwrap_or_default();
    format!("{}{}{}", TOKEN_THINKING_PREFIX, thinking, tail)
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
    if is_error {
        format!("[Tool error: {}]\n{}\n[/Tool error]", tool_name, payload)
    } else {
        format!("[Tool result: {}]\n{}\n[/Tool result]", tool_name, payload)
    }
}

pub fn build_tool_execution_followup_message(
    tool_results: &[String],
    final_answer_only: bool,
) -> String {
    let combined = tool_results.join("\n");
    let final_clause = if final_answer_only {
        "If the task is complete, provide only the final answer."
    } else {
        "If the task is complete, provide a summary."
    };

    format!(
        "{}\n{}\n{}\n\nPlease continue based on the tool results above. {}",
        TOOL_EXECUTION_RESULTS_OPEN, combined, TOOL_EXECUTION_RESULTS_CLOSE, final_clause
    )
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

        let thinking_token = build_thinking_token("reasoning...", Some("final"));
        assert_eq!(
            parse_thinking_token(&thinking_token),
            Some("reasoning...final")
        );
    }
}
