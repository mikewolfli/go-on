use std::fs;

use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogChunk {
    pub path: String,
    pub lines: Vec<String>,
    pub total_lines_read: usize,
}

fn mask_sensitive_line(line: &str) -> String {
    let sensitive_keywords = [
        "api_key",
        "apikey",
        "token",
        "authorization",
        "secret",
        "password",
    ];
    let lower = line.to_lowercase();

    // 检查是否包含任何敏感关键字
    let has_sensitive = sensitive_keywords.iter().any(|kw| lower.contains(kw));
    if !has_sensitive {
        return line.to_string();
    }

    // 按空格分割 token，对每个匹配敏感关键的 token 进行 mask
    line.split_whitespace()
        .map(|token| {
            let token_lower = token.to_lowercase();
            if sensitive_keywords.iter().any(|kw| token_lower.contains(kw)) {
                // 找到 '=' 或 ':' 的位置，只保留 key 部分
                if let Some(idx) = token.find('=') {
                    format!("{}=***", &token[..idx])
                } else if let Some(idx) = token.find(':') {
                    format!("{}:***", &token[..idx])
                } else {
                    "***".to_string()
                }
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[tauri::command]
pub fn read_recent_logs(
    state: State<'_, AppState>,
    log_path: Option<String>,
    lines: Option<usize>,
    mask_sensitive: Option<bool>,
) -> Result<LogChunk, String> {
    let path = {
        let inner = state
            .0
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?;
        match log_path {
            Some(p) => std::path::PathBuf::from(p),
            None => std::path::PathBuf::from(&inner.config.log_path),
        }
    }; // 锁在此释放
    let line_limit = lines.unwrap_or(200);
    let should_mask = mask_sensitive.unwrap_or(true);

    let content = fs::read_to_string(&path).unwrap_or_default();
    let mut chunks: Vec<String> = content.lines().map(|x| x.to_string()).collect();
    let total = chunks.len();

    if chunks.len() > line_limit {
        chunks = chunks.split_off(chunks.len() - line_limit);
    }

    if should_mask {
        chunks = chunks
            .into_iter()
            .map(|line| mask_sensitive_line(&line))
            .collect();
    }

    Ok(LogChunk {
        path: path.to_string_lossy().to_string(),
        lines: chunks,
        total_lines_read: total,
    })
}
