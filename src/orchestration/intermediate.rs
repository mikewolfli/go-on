//! Intermediate file management for agent-executed tasks.
//!
//! Each task gets a scoped directory under `.goon/intermediates/<task_id>/`
//! where tools can place intermediate/temporary files without polluting
//! the user's project tree. Directories are cleaned up on task completion.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Global base directory for intermediates, resolved once at startup.
static INTERMEDIATE_BASE: OnceLock<PathBuf> = OnceLock::new();

/// Initialize the intermediate base directory.
/// Call once during startup. Creates `.goon/intermediates/` if it doesn't exist.
pub fn init_intermediate_base(project_root: &Path) -> Result<PathBuf> {
    let base = project_root.join(".goon").join("intermediates");
    std::fs::create_dir_all(&base)?;
    INTERMEDIATE_BASE
        .set(base.clone())
        .map_err(|_| anyhow::anyhow!("INTERMEDIATE_BASE already initialized"))?;
    Ok(base)
}

/// Get the intermediate base directory (returns None if not initialized).
#[allow(dead_code)]
pub(crate) fn intermediate_base() -> Option<&'static PathBuf> {
    INTERMEDIATE_BASE.get()
}

/// Create a task-scoped intermediate directory and return its path.
/// Returns Ok(None) if the intermediate base has not been initialized
/// (e.g. during testing without bootstrap).
pub fn create_task_intermediate_dir(task_id: &str) -> Result<Option<PathBuf>> {
    let base = match INTERMEDIATE_BASE.get() {
        Some(b) => b.clone(),
        None => return Ok(None),
    };
    let dir = base.join(sanitize_task_id(task_id));
    std::fs::create_dir_all(&dir)?;
    Ok(Some(dir))
}

/// Remove a task's intermediate directory and all its contents.
#[allow(dead_code)]
pub(crate) fn cleanup_task_intermediates(task_id: &str) -> Result<()> {
    let base = match INTERMEDIATE_BASE.get() {
        Some(b) => b.clone(),
        None => return Ok(()),
    };
    let dir = base.join(sanitize_task_id(task_id));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Return the path to the task's intermediate directory (without creating it).
#[allow(dead_code)]
pub(crate) fn task_intermediate_dir(task_id: &str) -> Option<PathBuf> {
    Some(intermediate_base()?.join(sanitize_task_id(task_id)))
}

fn sanitize_task_id(task_id: &str) -> String {
    // Replace path separators with underscores to prevent directory traversal
    task_id.replace(['/', '\\', ':', ' ', '\n', '\r'], "_")
}
