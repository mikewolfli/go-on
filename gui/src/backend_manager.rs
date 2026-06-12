//! Backend binary management — discovery, spawning, and config generation.
//!
//! Most backend lifecycle logic (spawn, config generation, diagnostics) now
//! lives in `app/actions.rs` as `GoOnApp` methods. This module retains only
//! the small utilities that are called from `app/mod.rs`.

/// Find the go-on backend binary path relative to the GUI executable.
fn find_backend_binary() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe_name = if cfg!(target_os = "windows") {
        "go-on.exe"
    } else {
        "go-on"
    };
    let mut candidates = vec![
        exe_dir.join("backend").join(exe_name),
        exe_dir.join(exe_name),
    ];
    // Also search in Resources/backend (macOS .app bundle layout)
    if let Some(resources) = exe_dir.parent().map(|p| p.join("Resources")) {
        candidates.push(resources.join("backend").join(exe_name));
        candidates.push(resources.join(exe_name));
    }
    for path in &candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(exe_name);
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

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
