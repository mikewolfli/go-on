//! Backend binary management — discovery, spawning, and config generation.
//!
//! Most backend lifecycle logic (spawn, config generation, diagnostics) now
//! lives in `app/actions.rs` as `GoOnApp` methods. This module retains only
//! the small utilities that are called from `app/mod.rs`.

use crate::app::actions::find_backend_binary;

fn backend_log_path() -> Option<std::path::PathBuf> {
    find_backend_binary().and_then(|path| path.parent().map(|p| p.join("backend.log")))
}

/// Check if the backend's log file contains "Address already in use".
/// This indicates a port conflict that should be surfaced to the user.
pub(crate) fn backend_log_has_addr_in_use() -> bool {
    backend_log_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|s| s.contains("Address already in use"))
}
