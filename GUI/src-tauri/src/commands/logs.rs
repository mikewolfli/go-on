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
    let mut masked = line.to_string();
    let patterns = [
        "api_key",
        "apikey",
        "token",
        "authorization",
        "secret",
        "password",
    ];

    for p in patterns {
        if masked.to_lowercase().contains(p) {
            if let Some(idx) = masked.find('=') {
                masked = format!("{}=***", &masked[..idx]);
            } else if let Some(idx) = masked.find(':') {
                masked = format!("{}: ***", &masked[..idx]);
            }
        }
    }

    masked
}

#[tauri::command]
pub fn read_recent_logs(
    state: State<'_, AppState>,
    log_path: Option<String>,
    lines: Option<usize>,
    mask_sensitive: Option<bool>,
) -> Result<LogChunk, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    let path = log_path.unwrap_or_else(|| inner.config.log_path.clone());
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
        path,
        lines: chunks,
        total_lines_read: total,
    })
}
