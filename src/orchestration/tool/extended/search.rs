//! Search tools (grep, find_files)

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::t;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use glob::Pattern;
use regex::Regex;
use std::fs;
use std::path::Path;

// ── GrepTool ────────────────────────────────────────────────────────────────

pub struct GrepTool;

struct GrepCollectState<'a> {
    matches: &'a mut Vec<serde_json::Value>,
    files_scanned: &'a mut u64,
    total_matches: &'a mut u64,
    max_matches: u64,
}

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents using regex patterns"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let pattern = input.payload["pattern"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_pattern")))?;
        let directory = input.payload["directory"].as_str().unwrap_or(".");
        let include_pattern = input.payload["include"].as_str();
        let case_sensitive = input.payload["case_sensitive"].as_bool().unwrap_or(false);

        let regex = if case_sensitive {
            Regex::new(pattern).context("invalid regex pattern")?
        } else {
            Regex::new(&format!("(?i){}", pattern)).context("invalid regex pattern")?
        };

        let root = sanitize_path(input, directory)?;
        let glob_matcher = include_pattern.and_then(|p| Pattern::new(p).ok());

        let mut matches: Vec<serde_json::Value> = Vec::new();
        let mut files_scanned = 0u64;
        let mut total_matches = 0u64;
        let max_matches = 1000u64;

        let mut state = GrepCollectState {
            matches: &mut matches,
            files_scanned: &mut files_scanned,
            total_matches: &mut total_matches,
            max_matches,
        };

        collect_grep_matches(&root, &root, &regex, &glob_matcher, &mut state)?;

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "matches": matches,
                "files_scanned": files_scanned,
                "total_matches": total_matches,
                "truncated": total_matches >= max_matches,
            })),
            error: None,
            verification: Some("grep_completed".to_string()),
            audit_log: Some(format!(
                "Grep '{}' in '{}': {} matches in {} files",
                pattern, directory, total_matches, files_scanned
            )),
            pua_report: Some(tool_execution_report("grep", Some("grep_completed"))),
        })
    }
}

fn collect_grep_matches(
    root: &Path,
    current: &Path,
    regex: &Regex,
    glob_matcher: &Option<Pattern>,
    state: &mut GrepCollectState<'_>,
) -> Result<()> {
    if *state.total_matches >= state.max_matches {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            // Skip common non-source directories
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if dir_name == ".git" || dir_name == "target" || dir_name == "node_modules" {
                continue;
            }
            collect_grep_matches(root, &path, regex, glob_matcher, state)?;
            continue;
        }

        // Apply glob filter if provided
        if let Some(ref matcher) = glob_matcher {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if !matcher.matches_path(relative) {
                continue;
            }
        }

        *state.files_scanned += 1;

        // Try to read file as UTF-8 text
        if let Ok(content) = fs::read_to_string(&path) {
            for (line_num, line) in content.lines().enumerate() {
                if *state.total_matches >= state.max_matches {
                    break;
                }
                if regex.is_match(line) {
                    *state.total_matches += 1;
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    state.matches.push(serde_json::json!({
                        "file": relative.to_string_lossy(),
                        "line": line_num + 1,
                        "content": line,
                    }));
                }
            }
        }
    }
    Ok(())
}

// FindFilesTool has been merged into SearchFilesTool in tool/mod.rs.
// Use "search_files" (or alias "find_files") with max_results parameter.
// The collect_matching_files_bounded function is now in tool/mod.rs.
