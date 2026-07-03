//! Skills Folder — Index skills from URLs listed in a `skills/` folder.
//!
//! Place a `.txt` or `.list` file in a `skills/` directory (same folder as config)
//! with one URL per line. Each URL is stored for future use.
//!
//! Example `skills/sources.txt`:
//! ```text
//! https://example.com/api/skills
//! https://raw.githubusercontent.com/user/repo/main/skills.json
//! ```
//!
//! Results are searchable via `skill-finder`.

/// Index of skills sources from URL files in the `skills/` folder.
///
/// Scans a config-local `skills/` directory for `.txt` / `.list` files
/// containing one skill-source URL per line. Lines starting with `#` or
/// `//` are treated as comments.
///
/// NOTE: This module is test-only scaffolding designed for indexing skill
/// source URLs. Production skill discovery is handled by `skill_discovery`
/// and `skill_import`. The module is gated behind `#[cfg(test)]` because it
/// has no production callers today — it is built and tested for integration
/// test coverage.
#[cfg(test)]
pub mod folder_index {
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use tracing::{debug, warn};

    /// Name of the skills folder.
    const SKILLS_DIR: &str = "skills";

    /// Index of skill-source URLs discovered in the `skills/` folder.
    ///
    /// Scans `.txt` and `.list` files (by extension) for URL lines.
    /// Comments (`#` or `//`) and blank lines are ignored.
    pub struct SkillsFolderIndex {
        /// Known source URLs.
        sources: HashSet<String>,
        /// Directory path.
        skills_dir: PathBuf,
    }

    impl SkillsFolderIndex {
        /// Create a new index targeting `{config_dir}/skills/`.
        pub fn new(config_dir: Option<&Path>) -> Self {
            let skills_dir = config_dir
                .map(|d| d.join(SKILLS_DIR))
                .unwrap_or_else(|| PathBuf::from(SKILLS_DIR));

            let mut idx = Self {
                sources: HashSet::new(),
                skills_dir,
            };
            idx.scan_folder();
            idx
        }

        /// Scan the folder for files containing URLs.
        pub fn scan_folder(&mut self) {
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

            let mut found_urls: HashSet<String> = HashSet::new();

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
                        found_urls.insert(line.to_string());
                    }
                }
            }

            self.sources = found_urls;
            debug!("skills folder scan: {} sources", self.sources.len(),);
        }

        /// The number of source URLs discovered.
        pub fn source_count(&self) -> usize {
            self.sources.len()
        }

        /// Returns `true` if no sources are indexed.
        pub fn is_empty(&self) -> bool {
            self.sources.is_empty()
        }
    }
} // mod folder_index

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::folder_index::*;
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
        assert_eq!(index.source_count(), 2, "Expected 2 sources in fresh index");
    }

    #[test]
    fn test_index_not_empty_when_skills_exist() {
        let dir = create_skills_dir();
        let index = SkillsFolderIndex::new(Some(dir.path()));
        assert!(!index.is_empty());
    }

    #[test]
    fn test_index_finds_two_sources() {
        let dir = create_skills_dir();
        let index = SkillsFolderIndex::new(Some(dir.path()));
        assert_eq!(index.source_count(), 2);
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
            index.source_count(),
            1,
            "Expected 1 source (URL after comments)"
        );
    }
}
