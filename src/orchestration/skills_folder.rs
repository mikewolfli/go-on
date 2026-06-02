//! Skills Folder — Fetch and index skills from URLs listed in a `skills/` folder.
//!
//! Place a `.txt` or `.list` file in a `skills/` directory (same folder as config)
//! with one URL per line. Each URL is fetched and the returned JSON array of
//! skills is indexed for search.
//!
//! Example `skills/sources.txt`:
//! ```text
//! https://example.com/api/skills
//! https://raw.githubusercontent.com/user/repo/main/skills.json
//! ```
//!
//! Each URL should return a JSON array of skill objects:
//! ```json
//! [{"name":"my-skill","description":"Does something useful"}, ...]
//! ```
//!
//! All skills from all URLs are merged and searchable via `skill-finder`.
//! Results are cached for 5 minutes.
//!
//! Types consumed via global OnceLock static in tools_pack.rs.
// F-GAP-51: dead_code allowed on items below in non-test builds (consumed via OnceLock)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Name of the skills folder.
const SKILLS_DIR: &str = "skills";

/// How often to re-fetch remote URLs (seconds).
const FETCH_INTERVAL: Duration = Duration::from_secs(300);

/// How often to rescan the folder for new files (seconds).
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
const RESCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Timeout per URL fetch.
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// RemoteSkill
// ---------------------------------------------------------------------------

/// A single skill fetched from a remote URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-51 — consumed via OnceLock
pub struct RemoteSkill {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub categories: Vec<String>,
}

// ---------------------------------------------------------------------------
// CachedSource
// ---------------------------------------------------------------------------

/// A URL source with its fetched skills and cache timestamp.
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
struct CachedSource {
    /// Names of skills previously fetched from this URL.
    skill_names: Vec<String>,
    fetched_at: Instant,
}

// ---------------------------------------------------------------------------
// SkillsFolderIndex
// ---------------------------------------------------------------------------

/// Index of all skills fetched from URLs listed in the `skills/` folder.
///
/// Consumed via global OnceLock static in tools_pack.rs.
#[cfg_attr(not(test), allow(dead_code))] // F-GAP-51 — consumed via OnceLock
pub struct SkillsFolderIndex {
    skills: HashMap<String, RemoteSkill>,
    /// Known source URLs (keyed by URL string).
    #[allow(dead_code)]
    // F-GAP-49 — reserved for future use
    sources: HashMap<String, CachedSource>,
    /// Directory path.
    skills_dir: PathBuf,
    /// Last folder scan time.
    last_scan: Instant,
    /// Last fetch time.
    #[allow(dead_code)]
    // F-GAP-49 — reserved for future use
    last_fetch: Instant,
}

/// Async helper: fetch and parse skills from a URL.
/// Extracted so `fetch_url` can run it via either `Handle::block_on` or
/// a temporary `Runtime::block_on`, avoiding creating a new Runtime each time.
#[allow(dead_code)]
// F-GAP-49 — reserved for future use
async fn fetch_skills_from_url(url: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent("go-on-skills-folder/1.0")
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        // 404/410/Gone = URL confirmed dead -> delete source
        if status == 404 || status == 410 {
            return Err(format!("GONE:{}", status));
        }
        return Err(format!("HTTP {}", status));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(body)
}

#[cfg_attr(not(test), allow(dead_code))] // F-GAP-51 — consumed via OnceLock
impl SkillsFolderIndex {
    /// Create a new index targeting `{config_dir}/skills/`.
    pub fn new(config_dir: Option<&Path>) -> Self {
        let skills_dir = config_dir
            .map(|d| d.join(SKILLS_DIR))
            .unwrap_or_else(|| PathBuf::from(SKILLS_DIR));

        let mut idx = Self {
            skills: HashMap::new(),
            sources: HashMap::new(),
            skills_dir,
            last_scan: Instant::now(),
            last_fetch: Instant::now(),
        };
        idx.scan_folder();
        // fetch_all is not called here to avoid network dependencies;
        // callers should call refresh() explicitly when ready.
        idx
    }

    /// Refresh: rescan folder + re-fetch if stale.
    #[allow(dead_code)]
    // F-GAP-49 — reserved for future use
    pub fn refresh(&mut self) {
        if self.last_scan.elapsed() >= RESCAN_INTERVAL {
            self.scan_folder();
        }
        if self.last_fetch.elapsed() >= FETCH_INTERVAL {
            self.fetch_all();
        }
    }

    /// Scan the folder for files containing URLs.
    fn scan_folder(&mut self) {
        if !self.skills_dir.exists() {
            if let Err(e) = fs::create_dir_all(&self.skills_dir) {
                debug!("cannot create skills dir {:?}: {}", self.skills_dir, e);
                return;
            }
        }

        let read_dir = match fs::read_dir(&self.skills_dir) {
            Ok(r) => r,
            Err(e) => {
                debug!("cannot read skills dir {:?}: {}", self.skills_dir, e);
                return;
            }
        };

        let mut found_urls: Vec<String> = Vec::new();

        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    warn!("failed to read {:?}: {}", path, e);
                    continue;
                }
            };

            // Read each non-empty, non-comment line as a URL
            for line in content.lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') && !line.starts_with("//") {
                    found_urls.push(line.to_string());
                }
            }
        }

        let new_urls: std::collections::BTreeSet<String> = found_urls.into_iter().collect();
        let existing_urls: std::collections::BTreeSet<String> =
            self.sources.keys().cloned().collect();

        // Add new sources
        for url in &new_urls {
            if !existing_urls.contains(url) {
                debug!("new skill source registered: {}", url);
                self.sources.insert(
                    url.clone(),
                    CachedSource {
                        skill_names: Vec::new(),
                        fetched_at: Instant::now() - FETCH_INTERVAL - Duration::from_secs(1),
                    },
                );
            }
        }

        // Remove stale sources
        for url in existing_urls.difference(&new_urls) {
            debug!("skill source removed: {}", url);
            self.sources.remove(url);
        }

        self.last_scan = Instant::now();
        debug!(
            "skills folder scan: {} sources, {} indexed skills",
            self.sources.len(),
            self.skills.len()
        );
    }

    /// Fetch all URLs that haven't been fetched recently.
    #[allow(dead_code)]
    // F-GAP-49 — reserved for future use
    fn fetch_all(&mut self) {
        let now = Instant::now();
        let stale_urls: Vec<String> = self
            .sources
            .iter()
            .filter(|(_, cache)| now.duration_since(cache.fetched_at) >= FETCH_INTERVAL)
            .map(|(url, _)| url.clone())
            .collect();

        for url in stale_urls {
            self.fetch_url(&url);
        }

        self.last_fetch = Instant::now();
    }

    /// Fetch a single URL and parse skills.
    #[allow(dead_code)]
    // F-GAP-49 — reserved for future use
    fn fetch_url(&mut self, url: &str) {
        debug!("fetching skills from: {}", url);

        // Use the existing tokio runtime handle to avoid creating a new
        // blocking runtime on every fetch. Falls back to a new Runtime
        // only when no tokio runtime is present (e.g., in tests).
        let result = match tokio::runtime::Handle::try_current() {
            Ok(h) => tokio::task::block_in_place(move || {
                h.block_on(fetch_skills_from_url(url))
            }),
            Err(_) => {
                warn!(
                    "no tokio runtime found for fetching {}; creating temporary runtime",
                    url
                );
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("failed to create runtime for fetching {}: {}", url, e);
                        return;
                    }
                };
                rt.block_on(fetch_skills_from_url(url))
            }
        };

        match result {
            Ok(Value::Array(arr)) => {
                // Remove old skills from this source, add new ones
                if let Some(cache) = self.sources.get_mut(url) {
                    for name in cache.skill_names.drain(..) {
                        self.skills.remove(&name);
                    }
                }

                let mut count = 0;
                for item in arr {
                    if let Ok(skill) = serde_json::from_value::<RemoteSkill>(item) {
                        if !skill.name.is_empty() {
                            let skill_name = skill.name.clone();
                            self.skills.insert(skill_name.clone(), skill);
                            if let Some(cache) = self.sources.get_mut(url) {
                                cache.skill_names.push(skill_name);
                            }
                            count += 1;
                        }
                    }
                }
                if let Some(cache) = self.sources.get_mut(url) {
                    cache.fetched_at = Instant::now();
                }
                debug!("fetched {} skills from {}", count, url);
            }
            Ok(Value::Object(map)) => {
                let items = map
                    .get("skills")
                    .or_else(|| map.get("items"))
                    .or_else(|| map.get("data"))
                    .and_then(|v| v.as_array());
                if let Some(arr) = items {
                    // Remove old skills from this source, add new ones
                    if let Some(cache) = self.sources.get_mut(url) {
                        for name in cache.skill_names.drain(..) {
                            self.skills.remove(&name);
                        }
                    }

                    let mut count = 0;
                    for item in arr {
                        if let Ok(skill) = serde_json::from_value::<RemoteSkill>(item.clone()) {
                            if !skill.name.is_empty() {
                                let skill_name = skill.name.clone();
                                self.skills.insert(skill_name.clone(), skill);
                                if let Some(cache) = self.sources.get_mut(url) {
                                    cache.skill_names.push(skill_name);
                                }
                                count += 1;
                            }
                        }
                    }
                    if let Some(cache) = self.sources.get_mut(url) {
                        cache.fetched_at = Instant::now();
                    }
                    debug!("fetched {} skills from {} (wrapped)", count, url);
                } else {
                    warn!("unexpected response from {}: no array field", url);
                }
            }
            Ok(_) => warn!("unexpected response type from {}", url),
            Err(e) if e.starts_with("GONE:") => {
                // URL confirmed dead (404/410) → remove source + its skills
                warn!("removing dead skill source: {} ({})", url, e);
                if let Some(cache) = self.sources.remove(url) {
                    for name in cache.skill_names {
                        self.skills.remove(&name);
                    }
                }
            }
            Err(e) => {
                // Transient error (network, timeout, 5xx) → keep skills, retry later
                warn!("failed to fetch {} (will retry): {}", url, e);
            }
        }
    }

    /// Search across all fetched skills.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<ScoredRemoteSkill> {
        if query.is_empty() || self.skills.is_empty() {
            return Vec::new();
        }

        let q = query.to_ascii_lowercase();
        let q_words: Vec<&str> = q.split_whitespace().collect();

        let mut results: Vec<ScoredRemoteSkill> = self
            .skills
            .values()
            .map(|s| {
                let name_lower = s.name.to_ascii_lowercase();
                let desc_lower = s.description.to_ascii_lowercase();
                let score = if name_lower.contains(&q) || desc_lower.contains(&q) {
                    1.0
                } else {
                    let all = format!("{} {} {}", name_lower, desc_lower, s.categories.join(" "));
                    let matches = q_words.iter().filter(|w| all.contains(*w)).count();
                    if matches > 0 {
                        (matches as f64 / q_words.len() as f64).min(0.9)
                    } else {
                        0.0
                    }
                };
                ScoredRemoteSkill {
                    name: s.name.clone(),
                    description: s.description.clone(),
                    version: s.version.clone(),
                    author: s.author.clone(),
                    url: if s.url.is_empty() {
                        None
                    } else {
                        Some(s.url.clone())
                    },
                    categories: s.categories.clone(),
                    score: (score * 100.0).round() / 100.0,
                }
            })
            .filter(|s| s.score > 0.0)
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(top_k.min(50));
        results
    }

    /// Total indexed skills.
    #[allow(dead_code)]
    // F-GAP-49 — reserved for future use
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

/// A search result from the remote skills folder.
/// Consumed via global OnceLock static in tools_pack.rs.
#[derive(Debug, Clone, Serialize)]
pub struct ScoredRemoteSkill {
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub url: Option<String>,
    pub categories: Vec<String>,
    pub score: f64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_skills_dir() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).expect("create skills dir");
        fs::write(
            skills_dir.join("sources.txt"),
            "# Skill sources\nhttps://example.com/api/skills\nhttps://example2.com/skills\n",
        )
        .expect("write");
        dir
    }

    #[test]
    fn test_scan_folder_finds_urls() {
        let dir = create_skills_dir();
        let index = SkillsFolderIndex::new(Some(dir.path()));
        // scan_folder extracts both URLs from the file; fetch_all is no longer
        // called in new(), so both sources are present regardless of network.
        assert_eq!(index.sources.len(), 2, "Expected 2 sources in fresh index");
    }

    #[test]
    fn test_search_empty_when_no_skills() {
        let dir = create_skills_dir();
        let index = SkillsFolderIndex::new(Some(dir.path()));
        assert!(index.is_empty());
        assert!(index.search("test", 5).is_empty());
    }

    #[test]
    fn test_search_empty_query() {
        let dir = create_skills_dir();
        let index = SkillsFolderIndex::new(Some(dir.path()));
        assert!(index.search("", 5).is_empty());
    }

    #[test]
    fn test_no_skills_dir() {
        let dir = TempDir::new().expect("tempdir");
        let index = SkillsFolderIndex::new(Some(dir.path()));
        assert!(index.is_empty());
    }

    #[test]
    fn test_empty_file_ignored() {
        let dir = TempDir::new().expect("tempdir");
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).expect("create skills dir");
        fs::write(skills_dir.join("empty.txt"), "").expect("write");
        let index = SkillsFolderIndex::new(Some(dir.path()));
        assert!(index.is_empty());
    }

    #[test]
    fn test_comments_ignored() {
        let dir = TempDir::new().expect("tempdir");
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).expect("create skills dir");
        fs::write(
            skills_dir.join("sources.txt"),
            "# comment\n// also comment\n\nhttps://example.com/skills\n",
        )
        .expect("write");
        let index = SkillsFolderIndex::new(Some(dir.path()));
        assert_eq!(
            index.sources.len(),
            1,
            "Expected 1 source (URL after comments)"
        );
    }
}
