//! Runtime Hub discovery file management.
//!
//! When the Hub starts, it writes its connection info to a discovery file.
//! Clients find the Hub by reading this file from a well-known path.
//!
//! Path resolution order:
//!   1. $GO_ON_HUB_DISCOVERY_FILE (env)
//!   2. $XDG_RUNTIME_DIR/go-on/hub/discovery.json
//!   3. $TMPDIR/go-on-hub/discovery.json
//!
//! # Dead-code note
//! This module is a design reserve for future multi-process architecture.
//! See parent `hub/mod.rs` for the full rationale.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Content of the hub discovery file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubDiscovery {
    /// Unique hub identifier.
    pub hub_id: String,
    /// Transport protocol (always "loopback_http").
    pub transport: String,
    /// Local endpoint (e.g. "http://127.0.0.1:34567").
    pub endpoint: String,
    /// Opaque identity token (hex). Currently a random value: no signature
    /// verification is performed yet (handshake is an identity echo).
    pub public_key: String,
    /// Process PID.
    pub pid: u32,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
}

impl HubDiscovery {
    /// Return the default discovery file path.
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("GO_ON_HUB_DISCOVERY_FILE") {
            return PathBuf::from(path);
        }
        // XDG_RUNTIME_DIR takes precedence on Linux.
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            return PathBuf::from(dir)
                .join("go-on")
                .join("hub")
                .join("discovery.json");
        }
        // Fallback to temp directory.
        std::env::temp_dir()
            .join("go-on-hub")
            .join("discovery.json")
    }

    /// Write this discovery info to `path`.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create hub dir: {}", parent.display()))?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, &content)
            .with_context(|| format!("write hub discovery: {}", path.display()))?;
        Ok(())
    }
}
