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

use std::path::PathBuf;

/// Return the go-on data directory: `GO_ON_DATA_DIR` env override, or
/// `./.goon` (project-relative). The default deliberately matches the
/// historical literal so existing data is not relocated.
pub fn goon_data_dir() -> PathBuf {
    std::env::var("GO_ON_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".goon"))
}

/// Join a relative path (e.g. `"learning"`, `"evolution/history.ndjson"`)
/// under the canonical go-on data directory.
pub fn goon_subdir(rel: &str) -> PathBuf {
    goon_data_dir().join(rel)
}
