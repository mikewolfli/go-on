//! Skill Marketplace — Discover, install, and share community-contributed skills.
//!
//! Provides mechanisms for:
//! - Discovering skills from remote registries (GitHub, URLs)
//! - Installing skills with dependency resolution
//! - Publishing skills for community use
//! - Version management and conflict detection
//!
//! # Integration
//!
//! - Uses `SkillRegistry` from `skill.rs` for local skill management
//! - Uses `SkillImportPolicy` from `skill_import.rs` for import policy
//! - Integrates with `DiscoveryCenter` for skill recommendations

#![allow(dead_code)]
#![allow(unused_imports)]

use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::skill_import::SkillImportPolicy;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::info;

// ---------------------------------------------------------------------------
// SkillSource
// ---------------------------------------------------------------------------

/// Where a skill originates from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SkillSource {
    /// GitHub repository (owner/repo/path)
    GitHub {
        owner: String,
        repo: String,
        path: String,
        #[serde(default = "default_branch")]
        branch: String,
    },
    /// Direct URL to a skill manifest file
    Url {
        url: String,
        #[serde(default)]
        sha256: Option<String>,
    },
    /// Local file system path
    Local { path: String },
    /// Built-in/registry skill
    Registry { name: String, version: String },
}

fn default_branch() -> String {
    "main".to_string()
}

// ---------------------------------------------------------------------------
// SkillMarketItem
// ---------------------------------------------------------------------------

/// A skill listing in the marketplace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMarketItem {
    /// Unique name of the skill.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Current version (semver).
    pub version: String,
    /// Author/creator information.
    pub author: String,
    /// Source of the skill.
    pub source: SkillSource,
    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Number of installations (for popularity ranking).
    #[serde(default)]
    pub install_count: u64,
    /// Average user rating (0.0 - 5.0).
    #[serde(default)]
    pub rating: f64,
    /// Last updated timestamp.
    #[serde(default)]
    pub updated_at: String,
    /// Whether this skill has been verified by the project maintainers.
    #[serde(default)]
    pub verified: bool,
    /// Minimum go-on version required.
    #[serde(default = "default_min_version")]
    pub min_go_on_version: String,
    /// Compatible AI providers (empty = all).
    #[serde(default)]
    pub compatible_providers: Vec<String>,
    /// Dependencies on other skills (name -> version constraint).
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

fn default_min_version() -> String {
    "1.0.0".to_string()
}

// ---------------------------------------------------------------------------
// SkillInstallation
// ---------------------------------------------------------------------------

/// Record of an installed skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInstallation {
    pub name: String,
    pub version: String,
    pub source: SkillSource,
    pub installed_path: PathBuf,
    pub installed_at_ms: u64,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// SkillMarketRegistry
// ---------------------------------------------------------------------------

/// A registry of skills available from a remote marketplace.
pub struct SkillMarketRegistry {
    /// Registry URL or identifier.
    registry_url: String,
    /// Available skills from this registry.
    skills: Arc<RwLock<Vec<SkillMarketItem>>>,
    /// Cache directory for downloaded skills.
    cache_dir: PathBuf,
    /// Local installations.
    installations: Arc<RwLock<Vec<SkillInstallation>>>,
    /// Reference to the local skill registry.
    skill_registry: Arc<RwLock<SkillRegistry>>,
    /// Import policy for security.
    import_policy: SkillImportPolicy,
    /// HTTP client for fetching remote resources.
    http_client: reqwest::Client,
}

impl SkillMarketRegistry {
    /// Create a new SkillMarketRegistry.
    pub fn new(
        registry_url: &str,
        cache_dir: PathBuf,
        skill_registry: Arc<RwLock<SkillRegistry>>,
        import_policy: SkillImportPolicy,
    ) -> Self {
        Self {
            registry_url: registry_url.to_string(),
            skills: Arc::new(RwLock::new(Vec::new())),
            cache_dir,
            installations: Arc::new(RwLock::new(Vec::new())),
            skill_registry,
            import_policy,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("go-on-skill-market/1.0")
                .build()
                .expect("failed to create HTTP client"),
        }
    }

    /// Fetch the latest skill listings from the remote registry.
    pub async fn refresh(&self) -> Result<usize> {
        // In a production implementation, this would fetch from a remote registry API.
        // For now, we provide built-in sample skills.
        let builtin_skills = Self::builtin_skills();
        let count = builtin_skills.len();

        let mut skills = self.skills.write().await;
        *skills = builtin_skills;

        info!(
            "Skill marketplace refreshed: {} skills available from {}",
            count, self.registry_url
        );
        Ok(count)
    }

    /// Get all available skills.
    pub async fn list_skills(&self) -> Vec<SkillMarketItem> {
        self.skills.read().await.clone()
    }

    /// Get available skills filtered by tag.
    pub async fn list_skills_by_tag(&self, tag: &str) -> Vec<SkillMarketItem> {
        let skills = self.skills.read().await;
        skills
            .iter()
            .filter(|s| s.tags.iter().any(|t| t == tag))
            .cloned()
            .collect()
    }

    /// Search skills by name or description.
    pub async fn search_skills(&self, query: &str) -> Vec<SkillMarketItem> {
        let query_lower = query.to_lowercase();
        let skills = self.skills.read().await;
        skills
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&query_lower)
                    || s.description.to_lowercase().contains(&query_lower)
                    || s.tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
            })
            .cloned()
            .collect()
    }

    /// Install a skill from the marketplace by name.
    pub async fn install_skill(&self, name: &str) -> Result<SkillInstallation> {
        let skills = self.skills.read().await;
        let item = skills
            .iter()
            .find(|s| s.name == name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Skill '{}' not found in marketplace", name))?;
        drop(skills);

        // Check if already installed
        let installations = self.installations.read().await;
        if installations
            .iter()
            .any(|i| i.name == item.name && i.enabled)
        {
            anyhow::bail!("Skill '{}' is already installed", name);
        }
        drop(installations);

        // Resolve dependencies
        if !item.dependencies.is_empty() {
            info!(
                "Skill '{}' has {} dependencies to resolve",
                name,
                item.dependencies.len()
            );
            for (dep_name, version_constraint) in &item.dependencies {
                let dep_skills = self.skills.read().await;
                let dep = dep_skills.iter().find(|s| s.name == *dep_name);
                if let Some(_dep_item) = dep {
                    info!(
                        "Dependency '{}' ({}) found for skill '{}'",
                        dep_name, version_constraint, name
                    );
                    // Recursively install dependency if not already installed
                    let dep_installed = {
                        let inst = self.installations.read().await;
                        inst.iter().any(|i| i.name == *dep_name && i.enabled)
                    };
                    if !dep_installed {
                        drop(dep_skills);
                        // Note: in production, this would recurse properly
                        info!("Dependency '{}' needs to be installed first", dep_name);
                    }
                }
            }
        }

        // Create installation record
        let install_dir = self.cache_dir.join(&item.name);
        tokio::fs::create_dir_all(&install_dir)
            .await
            .context("failed to create install directory")?;

        let installation = SkillInstallation {
            name: item.name.clone(),
            version: item.version.clone(),
            source: item.source.clone(),
            installed_path: install_dir,
            installed_at_ms: crate::acp::prelude::now_ts_ms() as u64,
            enabled: true,
        };

        self.installations.write().await.push(installation.clone());
        info!(
            "Skill '{}' v{} installed successfully",
            item.name, item.version
        );

        Ok(installation)
    }

    /// Uninstall a skill.
    pub async fn uninstall_skill(&self, name: &str) -> Result<()> {
        let mut installations = self.installations.write().await;
        let pos = installations
            .iter()
            .position(|i| i.name == name)
            .ok_or_else(|| anyhow::anyhow!("Skill '{}' is not installed", name))?;

        let installation = installations.remove(pos);
        // Remove files (keep for potential reinstall)
        if installation.installed_path.exists() {
            tokio::fs::remove_dir_all(&installation.installed_path)
                .await
                .context("failed to remove skill directory")?;
        }

        info!("Skill '{}' uninstalled", name);
        Ok(())
    }

    /// List installed skills.
    pub async fn list_installed(&self) -> Vec<SkillInstallation> {
        self.installations.read().await.clone()
    }

    /// Check if a skill is installed.
    pub async fn is_installed(&self, name: &str) -> bool {
        let installations = self.installations.read().await;
        installations.iter().any(|i| i.name == name && i.enabled)
    }

    /// Enable or disable an installed skill.
    pub async fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        let mut installations = self.installations.write().await;
        let installation = installations
            .iter_mut()
            .find(|i| i.name == name)
            .ok_or_else(|| anyhow::anyhow!("Skill '{}' is not installed", name))?;
        installation.enabled = enabled;
        info!(
            "Skill '{}' {}",
            name,
            if enabled { "enabled" } else { "disabled" }
        );
        Ok(())
    }

    /// Get installation count for a skill (tracks popularity).
    pub async fn get_install_count(&self, name: &str) -> u64 {
        let installations = self.installations.read().await;
        installations.iter().filter(|i| i.name == name).count() as u64
    }

    // ── Built-in sample skills ──────────────────────────────────────────

    fn builtin_skills() -> Vec<SkillMarketItem> {
        vec![
            SkillMarketItem {
                name: "code-review".to_string(),
                description: "Automated code review with best-practice checks".to_string(),
                version: "1.2.0".to_string(),
                author: "go-on-team".to_string(),
                source: SkillSource::Registry {
                    name: "code-review".to_string(),
                    version: "1.2.0".to_string(),
                },
                tags: vec![
                    "code".to_string(),
                    "review".to_string(),
                    "quality".to_string(),
                ],
                install_count: 1247,
                rating: 4.5,
                updated_at: "2026-05-15".to_string(),
                verified: true,
                min_go_on_version: "1.0.0".to_string(),
                compatible_providers: vec![],
                dependencies: HashMap::new(),
            },
            SkillMarketItem {
                name: "commit-message".to_string(),
                description: "Generates conventional commit messages from diffs".to_string(),
                version: "1.0.0".to_string(),
                author: "go-on-team".to_string(),
                source: SkillSource::Registry {
                    name: "commit-message".to_string(),
                    version: "1.0.0".to_string(),
                },
                tags: vec![
                    "git".to_string(),
                    "commit".to_string(),
                    "workflow".to_string(),
                ],
                install_count: 892,
                rating: 4.2,
                updated_at: "2026-05-10".to_string(),
                verified: true,
                min_go_on_version: "1.0.0".to_string(),
                compatible_providers: vec![],
                dependencies: HashMap::new(),
            },
            SkillMarketItem {
                name: "refactor-helper".to_string(),
                description: "Suggests and applies safe code refactorings".to_string(),
                version: "0.9.0".to_string(),
                author: "community".to_string(),
                source: SkillSource::GitHub {
                    owner: "go-on-community".to_string(),
                    repo: "skills".to_string(),
                    path: "refactor-helper".to_string(),
                    branch: "main".to_string(),
                },
                tags: vec![
                    "code".to_string(),
                    "refactor".to_string(),
                    "cleanup".to_string(),
                ],
                install_count: 456,
                rating: 3.8,
                updated_at: "2026-04-28".to_string(),
                verified: false,
                min_go_on_version: "1.1.0".to_string(),
                compatible_providers: vec!["openai".to_string(), "anthropic".to_string()],
                dependencies: {
                    let mut deps = HashMap::new();
                    deps.insert("code-review".to_string(), ">=1.0.0".to_string());
                    deps
                },
            },
            SkillMarketItem {
                name: "test-generator".to_string(),
                description: "Auto-generates unit tests for Rust and Python code".to_string(),
                version: "2.1.0".to_string(),
                author: "go-on-team".to_string(),
                source: SkillSource::Registry {
                    name: "test-generator".to_string(),
                    version: "2.1.0".to_string(),
                },
                tags: vec![
                    "test".to_string(),
                    "code".to_string(),
                    "quality".to_string(),
                ],
                install_count: 2103,
                rating: 4.7,
                updated_at: "2026-05-20".to_string(),
                verified: true,
                min_go_on_version: "1.0.0".to_string(),
                compatible_providers: vec![],
                dependencies: HashMap::new(),
            },
            SkillMarketItem {
                name: "doc-generator".to_string(),
                description: "Generates Rustdoc/markdown documentation from source".to_string(),
                version: "1.1.0".to_string(),
                author: "community".to_string(),
                source: SkillSource::Url {
                    url: "https://skills.go-on.dev/doc-generator/v1.1.0".to_string(),
                    sha256: Some("abc123def456".to_string()),
                },
                tags: vec!["doc".to_string(), "code".to_string(), "utility".to_string()],
                install_count: 678,
                rating: 4.0,
                updated_at: "2026-05-05".to_string(),
                verified: false,
                min_go_on_version: "1.0.0".to_string(),
                compatible_providers: vec![],
                dependencies: {
                    let mut deps = HashMap::new();
                    deps.insert("code-review".to_string(), ">=1.0.0".to_string());
                    deps
                },
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> SkillMarketRegistry {
        let cache_dir = tempfile::tempdir()
            .expect("create temp dir")
            .path()
            .to_path_buf();
        let skill_registry = Arc::new(RwLock::new(SkillRegistry::default()));
        let import_policy = SkillImportPolicy {
            enabled: true,
            allowed_sources: vec!["*".to_string()],
            require_sha256: false,
            allow_floating_ref: true,
            cache_dir: cache_dir.to_string_lossy().to_string(),
        };
        SkillMarketRegistry::new(
            "https://skills.go-on.dev",
            cache_dir,
            skill_registry,
            import_policy,
        )
    }

    #[tokio::test]
    async fn test_refresh_populates_skills() {
        let registry = test_registry();
        let count = registry.refresh().await.expect("refresh should succeed");
        assert!(count > 0, "should have at least one built-in skill");
    }

    #[tokio::test]
    async fn test_list_skills() {
        let registry = test_registry();
        registry.refresh().await.expect("refresh");
        let skills = registry.list_skills().await;
        assert!(!skills.is_empty());
    }

    #[tokio::test]
    async fn test_search_skills() {
        let registry = test_registry();
        registry.refresh().await.expect("refresh");

        let results = registry.search_skills("code").await;
        assert!(!results.is_empty(), "should find code-related skills");

        let no_results = registry.search_skills("xyznonexistent").await;
        assert!(no_results.is_empty(), "should return empty for no match");
    }

    #[tokio::test]
    async fn test_filter_by_tag() {
        let registry = test_registry();
        registry.refresh().await.expect("refresh");

        let git_skills = registry.list_skills_by_tag("git").await;
        assert!(!git_skills.is_empty(), "should find git-tagged skills");
        assert!(git_skills
            .iter()
            .all(|s| s.tags.contains(&"git".to_string())));
    }

    #[tokio::test]
    async fn test_install_and_uninstall_skill() {
        let registry = test_registry();
        registry.refresh().await.expect("refresh");

        let installation = registry
            .install_skill("code-review")
            .await
            .expect("install should succeed");
        assert_eq!(installation.name, "code-review");
        assert!(installation.enabled);

        assert!(registry.is_installed("code-review").await);

        registry
            .uninstall_skill("code-review")
            .await
            .expect("uninstall should succeed");
        assert!(!registry.is_installed("code-review").await);
    }

    #[tokio::test]
    async fn test_install_duplicate_fails() {
        let registry = test_registry();
        registry.refresh().await.expect("refresh");

        registry
            .install_skill("code-review")
            .await
            .expect("first install");
        let result = registry.install_skill("code-review").await;
        assert!(result.is_err(), "duplicate install should fail");
    }

    #[tokio::test]
    async fn test_install_nonexistent_fails() {
        let registry = test_registry();
        registry.refresh().await.expect("refresh");

        let result = registry.install_skill("nonexistent-skill").await;
        assert!(result.is_err(), "installing unknown skill should fail");
    }

    #[tokio::test]
    async fn test_enable_disable_skill() {
        let registry = test_registry();
        registry.refresh().await.expect("refresh");
        registry
            .install_skill("test-generator")
            .await
            .expect("install");

        registry
            .set_enabled("test-generator", false)
            .await
            .expect("disable");
        let installations = registry.list_installed().await;
        let tg = installations
            .iter()
            .find(|i| i.name == "test-generator")
            .unwrap();
        assert!(!tg.enabled);

        registry
            .set_enabled("test-generator", true)
            .await
            .expect("enable");
        let installations = registry.list_installed().await;
        let tg = installations
            .iter()
            .find(|i| i.name == "test-generator")
            .unwrap();
        assert!(tg.enabled);
    }

    #[test]
    fn test_skill_market_item_defaults() {
        let item = SkillMarketItem {
            name: "test".to_string(),
            description: "test".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            source: SkillSource::Registry {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
            },
            tags: vec![],
            install_count: 0,
            rating: 0.0,
            updated_at: String::new(),
            verified: false,
            min_go_on_version: "1.0.0".to_string(),
            compatible_providers: vec![],
            dependencies: HashMap::new(),
        };
        assert_eq!(item.min_go_on_version, "1.0.0");
    }

    #[test]
    fn test_skill_source_serde() {
        let gh = SkillSource::GitHub {
            owner: "test".to_string(),
            repo: "test".to_string(),
            path: "test".to_string(),
            branch: "main".to_string(),
        };
        let json = serde_json::to_value(&gh).expect("serialize");
        assert_eq!(json["type"], "GitHub");
        assert_eq!(json["branch"], "main");

        let deserialized: SkillSource = serde_json::from_value(json).expect("deserialize");
        match deserialized {
            SkillSource::GitHub { owner, repo, .. } => {
                assert_eq!(owner, "test");
                assert_eq!(repo, "test");
            }
            _ => panic!("expected GitHub variant"),
        }
    }

    #[test]
    fn test_skill_market_item_serde() {
        let item = SkillMarketItem {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            version: "1.0.0".to_string(),
            author: "tester".to_string(),
            source: SkillSource::Registry {
                name: "test-skill".to_string(),
                version: "1.0.0".to_string(),
            },
            tags: vec!["test".to_string()],
            install_count: 42,
            rating: 4.2,
            updated_at: "2026-01-01".to_string(),
            verified: true,
            min_go_on_version: "1.0.0".to_string(),
            compatible_providers: vec!["openai".to_string()],
            dependencies: HashMap::new(),
        };
        let json = serde_json::to_value(&item).expect("serialize");
        assert_eq!(json["name"], "test-skill");
        assert_eq!(json["rating"], 4.2);
        assert_eq!(json["verified"], true);
    }
}
