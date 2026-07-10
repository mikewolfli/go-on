//! Runtime Hub discovery file management.
//!
//! When the Hub starts, it writes its connection info to a discovery file.
//! Clients find the Hub by reading this file from a well-known path.
//!
//! Path resolution order:
//!   1. $GO_ON_HUB_DISCOVERY_FILE (env)
//!   2. $XDG_RUNTIME_DIR/go-on/hub/discovery.json
//!   3. $TMPDIR/go-on-hub/discovery.json

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
    /// Hub's public verification key (hex).
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

    /// Read and parse the discovery file at `path`.
    pub fn read(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("read hub discovery: {}", path.display()))?;
        let discovery: HubDiscovery = serde_json::from_str(&content)
            .with_context(|| format!("parse hub discovery: {}", path.display()))?;
        Ok(discovery)
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

    /// Check whether the hub process is still alive using /proc (Linux) or kill -0.
    /// Check whether the hub process is still alive.
    pub fn is_alive(&self) -> bool {
        if self.pid == 0 {
            return false;
        }
        #[cfg(unix)]
        {
            let output = std::process::Command::new("kill")
                .args(["-0", &self.pid.to_string()])
                .output();
            match output {
                Ok(o) => o.status.success(),
                Err(_) => true, // assume alive if kill command unavailable
            }
        }
        #[cfg(not(unix))]
        {
            // Windows: use tasklist /FI "PID eq {pid}"
            let output = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {}", self.pid)])
                .output();
            match output {
                Ok(o) => {
                    let out = String::from_utf8_lossy(&o.stdout);
                    out.contains(&self.pid.to_string())
                }
                Err(_) => true,
            }
        }
    }
}
