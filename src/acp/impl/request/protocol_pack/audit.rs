use super::*;

pub fn audit_file_path_from_arguments(name: &str, arguments: &Value) -> String {
    for key in ["path", "filePath", "sourcePdfPath"] {
        if let Some(path) = arguments.get(key).and_then(Value::as_str) {
            return path.to_string();
        }
    }
    format!("tool:{name}")
}

pub fn is_rate_limited_message(message: &str) -> bool {
    crate::agents::agent::is_rate_limit_error(message)
        || message.contains("rate_limited")
        || message.contains("error.chat.rate_limited")
}

pub fn normalize_rate_limited_message(message: &str) -> String {
    if message.to_ascii_lowercase().contains("rate limited") {
        message.to_string()
    } else {
        format!("rate limited: {message}")
    }
}
