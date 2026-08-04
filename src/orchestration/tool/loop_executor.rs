//! Loop executor — file-walk helpers used by `SearchFilesTool`.
//!
//! The former `execute_loop` / `execute_loop_async` tool orchestration loop and
//! its `ToolRecommender` dependency had zero production callers (the production
//! tool execution stack is `tool::executor::execute_tools_concurrent`) and were
//! removed along with `tool/pipeline.rs` and `tool/recommender.rs`.

use glob::Pattern;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Recursively walk a directory tree and collect files matching the given
/// glob [`Pattern`]. Returns their full paths.
pub fn collect_matching_files(
    root: &Path,
    current: &Path,
    matcher: &Pattern,
    files: &mut Vec<String>,
) -> Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_matching_files(root, &path, matcher, files)?;
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        let candidate = relative.to_string_lossy().replace('\\', "/");
        if matcher.matches(&candidate) || matcher.matches_path(relative) {
            files.push(path.to_string_lossy().to_string());
        }
    }
    Ok(())
}

/// Recursively walk a directory tree using `tokio::fs` and collect files
/// matching the given glob pattern. Returns their full paths.
pub async fn collect_matching_files_async(root: PathBuf, matcher: Pattern) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut dirs_to_visit = vec![root.clone()];

    while let Some(dir) = dirs_to_visit.pop() {
        let mut rd = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() {
                dirs_to_visit.push(path);
            } else {
                let relative = path.strip_prefix(&root).unwrap_or(&path);
                let candidate = relative.to_string_lossy().replace('\\', "/");
                if matcher.matches(&candidate) || matcher.matches_path(relative) {
                    files.push(path.to_string_lossy().to_string());
                }
            }
        }
    }

    Ok(files)
}
