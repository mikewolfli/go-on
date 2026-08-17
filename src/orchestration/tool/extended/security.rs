//! Security scanning tools.
//!
//! Scans project dependencies for known vulnerabilities using
//! OSV (Open Source Vulnerabilities) API.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::debug;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};

// ── OSV Cache ────────────────────────────────────────────────────────────────

/// Cache entry for OSV vulnerability results.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OsvCacheEntry {
    results: Vec<Value>,
    cached_at: u64, // unix timestamp in seconds
}

/// Simple JSON file-based cache for OSV queries.
struct OsvCache {
    path: PathBuf,
    ttl_secs: u64,
    entries: HashMap<String, OsvCacheEntry>,
}

/// Bounded cache size (oldest entry evicted beyond this) so the cache file
/// cannot grow without limit.
const MAX_OSV_CACHE_ENTRIES: usize = 5_000;

impl OsvCache {
    fn load_or_create(ttl_hours: u64) -> Self {
        let path = osv_cache_path();
        // Cap the read: the cache is self-written, but a corrupt/huge file on
        // disk must not OOM the tool (same input-side guard as the lock-file
        // extractors below).
        let entries = crate::orchestration::tool::exec_common::read_text_capped(
            &path,
            crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
        )
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
        Self {
            path,
            ttl_secs: ttl_hours * 3600,
            entries,
        }
    }

    fn get(&self, key: &str) -> Option<Vec<Value>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.entries.get(key).and_then(|entry| {
            if now - entry.cached_at < self.ttl_secs {
                Some(entry.results.clone())
            } else {
                None // expired
            }
        })
    }

    fn set(&mut self, key: String, results: Vec<Value>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.entries.insert(
            key,
            OsvCacheEntry {
                results,
                cached_at: now,
            },
        );
        // Bounded cache: evict the oldest entry when over capacity so a
        // long-running scanner (many ecosystems/packages) cannot grow the
        // cache file without bound (each `set` rewrites the whole file).
        if self.entries.len() > MAX_OSV_CACHE_ENTRIES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.cached_at)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        // Persist to disk; a write failure is logged (not silent) so a cache
        // that silently stops persisting is observable.
        if let Ok(data) = serde_json::to_string(&self.entries) {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&self.path, data) {
                tracing::warn!(
                    "osv cache: failed to persist {} entries to {}: {e}",
                    self.entries.len(),
                    self.path.display()
                );
            }
        }
    }
}

/// Determine the OSV cache file path (~/.cache/go-on/osv-cache.json).
fn osv_cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".cache")
        .join("go-on")
        .join("osv-cache.json")
}

pub struct SecurityScanTool;

impl Tool for SecurityScanTool {
    fn name(&self) -> &'static str {
        "security_scan"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input
            .payload
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let cache_ttl_hours = input
            .payload
            .get("cache_ttl_hours")
            .and_then(|v| v.as_u64())
            .unwrap_or(24);

        let base_dir = sanitize_path(input, directory)?;
        debug!(directory = %directory, cache_ttl_hours = %cache_ttl_hours, "tool: security_scan");

        // Discover dependency lock/manifest files.
        let lock_files = discover_lock_files(&base_dir);

        if lock_files.is_empty() {
            return Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "scanned": false,
                    "note": "No supported lock files found (Cargo.lock, \
                             package-lock.json, requirements.txt, go.sum)",
                    "vulnerabilities": [],
                })),
                error: None,
                verification: Some("security_scan_completed".to_string()),
                audit_log: Some("security_scan: no lock files found".to_string()),
                pua_report: Some(tool_execution_report(
                    "security_scan",
                    Some("security_scan_completed"),
                )),
            });
        }

        // Extract package list from discovered files.
        let mut all_packages: Vec<OsvPackage> = Vec::new();
        for lf in &lock_files {
            match extract_packages(lf) {
                Ok(pkgs) => {
                    debug!(file = %lf.display(), count = %pkgs.len(), "extracted packages");
                    all_packages.extend(pkgs);
                }
                Err(e) => {
                    debug!(file = %lf.display(), error = %e, "failed to extract packages");
                }
            }
        }

        if all_packages.is_empty() {
            return Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "scanned": true,
                    "lock_files": lock_files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                    "packages_found": 0,
                    "vulnerabilities": [],
                })),
                error: None,
                verification: Some("security_scan_completed".to_string()),
                audit_log: Some("security_scan: no packages extracted".to_string()),
                pua_report: Some(tool_execution_report(
                    "security_scan",
                    Some("security_scan_completed"),
                )),
            });
        }

        // Initialize OSV cache.
        let mut cache = OsvCache::load_or_create(cache_ttl_hours);

        // Query OSV API for each package (batched to avoid excessive requests),
        // using the local cache to avoid redundant HTTP queries.
        let mut vulnerabilities: Vec<Value> = Vec::new();
        for pkg in &all_packages {
            let cache_key = format!(
                "{}:{}@{}",
                pkg.ecosystem,
                pkg.name,
                pkg.version.as_deref().unwrap_or("*")
            );

            // Check cache first
            if let Some(cached) = cache.get(&cache_key) {
                debug!(package = %pkg.name, cached = %cached.len(), "cache hit for OSV query");
                vulnerabilities.extend(cached);
                continue;
            }

            debug!(package = %pkg.name, ecosystem = %pkg.ecosystem, "querying OSV");
            match query_osv(pkg) {
                Ok(mut vulns) => {
                    // Store in cache for future lookups
                    cache.set(cache_key, vulns.clone());
                    vulnerabilities.append(&mut vulns);
                }
                Err(e) => {
                    debug!(package = %pkg.name, error = %e, "OSV query failed");
                }
            }
        }

        debug!(
            lock_files = %lock_files.len(),
            packages = %all_packages.len(),
            vulns = %vulnerabilities.len(),
            "tool: security_scan complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "scanned": true,
                "lock_files": lock_files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
                "packages_found": all_packages.len(),
                "vulnerabilities": vulnerabilities,
            })),
            error: None,
            verification: Some("security_scan_completed".to_string()),
            audit_log: Some(format!(
                "security_scan: {} lock files, {} packages, {} vulnerabilities found",
                lock_files.len(),
                all_packages.len(),
                vulnerabilities.len()
            )),
            pua_report: Some(tool_execution_report(
                "security_scan",
                Some("security_scan_completed"),
            )),
        })
    }
}

// ── Data structures ───────────────────────────────────────────────────────

/// A package identifier for OSV querying.
#[derive(Debug, Clone)]
struct OsvPackage {
    name: String,
    ecosystem: String,
    version: Option<String>,
}

// ── Lock file discovery ───────────────────────────────────────────────────

/// Discover dependency lock/manifest files in the given directory.
fn discover_lock_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for name in &[
        "Cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "requirements.txt",
        "go.sum",
        "go.mod",
        "pipfile.lock",
        "poetry.lock",
        "gemfile.lock",
    ] {
        let candidate = dir.join(name);
        if candidate.exists() {
            files.push(candidate);
        }
    }

    files
}

// ── Package extraction ────────────────────────────────────────────────────

/// Extract packages from a lock/manifest file based on its name.
fn extract_packages(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    match file_name {
        "Cargo.lock" => extract_cargo_lock(path),
        "package-lock.json" => extract_npm_lock(path),
        "yarn.lock" => extract_yarn_lock(path),
        "requirements.txt" => extract_requirements_txt(path),
        "go.sum" | "go.mod" => extract_go_deps(path),
        "pipfile.lock" => extract_pipfile_lock(path),
        "poetry.lock" => extract_poetry_lock(path),
        "gemfile.lock" => extract_gemfile_lock(path),
        _ => Err(anyhow::anyhow!("unsupported lock file: {}", file_name)),
    }
}

fn extract_cargo_lock(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .context("failed to read Cargo.lock")?;
    let parsed: Value = toml::from_str(&content).context("failed to parse Cargo.lock as TOML")?;

    let packages = parsed["package"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let name = p["name"].as_str()?.to_string();
                    let version = p["version"].as_str().map(|v| v.to_string());
                    Some(OsvPackage {
                        name,
                        ecosystem: "crates.io".to_string(),
                        version,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(packages)
}

fn extract_npm_lock(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .context("failed to read package-lock.json")?;
    let parsed: Value =
        serde_json::from_str(&content).context("failed to parse package-lock.json")?;

    let packages = parsed["dependencies"]
        .as_object()
        .map(|deps| {
            deps.iter()
                .map(|(name, info)| {
                    let version = info["version"].as_str().map(|v| v.to_string());
                    OsvPackage {
                        name: name.clone(),
                        ecosystem: "npm".to_string(),
                        version,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(packages)
}

fn extract_yarn_lock(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .context("failed to read yarn.lock")?;
    // Parse simple yarn.lock format: lines starting with `"` are package specifiers.
    let mut packages = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('"') && trimmed.contains('@') {
            // e.g. "package@^1.0.0":
            let inner = trimmed.trim_matches('"');
            if let Some(at_pos) = inner.rfind('@') {
                let name = inner[..at_pos].to_string();
                let version = inner[at_pos + 1..]
                    .trim_start_matches('^')
                    .trim_start_matches('~')
                    .to_string();
                packages.push(OsvPackage {
                    name,
                    ecosystem: "npm".to_string(),
                    version: Some(version),
                });
            }
        }
    }
    Ok(packages)
}

fn extract_requirements_txt(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .context("failed to read requirements.txt")?;
    let mut packages = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        // Split on ==, >=, <=, ~= etc.
        let parts: Vec<&str> = trimmed
            .split(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' && c != '_')
            .collect();
        let name = parts.first().unwrap_or(&trimmed).to_string();
        // Try to extract version after == or similar.
        let version = trimmed
            .find("==")
            .map(|eq_pos| trimmed[eq_pos + 2..].trim().to_string());
        packages.push(OsvPackage {
            name,
            ecosystem: "PyPI".to_string(),
            version,
        });
    }
    Ok(packages)
}

fn extract_go_deps(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .context("failed to read go.sum/go.mod")?;
    let mut packages = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("go ") || trimmed.starts_with("require") {
            continue;
        }
        // Format: module/path v1.2.3 h1:hash...
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[0].to_string();
            let version = parts[1].to_string();
            packages.push(OsvPackage {
                name,
                ecosystem: "Go".to_string(),
                version: Some(version),
            });
        }
    }
    Ok(packages)
}

fn extract_pipfile_lock(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .context("failed to read Pipfile.lock")?;
    let parsed: Value = serde_json::from_str(&content).context("failed to parse Pipfile.lock")?;

    let mut packages = Vec::new();
    if let Some(default) = parsed["default"].as_object() {
        for (name, info) in default {
            let version = info["version"].as_str().map(|v| v.to_string());
            packages.push(OsvPackage {
                name: name.clone(),
                ecosystem: "PyPI".to_string(),
                version,
            });
        }
    }
    if let Some(dev) = parsed["develop"].as_object() {
        for (name, info) in dev {
            let version = info["version"].as_str().map(|v| v.to_string());
            packages.push(OsvPackage {
                name: name.clone(),
                ecosystem: "PyPI".to_string(),
                version,
            });
        }
    }
    Ok(packages)
}

fn extract_poetry_lock(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    // Poetry uses TOML-based poetry.lock similar to Cargo.lock.
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .context("failed to read poetry.lock")?;
    let parsed: Value = toml::from_str(&content).context("failed to parse poetry.lock as TOML")?;

    let packages = parsed["package"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let name = p["name"].as_str()?.to_string();
                    let version = p["version"].as_str().map(|v| v.to_string());
                    Some(OsvPackage {
                        name,
                        ecosystem: "PyPI".to_string(),
                        version,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(packages)
}

fn extract_gemfile_lock(path: &std::path::Path) -> Result<Vec<OsvPackage>> {
    let content = crate::orchestration::tool::exec_common::read_text_capped(
        path,
        crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
    )
    .context("failed to read Gemfile.lock")?;
    let mut packages = Vec::new();
    let mut in_specs = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("GEM") || trimmed.starts_with("specs:") {
            in_specs = true;
            continue;
        }
        if in_specs {
            if trimmed.is_empty()
                || trimmed.starts_with("PLATFORMS")
                || trimmed.starts_with("DEPENDENCIES")
            {
                break;
            }
            // Format: "  rack (2.2.3)"
            if trimmed.starts_with(' ') && !trimmed.starts_with("    ") {
                let cleaned = trimmed.trim_start();
                if let Some(paren_pos) = cleaned.find('(') {
                    let name = cleaned[..paren_pos].trim().to_string();
                    let version = cleaned[paren_pos + 1..]
                        .trim_end_matches(')')
                        .trim()
                        .to_string();
                    packages.push(OsvPackage {
                        name,
                        ecosystem: "RubyGems".to_string(),
                        version: Some(version),
                    });
                }
            }
        }
    }
    Ok(packages)
}

// ── OSV API query ─────────────────────────────────────────────────────────

/// Query the OSV API for vulnerabilities affecting a given package.
fn query_osv(pkg: &OsvPackage) -> Result<Vec<Value>> {
    // Reuse the process-global blocking client (connection pooling) instead of
    // building a fresh reqwest client per package; the 15s budget is applied
    // per request so a slow OSV response does not stall the whole scan.
    let client = crate::shared::http_client::blocking_http_client()
        .map_err(|err| anyhow::anyhow!("failed to get shared HTTP client: {err}"))?;

    let mut body = json!({
        "package": {
            "name": pkg.name,
            "ecosystem": pkg.ecosystem,
        }
    });

    if let Some(version) = &pkg.version {
        body["version"] = json!(version);
    }

    let mut resp = client
        .post("https://api.osv.dev/v1/query")
        .timeout(std::time::Duration::from_secs(15))
        .json(&body)
        .send()
        .with_context(|| format!("OSV API request failed for {}", pkg.name))?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!(
            "OSV API returned HTTP {} for {}",
            resp.status(),
            pkg.name
        ));
    }

    // Capped body read: OSV returns full advisory lists for popular packages;
    // `resp.json()` would buffer the whole body unboundedly.
    let body = crate::orchestration::tool::extended::http::read_blocking_body_capped(
        &mut resp,
        "osv.dev API",
    )
    .context("failed to read OSV API response")?;
    let data: Value = serde_json::from_slice(&body).context("failed to parse OSV API response")?;

    let vulns = data["vulns"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|vuln| {
                    json!({
                        "package": pkg.name,
                        "ecosystem": pkg.ecosystem,
                        "version": pkg.version,
                        "id": vuln["id"].as_str().unwrap_or("unknown"),
                        "summary": vuln["summary"].as_str().unwrap_or(""),
                        "aliases": vuln["aliases"].as_array().map(|a| {
                            a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>()
                        }).unwrap_or_default(),
                        "modified": vuln["modified"].as_str().unwrap_or(""),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(vulns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-sec".to_string(),
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
    fn security_scan_finds_no_lock_files_in_empty_dir() {
        let tmp = TempDir::new().expect("temp dir");
        let input = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
        }));
        let tool = SecurityScanTool;
        let output = tool.run(&input).expect("security_scan should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert!(
            !result["scanned"].as_bool().unwrap(),
            "expected scanned = false for empty project"
        );
    }

    #[test]
    fn security_scan_parses_cargo_lock() {
        let tmp = TempDir::new().expect("temp dir");
        let lock_content = r#"
[[package]]
name = "serde"
version = "1.0.0"

[[package]]
name = "tokio"
version = "1.35.0"
"#;
        std::fs::write(tmp.path().join("Cargo.lock"), lock_content).unwrap();

        let input = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
        }));
        let tool = SecurityScanTool;
        let output = tool.run(&input).expect("security_scan should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert_eq!(result["packages_found"].as_u64().unwrap(), 2);
        assert!(!result["lock_files"].as_array().unwrap().is_empty());
    }

    #[test]
    fn security_scan_parses_npm_lock() {
        let tmp = TempDir::new().expect("temp dir");
        let lock_content = r#"{
            "name": "test",
            "dependencies": {
                "lodash": {"version": "4.17.21"},
                "express": {"version": "4.18.0"}
            }
        }"#;
        std::fs::write(tmp.path().join("package-lock.json"), lock_content).unwrap();

        let input = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
        }));
        let tool = SecurityScanTool;
        let output = tool.run(&input).expect("security_scan should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert_eq!(result["packages_found"].as_u64().unwrap(), 2);
    }
}
