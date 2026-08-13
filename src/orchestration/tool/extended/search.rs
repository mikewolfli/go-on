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

        // Stream the file line-by-line instead of buffering the whole file:
        // a model-picked 10GB file must not OOM the process. Each line is
        // bounded (oversized lines are skipped); line numbers stay true to
        // the file.
        use std::io::{BufRead, Read};
        const MAX_LINE_BYTES: usize = 1024 * 1024; // 1 MiB per line
        if let Ok(file) = std::fs::File::open(&path) {
            let mut reader = std::io::BufReader::new(file);
            let mut line_num = 0u64;
            let mut line_buf: Vec<u8> = Vec::new();
            loop {
                line_buf.clear();
                let n = (&mut reader)
                    .take(MAX_LINE_BYTES as u64 + 1)
                    .read_until(b'\n', &mut line_buf)
                    .unwrap_or(0);
                if n == 0 {
                    break; // EOF
                }
                line_num += 1;
                if line_buf.len() > MAX_LINE_BYTES {
                    // Shared drain: advance to the next line boundary without
                    // buffering the line or consuming the next line's prefix.
                    crate::shared::bufread::drain_to_newline(&mut reader);
                    continue;
                }
                if *state.total_matches >= state.max_matches {
                    break;
                }
                while matches!(line_buf.last(), Some(b'\n') | Some(b'\r')) {
                    line_buf.pop();
                }
                // Match on the borrowed slice first; only allocate an owned
                // String when the line actually matches (avoids a 1 MiB-per-
                // line clone for every scanned line).
                let Ok(line_ref) = std::str::from_utf8(&line_buf) else {
                    continue;
                };
                if regex.is_match(line_ref) {
                    *state.total_matches += 1;
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    state.matches.push(serde_json::json!({
                        "file": relative.to_string_lossy(),
                        "line": line_num,
                        "content": line_ref,
                    }));
                }
            }
        }
    }
    Ok(())
}

// FindFilesTool has been merged into SearchFilesTool (tool/mod.rs registers
// the `find_files` alias). Result sets are bounded by the `max_results`
// argument, implemented in tool/builtin_tools.rs + tool/file_walk.rs.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-grep".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn grep_oversized_line_is_drained_without_corrupting_next_line() {
        // Regression (mirror of read_lines): an oversized first line must be
        // drained to the newline only — a naive read() would consume the next
        // line's prefix and the match below would silently miss content.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join("big.txt");
        let mut content = "x".repeat(1024 * 1024 + 100);
        content.push_str("\nsentinel-尾行\n");
        std::fs::write(&path, content).expect("write file");

        let input = tool_input(serde_json::json!({
            "pattern": "sentinel",
            "directory": tmp.path().to_str().unwrap(),
        }));
        let tool = GrepTool;
        let result = tool.run(&input).expect("run");
        assert!(result.success);
        let payload = result.result.expect("payload");
        let matches = payload["matches"].as_array().expect("matches array");
        assert_eq!(matches.len(), 1, "only the sentinel line matches");
        assert_eq!(
            matches[0]["line"].as_u64().unwrap(),
            2,
            "line numbers must not drift across the oversized line"
        );
        assert_eq!(matches[0]["content"].as_str().unwrap(), "sentinel-尾行");
    }

    #[test]
    fn grep_borrowed_matching_finds_case_insensitive_hit() {
        // The match path works on the borrowed line slice (no per-line owned
        // String clone unless the line matches).
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("a.txt"), "Hello World\nno match here\n")
            .expect("write file");

        let input = tool_input(serde_json::json!({
            "pattern": "hello",
            "directory": tmp.path().to_str().unwrap(),
        }));
        let result = GrepTool.run(&input).expect("run");
        let payload = result.result.expect("payload");
        let matches = payload["matches"].as_array().expect("matches");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["line"].as_u64().unwrap(), 1);
        assert_eq!(matches[0]["content"].as_str().unwrap(), "Hello World");
    }

    #[test]
    fn grep_reports_truncation_at_match_cap() {
        // max_matches = 1000: a pattern matching every line must stop at the
        // cap and report `truncated` (explicit, not silent).
        let tmp = TempDir::new().expect("temp dir");
        for i in 0..1200 {
            std::fs::write(tmp.path().join(format!("f{i}.txt")), "match\n").expect("write file");
        }

        let input = tool_input(serde_json::json!({
            "pattern": "match",
            "directory": tmp.path().to_str().unwrap(),
        }));
        let result = GrepTool.run(&input).expect("run");
        let payload = result.result.expect("payload");
        assert_eq!(payload["matches"].as_array().unwrap().len(), 1000);
        assert!(payload["truncated"].as_bool().unwrap());
    }
}
