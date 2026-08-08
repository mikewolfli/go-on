use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::interval;
use tracing::{info, warn};

use crate::i18n::runtime::tf;
use crate::orchestration::skill_import::{parse_skill_md, SkillImportManifest};

use super::execution::{
    extract_intent_tokens, name_similarity, normalize_name, semantic_similarity,
    tokenize_with_stopwords, PromptBasedSkill,
};

/// Convert a JSON schema Value (e.g. `{"type":"object","properties":{"code":{"type":"string"}}}`)
/// into a flat `HashMap<String, String>` suitable for `PromptBasedSkill::input_schema`.
fn schema_value_to_map(schema: &serde_json::Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (key, val) in props {
            let type_str = val.get("type").and_then(|v| v.as_str()).unwrap_or("string");
            let desc = val
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if desc.is_empty() {
                map.insert(key.clone(), type_str.to_string());
            } else {
                map.insert(key.clone(), format!("{} — {}", type_str, desc));
            }
        }
    }
    map
}

/// Records a version change in a skill's evolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillVersionRecord {
    pub skill_name: String,
    pub version: u32,
    pub change_description: String,
    pub score_at_version: f64,
    pub timestamp_ms: u64,
}

/// A serializable snapshot of a prompt-based skill for disk persistence.
///
/// This record is written to a JSON file whenever a prompt-based skill
/// is created or removed, so that skills survive a backend restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPromptSkill {
    pub name: String,
    pub description: String,
    pub prompt_template: String,
    pub input_schema: HashMap<String, String>,
    pub created_at: i64,
}

#[derive(Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Arc<dyn super::Skill>>,
    stats: HashMap<String, SkillRuntimeStats>,
    /// Skill evolution history keyed by skill name
    pub evolution_history: HashMap<String, Vec<SkillVersionRecord>>,
    /// Optional path to persist prompt-based skills to disk.
    /// When set, skills created via `create_skill_from_prompt` are saved
    /// automatically and reloaded at startup.
    persistence_path: Option<PathBuf>,
    /// Original data for prompt-based skills, keyed by name.
    /// Used to serialize skills back to disk without downcasting.
    prompt_skill_data: HashMap<String, SavedPromptSkill>,
    /// Tracks file modification timestamps for local SKILL.md files.
    /// Used by hot-reload to skip re-parsing unchanged files.
    skill_file_mtimes: HashMap<PathBuf, SystemTime>,
    /// Provenance records keyed by skill name.
    /// Populated by `register_with_provenance` and `discover_and_register_local_skills`.
    provenances: HashMap<String, SkillProvenance>,
    /// Namespace records keyed by skill name (e.g., "community", "builtin", "custom").
    /// Set during registration; exposed via `list()` and `descriptor()`.
    namespaces: HashMap<String, String>,
    /// Set of skill names that are hidden from model-facing discovery.
    /// This is a post-registration override for skills whose trait-level
    /// `disable_model_invocation()` returns `false` but should still be
    /// hidden (e.g., utility skills registered as `Arc<dyn Skill>`).
    hidden_skills: HashSet<String>,
}

impl std::fmt::Debug for SkillRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillRegistry")
            .field("skills", &self.skills.keys().collect::<Vec<_>>())
            .field("stats", &self.stats)
            .field("evolution_history", &self.evolution_history)
            .field("persistence_path", &self.persistence_path)
            .field("prompt_skill_count", &self.prompt_skill_data.len())
            .finish()
    }
}

/// Tracking provenance for a skill — where it was installed from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillProvenance {
    /// URI or file path the skill was installed from.
    pub source: String,
    /// Content digest (SHA-256 of SKILL.md contents) for integrity verification.
    pub content_digest: Option<String>,
    /// Timestamp (Unix ms) when the skill was installed.
    pub installed_at_ms: u64,
}

pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// Optional namespace for grouping (e.g., "community", "builtin", "custom").
    pub namespace: Option<String>,
    pub score: f64,
    pub total_calls: u64,
    pub success_calls: u64,
    pub failure_calls: u64,
    pub average_latency_ms: f64,
    /// Provenance tracking — where this skill was installed from.
    pub provenance: Option<SkillProvenance>,
    /// Whether this skill is hidden from model-facing discovery.
    pub hidden: bool,
    /// Optional policy for implicit invocation and product gating.
    pub policy: Option<crate::orchestration::skill::execution::SkillPolicy>,
}

#[derive(Clone, Debug, Default)]
struct SkillRuntimeStats {
    total_calls: u64,
    success_calls: u64,
    failure_calls: u64,
    total_latency_ms: u64,
}

impl SkillRuntimeStats {
    fn record(&mut self, success: bool, latency: Duration) {
        self.total_calls += 1;
        if success {
            self.success_calls += 1;
        } else {
            self.failure_calls += 1;
        }
        self.total_latency_ms = self
            .total_latency_ms
            .saturating_add(latency.as_millis() as u64);
    }

    fn average_latency_ms(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.total_latency_ms as f64 / self.total_calls as f64
        }
    }

    fn score(&self) -> f64 {
        if self.total_calls == 0 {
            return 0.5;
        }
        let success_rate = self.success_calls as f64 / self.total_calls as f64;
        let latency_penalty = (self.average_latency_ms() / 4000.0).clamp(0.0, 0.4);
        (success_rate - latency_penalty).clamp(0.0, 1.0)
    }
}

impl SkillRegistry {
    /// Register a skill with optional provenance tracking.
    ///
    /// If `provenance` is provided, the skill  record will carry
    /// the installation source and a content digest for integrity checks.
    pub fn register_with_provenance(
        &mut self,
        skill: Arc<dyn super::Skill>,
        provenance: Option<SkillProvenance>,
    ) -> Result<()> {
        self.validate_and_insert(skill, provenance)?;
        Ok(())
    }

    /// Register a skill, returning an error if the name is invalid or already registered.
    ///
    /// Name rules:
    /// - 1–64 characters
    /// - Only ASCII: lowercase letters, digits, `.`, `_`, `-`
    /// - Must be unique (duplicates are rejected)
    pub fn register(&mut self, skill: Arc<dyn super::Skill>) -> Result<()> {
        self.validate_and_insert(skill, None)
    }

    /// Register a skill that should be hidden from model discovery.
    /// Convenience wrapper around `register()` followed by `set_hidden()`.
    pub fn register_hidden(&mut self, skill: Arc<dyn super::Skill>) -> Result<()> {
        let name = skill.name().to_string();
        self.validate_and_insert(skill, None)?;
        self.set_hidden(&name, true);
        Ok(())
    }

    /// Set the namespace for an already-registered skill.
    ///
    /// Namespace is display/filter metadata (e.g., "community", "builtin", "custom")
    /// that does not affect the registry lookup key. Has no effect if the skill
    /// name is not registered.
    pub fn set_namespace(&mut self, name: &str, namespace: String) {
        if self.skills.contains_key(name) {
            self.namespaces.insert(name.to_string(), namespace);
        }
    }

    /// Mark a skill as hidden from or visible to model-facing discovery.
    ///
    /// Hidden skills are excluded from `list()` (unless `include_hidden` is true)
    /// and the semantic skill index, but remain invocable via `get()`.
    /// This is a convenience wrapper over `Skill::disable_model_invocation`
    /// that allows changing the flag after registration.
    pub fn set_hidden(&mut self, name: &str, hidden: bool) {
        // The `Skill` trait exposes `disable_model_invocation()` as a read-only
        // method. For post-registration changes, we track hidden status in
        // this separate set. The listing filter checks both the trait method
        // and this set.
        //
        // This is primarily useful for wrapping skills that don't have the
        // field (e.g. `EchoSkill`, `SkillCreatorSkill`).
        if hidden {
            self.hidden_skills.insert(name.to_string());
        } else {
            self.hidden_skills.remove(name);
        }
    }

    /// Returns `true` if the skill is hidden from model-facing discovery.
    /// Checks both the `Skill` trait's `disable_model_invocation()` and the
    /// registry-level `hidden_skills` override set.
    pub fn is_hidden(&self, name: &str) -> bool {
        self.hidden_skills.contains(name)
            || self
                .skills
                .get(name)
                .map(|s| s.disable_model_invocation())
                .unwrap_or(false)
    }

    /// Register the set of built-in skills that ship with go-on.
    ///
    /// These skills are registered with `"builtin"` namespace and a provenance
    /// source of `"builtin://<name>"`.  Built-in registration is skipped for any
    /// skill whose name is already taken (e.g. by a locally discovered skill).
    pub fn register_builtin_skills(&mut self) -> Result<()> {
        let builtins: Vec<PromptBasedSkill> = vec![PromptBasedSkill {
            name: "create-skill".to_string(),
            description: "Creates a new reusable skill from a natural language description"
                .to_string(),
            prompt_template: [
                "You are a skill creation assistant.",
                "Given the user's description of a task they want to automate,",
                "generate a complete skill definition: name, description, and",
                "prompt_template that captures the steps needed to accomplish the task.",
                "",
                "User request: {description}",
            ]
            .join("\n"),
            input_schema: HashMap::from([("description".to_string(), "string".to_string())]),
            timeout_secs: 120,
            max_retries: 2,
            disable_model_invocation: false,
            policy: None,
        }];

        for skill in builtins {
            if self.skills.contains_key(&skill.name) {
                tracing::trace!(
                    "Built-in skill '{}' already registered — skipping",
                    skill.name
                );
                continue;
            }

            let name = skill.name.clone();
            self.register_with_provenance(
                Arc::new(skill),
                Some(SkillProvenance {
                    source: format!("builtin://{name}"),
                    content_digest: None,
                    installed_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                }),
            )?;
            self.namespaces.insert(name, "builtin".to_string());
        }

        Ok(())
    }

    fn validate_and_insert(
        &mut self,
        skill: Arc<dyn super::Skill>,
        provenance: Option<SkillProvenance>,
    ) -> Result<()> {
        let name = skill.name().to_string();
        if name.is_empty() || name.len() > 64 {
            anyhow::bail!(
                "{}",
                tf(
                    "error.skill_name_length",
                    &[("name", name.as_str()), ("len", &name.len().to_string())]
                )
            );
        }
        if !name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        }) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.skill_name_invalid_chars",
                    &[("name", name.as_str()), ("chars", "invalid characters")]
                )
            );
        }
        if self.skills.contains_key(&name) {
            anyhow::bail!(
                "{}",
                tf("error.skill_already_registered", &[("name", name.as_str())])
            );
        }
        match skill.input_schema() {
            serde_json::Value::Object(_) => {}
            other => anyhow::bail!(
                "{}",
                tf(
                    "error.skill_name_invalid_chars",
                    &[
                        ("name", &name),
                        (
                            "chars",
                            &format!("input_schema must be a JSON object, got: {}", other)
                        )
                    ]
                )
            ),
        }
        self.skills.insert(name.clone(), skill);
        self.stats.entry(name.clone()).or_default();
        // Store provenance if provided (also exposed via list() and descriptor())
        if let Some(p) = provenance {
            tracing::info!(
                skill = %name,
                source = %p.source,
                digest = ?p.content_digest,
                "skill registered with provenance"
            );
            self.provenances.insert(name.clone(), p);
        }
        Ok(())
    }

    /// Look up a skill by name, supporting `namespace:name` syntax.
    ///
    /// When `name` contains a `:`, it is split into namespace and skill name,
    /// and the lookup succeeds only if both match. Without `:`, performs an
    /// exact name lookup as before.
    pub fn get(&self, name: &str) -> Option<Arc<dyn super::Skill>> {
        if let Some((ns, skill_name)) = name.split_once(':') {
            // Namespace-qualified lookup: verify both namespace and name
            let skill = self.skills.get(skill_name)?;
            let registered_ns = self.namespaces.get(skill_name)?;
            if registered_ns == ns {
                Some(skill.clone())
            } else {
                None
            }
        } else {
            self.skills.get(name).cloned()
        }
    }

    /// Unregister a skill by name and persist the change if persistence is enabled.
    ///
    /// Supports `namespace:name` syntax for disambiguation.
    /// Returns `true` if the skill was found and removed, `false` if it did not exist.
    /// Persists prompt-skill data to disk after removal when a persistence path is set.
    pub fn unregister(&mut self, name: &str) -> bool {
        // Resolve the internal key — strip optional namespace prefix
        let internal_name = if let Some((_ns, skill_name)) = name.split_once(':') {
            // When namespace-qualified, verify the namespace matches
            let registered_ns = self.namespaces.get(skill_name);
            if registered_ns.map(|ns| ns.as_str()) != Some(_ns) {
                return false;
            }
            skill_name
        } else {
            name
        };
        let removed = self.skills.remove(internal_name).is_some();
        if removed {
            self.stats.remove(internal_name);
            self.evolution_history.remove(internal_name);
            self.prompt_skill_data.remove(internal_name);
            self.provenances.remove(internal_name);
            self.namespaces.remove(internal_name);
            // Persist the change if prompt skill persistence is enabled.
            if self.persistence_path.is_some() {
                let _ = self.save_prompt_skills_to_disk();
            }
        }
        removed
    }

    /// List skill descriptors sorted by score (comprehensive output).
    ///
    /// When `include_hidden` is `false` (the normal case for model-facing
    /// listings), skills with `disable_model_invocation()` returning `true`
    /// are excluded. Pass `true` to include all skills regardless.
    pub fn list(&self, include_hidden: bool) -> Vec<SkillDescriptor> {
        let mut items = self
            .skills
            .iter()
            .filter(|(name, skill)| {
                include_hidden
                    || (!skill.disable_model_invocation()
                        && !self.hidden_skills.contains(name.as_str()))
            })
            .map(|(name, skill)| {
                let stats = self.stats.get(name).cloned().unwrap_or_default();
                SkillDescriptor {
                    name: skill.name().to_string(),
                    description: skill.description().to_string(),
                    input_schema: skill.input_schema(),
                    namespace: self.namespaces.get(name).cloned(),
                    score: stats.score(),
                    total_calls: stats.total_calls,
                    success_calls: stats.success_calls,
                    failure_calls: stats.failure_calls,
                    average_latency_ms: stats.average_latency_ms(),
                    provenance: self.provenances.get(name).cloned(),
                    hidden: skill.disable_model_invocation()
                        || self.hidden_skills.contains(name.as_str()),
                    policy: skill.policy().cloned(),
                }
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        items
    }

    pub fn record_outcome(&mut self, name: &str, success: bool, latency: Duration) {
        let stats = self.stats.entry(name.to_string()).or_default();
        stats.record(success, latency);
    }

    /// Get the score (0.0–1.0) for a skill by name.
    pub fn score_of(&self, name: &str) -> Option<f64> {
        self.stats.get(name).map(SkillRuntimeStats::score)
    }

    /// Return a full `SkillDescriptor` for the named skill, combining the
    /// skill's metadata with its runtime statistics. Returns `None` if the
    /// skill is not registered.
    pub fn descriptor(&self, name: &str) -> Option<SkillDescriptor> {
        let skill = self.skills.get(name)?;
        let stats = self.stats.get(name).cloned().unwrap_or_default();
        Some(SkillDescriptor {
            name: skill.name().to_string(),
            description: skill.description().to_string(),
            input_schema: skill.input_schema(),
            namespace: self.namespaces.get(name).cloned(),
            score: stats.score(),
            total_calls: stats.total_calls,
            success_calls: stats.success_calls,
            failure_calls: stats.failure_calls,
            average_latency_ms: stats.average_latency_ms(),
            provenance: self.provenances.get(name).cloned(),
            hidden: skill.disable_model_invocation() || self.hidden_skills.contains(name),
            policy: skill.policy().cloned(),
        })
    }

    pub fn best_match_with_input(&self, requested: &str, input: &Value) -> Option<String> {
        let normalized_requested = normalize_name(requested);
        if normalized_requested.is_empty() {
            return None;
        }

        let intent_tokens = extract_intent_tokens(input);

        self.skills
            .values()
            .map(|skill| {
                let name = skill.name().to_string();
                let normalized_name = normalize_name(&name);
                let name_score = name_similarity(&normalized_requested, &normalized_name);
                let runtime_score = self.score_of(&name).unwrap_or(0.5);
                let semantic_score = semantic_similarity(&intent_tokens, skill);
                let composite = (0.35 * name_score + 0.25 * runtime_score + 0.40 * semantic_score)
                    .clamp(0.0, 1.0);
                (name.clone(), composite)
            })
            .filter(|(_, score)| *score >= 0.55)
            .max_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(name, _)| name)
    }

    /// Create a new skill from a prompt template.
    ///
    /// Generates a `PromptBasedSkill` that wraps the given prompt into a Skill trait.
    /// The skill is automatically persisted to disk if `persistence_path` is set.
    pub fn create_skill_from_prompt(
        &mut self,
        name: &str,
        description: &str,
        prompt_template: &str,
        input_schema: HashMap<String, String>,
    ) -> Result<()> {
        // Validate name uniqueness
        if self.skills.contains_key(name) {
            anyhow::bail!(
                "{}",
                tf("error.skill_already_registered", &[("name", name)])
            );
        }

        let skill = PromptBasedSkill {
            name: name.to_string(),
            description: description.to_string(),
            prompt_template: prompt_template.to_string(),
            input_schema: input_schema.clone(),
            timeout_secs: 120,
            max_retries: 2,
            disable_model_invocation: false,
            policy: None,
        };

        self.register(Arc::new(skill))?;

        // Store original data for disk persistence
        self.prompt_skill_data.insert(
            name.to_string(),
            SavedPromptSkill {
                name: name.to_string(),
                description: description.to_string(),
                prompt_template: prompt_template.to_string(),
                input_schema,
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            },
        );

        // Persist to disk if persistence path is configured
        if self.persistence_path.is_some() {
            if let Err(e) = self.save_prompt_skills_to_disk() {
                tracing::warn!("Failed to persist prompt skills: {}", e);
            }
        }

        // Record evolution
        self.evolution_history
            .entry(name.to_string())
            .or_default()
            .push(SkillVersionRecord {
                skill_name: name.to_string(),
                version: 1,
                change_description: "Created from prompt template".to_string(),
                score_at_version: 0.5,
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            });
        // Cap evolution history at 50 records per skill
        if let Some(history) = self.evolution_history.get_mut(name) {
            if history.len() > 50 {
                history.drain(0..history.len() - 50);
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Disk persistence for prompt-based skills
    // -----------------------------------------------------------------------

    /// Set the persistence path for prompt-based skills.
    /// When set, `create_skill_from_prompt` will automatically persist.
    pub fn set_persistence_path(&mut self, path: PathBuf) {
        self.persistence_path = Some(path);
    }

    /// Load saved prompt skills from disk and register them in the registry.
    ///
    /// This is intended to be called once at startup, after setting
    /// the persistence path. Skills that were previously saved to disk
    /// (e.g. from `create_skill_from_prompt`) are recreated and registered.
    pub fn load_prompt_skills_from_disk(&mut self) -> Result<()> {
        let Some(ref path) = self.persistence_path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let content =
            std::fs::read_to_string(path).with_context(|| "failed to read prompt skills file")?;
        let saved: Vec<SavedPromptSkill> =
            serde_json::from_str(&content).context("failed to parse prompt skills file")?;
        for entry in saved {
            let name = entry.name.clone();
            let ps = PromptBasedSkill {
                name: name.clone(),
                description: entry.description.clone(),
                prompt_template: entry.prompt_template.clone(),
                input_schema: entry.input_schema.clone(),
                timeout_secs: 120,
                max_retries: 2,
                disable_model_invocation: false,
                policy: None,
            };
            // Use register() for proper validation instead of raw insertion.
            self.register(Arc::new(ps))?;
            self.prompt_skill_data.insert(name, entry);
        }
        Ok(())
    }

    /// Save all prompt-based skills to disk.
    ///
    /// Only skills tracked in `prompt_skill_data` are persisted.
    /// This includes skills created via `create_skill_from_prompt`
    /// and SKILL.md imports.
    pub fn save_prompt_skills_to_disk(&self) -> Result<()> {
        let Some(ref path) = self.persistence_path else {
            return Ok(());
        };
        let saved: Vec<&SavedPromptSkill> = self.prompt_skill_data.values().collect();
        if saved.is_empty() {
            // If the file exists but there are no prompt skills, remove it
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let content =
            serde_json::to_string_pretty(&saved).context("failed to serialize prompt skills")?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Shared helper: register a single [`ParsedSkillFile`] as a `PromptBasedSkill`.
    ///
    /// Handles hot-reload (unregister old), registration, mtime tracking, and
    /// prompt skill data persistence. Used by both `discover_and_register_local_skills`
    /// and the background refresh task to eliminate DRY violation.
    fn register_parsed_skill(&mut self, pf: ParsedSkillFile) -> Result<()> {
        // Hot-reload: unregister if the skill name is already taken
        if self.known_skill_names().contains(&pf.manifest.name) {
            info!(
                "Hot-reloading skill '{}' from '{}'",
                pf.manifest.name,
                pf.md_path.display()
            );
            self.unregister(&pf.manifest.name);
        }

        let prompt_text = pf
            .manifest
            .prompt_template
            .clone()
            .unwrap_or_else(|| pf.manifest.description.clone());

        let parsed_schema = schema_value_to_map(&pf.manifest.input_schema);
        let policy = if !pf.manifest.allow_implicit_invocation {
            Some(crate::orchestration::skill::execution::SkillPolicy {
                allow_implicit_invocation: Some(false),
                products: Vec::new(),
            })
        } else {
            None
        };
        let skill = PromptBasedSkill {
            name: pf.manifest.name.clone(),
            description: pf.manifest.description.clone(),
            prompt_template: prompt_text,
            input_schema: parsed_schema.clone(),
            timeout_secs: 30,
            max_retries: 2,
            disable_model_invocation: pf.manifest.disable_model_invocation,
            policy,
        };

        // Compute content digest from the raw SKILL.md bytes for provenance tracking
        let content_digest = {
            let content = fs::read(&pf.md_path).unwrap_or_default();
            Some(crate::shared::sha256_hex(&content))
        };

        let provenance = Some(SkillProvenance {
            source: pf.md_path.to_string_lossy().to_string(),
            content_digest,
            installed_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });

        self.register_with_provenance(Arc::new(skill), provenance)?;

        // Set namespace from manifest (YAML frontmatter) or derive from parent directory
        let namespace = pf.manifest.namespace.clone().or_else(|| {
            pf.md_path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
        });
        if let Some(ref ns) = namespace {
            self.set_namespace(&pf.manifest.name, ns.clone());
        }

        // Record the mtime so subsequent refresh ticks skip this file
        self.skill_file_mtimes.insert(pf.md_path, pf.current_mtime);

        // Track the imported skill data for persistence
        self.prompt_skill_data.insert(
            pf.manifest.name.clone(),
            SavedPromptSkill {
                name: pf.manifest.name.clone(),
                description: pf.manifest.description.clone(),
                prompt_template: pf.manifest.prompt_template.unwrap_or_default(),
                input_schema: parsed_schema,
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            },
        );

        Ok(())
    }

    /// Discover and register local skills from `~/.agents/skills/` directory.
    ///
    /// Scans each subdirectory of the agents skills directory for a `SKILL.md`
    /// (or `agent.md` as fallback) file, parses it via `parse_skill_md`,
    /// and registers the resulting manifest as a `PromptBasedSkill` in the
    /// registry. Skills that are already registered are skipped.
    ///
    /// Returns a summary of how many skills were registered vs skipped.
    ///
    /// # Arguments
    ///
    /// * `agents_skills_dir` — Optional path override. Defaults to
    ///   `~/.agents/skills` when `None`.
    pub fn discover_and_register_local_skills(
        &mut self,
        agents_skills_dir: Option<&Path>,
    ) -> Result<LocalSkillDiscoverySummary> {
        let dir = agents_skills_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(default_agents_skills_dir);

        if !dir.exists() {
            warn!(
                "Skills directory '{}' does not exist — no local skills discovered. \
                 Create ~/.agents/skills/ with SKILL.md files to register agent skills.",
                dir.display()
            );
            return Ok(LocalSkillDiscoverySummary {
                registered: 0,
                skipped: 0,
                errors: Vec::new(),
            });
        }

        // Shared scan (I/O + parse) used by both cold-start discovery and the
        // background refresh task — the former inline loop was a verbatim copy.
        let (parsed_files, mut errors, mut skipped) =
            scan_skills_directory(&dir, &self.skill_file_mtimes);

        let mut registered = 0usize;
        for pf in parsed_files {
            let skill_name = pf.manifest.name.clone();
            match self.register_parsed_skill(pf) {
                Ok(()) => {
                    registered += 1;
                }
                Err(e) => {
                    warn!("Failed to register skill '{}': {}", skill_name, e);
                    errors.push(format!("{}: registration error: {}", skill_name, e));
                    skipped += 1;
                }
            }
        }

        Ok(LocalSkillDiscoverySummary {
            registered,
            skipped,
            errors,
        })
    }

    /// Returns the known file mtimes for hot-reload tracking.
    /// Used by the background refresh task to scan outside the write lock.
    pub fn known_skill_mtimes(&self) -> HashMap<PathBuf, SystemTime> {
        self.skill_file_mtimes.clone()
    }

    /// Returns the set of currently registered skill names.
    ///
    /// When a skill has a namespace set, the returned name is formatted
    /// as `<namespace>:<name>` to support namespace-qualified lookups.
    pub fn known_skill_names(&self) -> HashSet<String> {
        self.skills
            .keys()
            .map(|name| {
                if let Some(ns) = self.namespaces.get(name) {
                    format!("{}:{}", ns, name)
                } else {
                    name.clone()
                }
            })
            .collect()
    }

    /// Returns a map of prompt-based skill data (skills created via
    /// `create_skill_from_prompt`). Used to merge GUI-created skills
    /// into the imported skill list.
    pub fn prompt_skill_data(&self) -> HashMap<String, SavedPromptSkill> {
        self.prompt_skill_data.clone()
    }

    /// Discover skills matching `query` using token-based similarity scoring.
    ///
    /// Returns up to `top_k` scored results sorted by relevance (highest first).
    /// Excludes hidden skills. Uses the same weight configuration as the original
    /// `SkillDiscovery` for backward-compatible results.
    ///
    /// This consolidates the fuzzy-matching logic previously split between
    /// `SkillRegistry::best_match_with_input` and `skill_discovery::SkillDiscovery`.
    pub fn discover_skills(&self, query: &str, top_k: usize) -> Vec<super::SkillDescriptor> {
        const MIN_SCORE: f64 = 0.40;
        const W_NAME: f64 = 0.35;
        const W_DESC: f64 = 0.40;
        const W_RUNTIME: f64 = 0.25;

        let query_trimmed = query.trim();
        if query_trimmed.is_empty() {
            let mut all = self.list(false);
            all.sort_by(|a, b| a.name.cmp(&b.name));
            all.truncate(top_k);
            return all;
        }

        let query_tokens = tokenize(query_trimmed);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(super::SkillDescriptor, f64)> = self
            .list(false)
            .into_iter()
            .map(|desc| {
                let name_tokens = tokenize(&desc.name);
                let desc_tokens = tokenize(&desc.description);

                // Jaccard similarity for name
                let name_overlap = if name_tokens.is_empty() {
                    0.0
                } else {
                    let intersect = name_tokens.intersection(&query_tokens).count() as f64;
                    let union = name_tokens.union(&query_tokens).count() as f64;
                    if union > 0.0 {
                        intersect / union
                    } else {
                        0.0
                    }
                };

                // Jaccard similarity for description
                let desc_overlap = if desc_tokens.is_empty() {
                    0.0
                } else {
                    let intersect = desc_tokens.intersection(&query_tokens).count() as f64;
                    let union = desc_tokens.union(&query_tokens).count() as f64;
                    if union > 0.0 {
                        intersect / union
                    } else {
                        0.0
                    }
                };

                // Runtime score — only contributes when there's semantic overlap
                let has_semantic = name_overlap > 0.0 || desc_overlap > 0.0;
                let runtime = if has_semantic { desc.score } else { 0.0 };

                let composite = name_overlap * W_NAME + desc_overlap * W_DESC + runtime * W_RUNTIME;
                (desc, composite)
            })
            .filter(|(_, score)| *score >= MIN_SCORE)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored.into_iter().map(|(desc, _)| desc).collect()
    }
}

/// Returns the default path for Zed's agent skills directory (`~/.agents/skills`).
fn default_agents_skills_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".agents").join("skills")
    } else {
        PathBuf::from(".agents/skills")
    }
}

/// Summary of a local skill discovery operation.
#[derive(Debug, Clone, Default)]
pub struct LocalSkillDiscoverySummary {
    /// Number of skills successfully registered.
    pub registered: usize,
    /// Number of skills skipped (already registered, missing SKILL.md, or parse failure).
    pub skipped: usize,
    /// Any error messages encountered during discovery.
    pub errors: Vec<String>,
}

/// Internal parsed result from scanning a single SKILL.md/agent.md file.
struct ParsedSkillFile {
    md_path: PathBuf,
    current_mtime: SystemTime,
    manifest: SkillImportManifest,
}

/// Scan a directory and parse all SKILL.md/agent.md files that are new or
/// have changed (based on `known_mtimes`). Returns the parsed results along
/// with any error messages and the number of skipped (unchanged) files.
///
/// This function performs no registration — it is purely I/O + parsing,
/// designed to be called outside a registry write lock.
fn scan_skills_directory(
    dir: &Path,
    known_mtimes: &HashMap<PathBuf, SystemTime>,
) -> (Vec<ParsedSkillFile>, Vec<String>, usize) {
    let mut parsed: Vec<ParsedSkillFile> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut skipped = 0usize;

    if !dir.exists() {
        return (parsed, errors, 0);
    }

    let read_dir = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            errors.push(format!("failed to read '{}': {}", dir.display(), e));
            return (parsed, errors, 0);
        }
    };

    let entries: Vec<PathBuf> = read_dir.flatten().map(|e| e.path()).collect();
    if entries.is_empty() {
        return (parsed, errors, skipped);
    }

    // Parallel phase: file I/O + YAML parsing dominate discovery time for
    // large directories. Use a bounded worker pool (at most 8, never more
    // threads than entries) instead of one thread per entry to avoid spawn
    // overhead on the typical small directory. Registration stays serial and
    // happens in the caller (`register_parsed_skill` needs `&mut self`).
    let worker_count = entries.len().min(8);
    let chunk_size = entries.len().div_ceil(worker_count);

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for chunk in entries.chunks(chunk_size) {
            handles.push(scope.spawn(move || {
                let mut parsed_local: Vec<ParsedSkillFile> = Vec::new();
                let mut errors_local: Vec<String> = Vec::new();
                let mut skipped_local = 0usize;

                for path in chunk {
                    if !path.is_dir() {
                        continue;
                    }

                    let skill_md_path = path.join("SKILL.md");
                    let agent_md_path = path.join("agent.md");

                    let md_path = if skill_md_path.exists() {
                        skill_md_path
                    } else if agent_md_path.exists() {
                        agent_md_path
                    } else {
                        skipped_local += 1;
                        continue;
                    };

                    let current_mtime =
                        match fs::metadata(&md_path).and_then(|meta| meta.modified()) {
                            Ok(mtime) => mtime,
                            Err(e) => {
                                warn!(
                                    "Failed to read metadata for {}: {} — will re-parse",
                                    md_path.display(),
                                    e
                                );
                                SystemTime::UNIX_EPOCH
                            }
                        };

                    if let Some(prev_mtime) = known_mtimes.get(&md_path) {
                        if *prev_mtime == current_mtime {
                            skipped_local += 1;
                            continue;
                        }
                    }

                    let content = match fs::read(&md_path) {
                        Ok(c) => c,
                        Err(e) => {
                            warn!("Failed to read {}: {}", md_path.display(), e);
                            errors_local.push(format!("{}: read error: {}", md_path.display(), e));
                            skipped_local += 1;
                            continue;
                        }
                    };

                    let manifest = match parse_skill_md(&content) {
                        Ok(m) => m,
                        Err(e) => {
                            let err_msg = format!(
                                "{}: invalid SKILL.md frontmatter — {}. \
                                 Ensure the file starts with '---' followed by valid YAML \
                                 with 'name:' and 'description:' fields.",
                                md_path.display(),
                                e
                            );
                            warn!("{}", err_msg);
                            errors_local.push(err_msg);
                            skipped_local += 1;
                            continue;
                        }
                    };

                    parsed_local.push(ParsedSkillFile {
                        md_path,
                        current_mtime,
                        manifest,
                    });
                }

                (parsed_local, errors_local, skipped_local)
            }));
        }

        for handle in handles {
            let (parsed_local, errors_local, skipped_local) =
                handle.join().expect("skill scan worker panicked");
            parsed.extend(parsed_local);
            errors.extend(errors_local);
            skipped += skipped_local;
        }
    });

    (parsed, errors, skipped)
}

/// Spawn a background tokio task that periodically rescans `~/.agents/skills/`
/// for new SKILL.md files and registers them in the given registry.
///
/// The rescan interval is 60 seconds. This allows new agent skills to be picked
/// up without restarting the server. The task runs until the returned
/// `JoinHandle` is dropped or the tokio runtime shuts down.
///
/// # Errors
///
/// Errors during rescan are logged via `tracing::warn!` but do not terminate
/// the background task.
///
/// Returns a no-op handle if no Tokio runtime is active (e.g., during sync tests).
pub fn spawn_skill_refresh_task(
    registry: std::sync::Arc<std::sync::RwLock<SkillRegistry>>,
    agents_skills_dir: Option<std::path::PathBuf>,
) -> Option<tokio::task::JoinHandle<()>> {
    let dir = agents_skills_dir.unwrap_or_else(default_agents_skills_dir);
    // Check if a Tokio runtime is active before trying to spawn.
    // Without this check, calling `tokio::spawn` from a `#[test]` (non-async) context
    // panics with "there is no reactor running".
    if tokio::runtime::Handle::try_current().is_err() {
        warn!(
            "No Tokio runtime active — background skill refresh disabled for '{}'",
            dir.display()
        );
        return None;
    }
    info!(
        "Spawning background skill refresh task (scanning '{}' every 60s)",
        dir.display()
    );
    Some(tokio::spawn(async move {
        let mut ticker = interval(std::time::Duration::from_secs(60));
        // Skip the first tick — the initial scan already happens during bootstrap.
        // This avoids redundant work and duplicate log messages at startup.
        ticker.tick().await;
        loop {
            ticker.tick().await;

            // Phase 1: Read current known mtimes under a read lock, then drop it.
            let known_mtimes = match registry.read() {
                Ok(guard) => guard.known_skill_mtimes(),
                Err(e) => {
                    warn!(
                        "Background skill refresh: failed to read-lock registry: {}",
                        e
                    );
                    continue;
                }
            };

            // Phase 2: Scan the filesystem and parse changed files outside
            // any lock. This avoids holding the write lock during I/O.
            let (parsed_files, errors, _skipped) = scan_skills_directory(&dir, &known_mtimes);

            for err in &errors {
                warn!("Background skill refresh warning: {}", err);
            }

            if parsed_files.is_empty() {
                continue;
            }

            // Phase 3: Briefly acquire the write lock to apply changes.
            match registry.write() {
                Ok(mut guard) => {
                    let mut registered = 0usize;
                    for pf in parsed_files {
                        match guard.register_parsed_skill(pf) {
                            Ok(()) => registered += 1,
                            Err(e) => warn!("Failed to register skill: {}", e),
                        }
                    }
                    if registered > 0 {
                        info!(
                            "Background skill refresh: registered {} new skill(s) from {}",
                            registered,
                            dir.display()
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Background skill refresh: failed to write-lock registry: {}",
                        e
                    );
                }
            }
        }
    }))
}

/// Tokenize a string into lowercase word tokens, filtering short/common words.
/// Shared between `SkillRegistry::discover_skills` and legacy `SkillDiscovery`.
///
/// Delegates to `execution::tokenize_with_stopwords` with the common English
/// stop-word list; `execution::tokenize_text` is the no-stop-word variant, so
/// the two skill-module tokenizers share one rule (B3).
pub fn tokenize(text: &str) -> HashSet<String> {
    const STOP_WORDS: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "can", "shall",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through",
        "during", "before", "after", "above", "below", "between", "out", "off", "over", "under",
        "again", "further", "then", "once", "here", "there", "when", "where", "why", "how", "all",
        "each", "every", "both", "few", "more", "most", "other", "some", "such", "no", "nor",
        "not", "only", "own", "same", "so", "than", "too", "very", "just", "because", "but", "and",
        "or", "if", "while", "that", "this", "these", "those", "it", "its",
    ];
    let stop_words: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    tokenize_with_stopwords(text, 3, &stop_words)
        .into_iter()
        .collect()
}
