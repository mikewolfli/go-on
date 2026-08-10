//! Canonical go-on data directory resolution.
//!
//! All workspace-scoped go-on state (learning records, knowledge, evolution
//! history, metacognitive snapshots, chat sessions, memory tiers) used to be
//! scattered across 8+ call sites with different literal strings (".goon",
//! ".goon/memory/", "$HOME/.goon", env overrides). This module is the single
//! resolver:
//!
//! - `goon_data_dir()` — `GO_ON_DATA_DIR` env override → default `./.goon`
//!   (project-relative, same as the historical default so existing data
//!   directories are not silently relocated).
//! - `goon_subdir(rel)` — join a relative path under the data dir.
//!
//! # Deliberately excluded
//!
//! Global audit logs (`src/governance/audit.rs`) resolve to `$HOME/.goon`
//! independently — they are process-global data and must NOT follow the
//! project-relative data dir (moving them would lose the tamper-evident
//! chain history). See `dirs_or_fallback` in governance/audit.rs.
//!
//! # Backward compatibility
//!
//! `GO_ON_MEMORY_PATH` (memory_bridge.rs) remains a separate override for the
//! memory subtree only; it is layered on top of this resolver.

use std::path::{Path, PathBuf};

/// Return the go-on data directory: `GO_ON_DATA_DIR` env override, or
/// `./.goon` (project-relative). The default deliberately matches the
/// historical literal so existing data is not relocated.
pub fn goon_data_dir() -> PathBuf {
    std::env::var("GO_ON_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".goon"))
}

/// Resolve the workspace-scoped go-on data root.
///
/// Unified rule for every subsystem that persists project-scoped state
/// (chat sessions, reinforcement artifacts, learning, metacognitive):
///
/// - `GO_ON_DATA_DIR` env override wins over everything else;
/// - otherwise, when an explicit `config_path` is supplied (e.g. `-c` points
///   elsewhere than the CWD), the data root lives next to the config file
///   (`<config-dir>/.goon`);
/// - otherwise it defaults to `./.goon` (CWD-relative, matching the
///   historical literal).
///
/// Global process data (`~/.goon` audit logs in governance/audit.rs) is
/// deliberately NOT routed through here — it is process-global and must not
/// follow the project-relative data dir.
pub fn resolve_goon_root(config_path: Option<&Path>) -> PathBuf {
    if let Ok(override_dir) = std::env::var("GO_ON_DATA_DIR") {
        return PathBuf::from(override_dir);
    }
    let base = config_path
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    base.join(".goon")
}

/// Join a relative path (e.g. `"learning"`, `"evolution/history.ndjson"`)
/// under the canonical go-on data directory.
pub fn goon_subdir(rel: &str) -> PathBuf {
    goon_data_dir().join(rel)
}
