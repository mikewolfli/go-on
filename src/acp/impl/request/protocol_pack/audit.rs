use super::*;
use crate::governance::hardening::GovernanceAction;

pub fn governance_action_label(action: GovernanceAction) -> &'static str {
    match action {
        GovernanceAction::Read => "read",
        GovernanceAction::Search => "search",
        GovernanceAction::Write => "write",
        GovernanceAction::Shell => "shell",
        GovernanceAction::Network => "network",
    }
}

pub fn audit_file_path_from_arguments(name: &str, arguments: &Value) -> String {
    for key in ["path", "filePath", "sourcePdfPath"] {
        if let Some(path) = arguments.get(key).and_then(Value::as_str) {
            return path.to_string();
        }
    }
    format!("tool:{name}")
}

pub fn is_rate_limited_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("rate limited")
        || normalized.contains("rate_limited")
        || normalized.contains("error.chat.rate_limited")
        || normalized.contains("too many requests")
}

pub fn normalize_rate_limited_message(message: &str) -> String {
    if message.to_ascii_lowercase().contains("rate limited") {
        message.to_string()
    } else {
        format!("rate limited: {message}")
    }
}
