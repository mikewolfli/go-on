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
    extract_intent_tokens, name_similarity, normalize_name, semantic_similarity, PromptBasedSkill,
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

pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub score: f64,
    pub total_calls: u64,
    pub success_calls: u64,
    pub failure_calls: u64,
    pub average_latency_ms: f64,
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
    /// Register a skill, returning an error if the name is invalid or already registered.
    ///
    /// Name rules:
    /// - 1–64 characters
    /// - Only ASCII: lowercase letters, digits, `.`, `_`, `-`
    /// - Must be unique (duplicates are rejected)
    pub fn register(&mut self, skill: Arc<dyn super::Skill>) -> Result<()> {
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
        self.stats.entry(name).or_default();
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn super::Skill>> {
        self.skills.get(name).cloned()
    }

    pub fn unregister(&mut self, name: &str) -> bool {
        let removed = self.skills.remove(name).is_some();
        if removed {
            self.stats.remove(name);
            self.evolution_history.remove(name); // Clean up history too
            self.prompt_skill_data.remove(name);
        }
        removed
    }

    /// List all skill descriptors sorted by score (comprehensive output).
    pub fn list(&self) -> Vec<SkillDescriptor> {
        let mut items = self
            .skills
            .iter()
            .map(|(name, skill)| {
                let stats = self.stats.get(name).cloned().unwrap_or_default();
                SkillDescriptor {
                    name: skill.name().to_string(),
                    description: skill.description().to_string(),
                    input_schema: skill.input_schema(),
                    score: stats.score(),
                    total_calls: stats.total_calls,
                    success_calls: stats.success_calls,
                    failure_calls: stats.failure_calls,
                    average_latency_ms: stats.average_latency_ms(),
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
            score: stats.score(),
            total_calls: stats.total_calls,
            success_calls: stats.success_calls,
            failure_calls: stats.failure_calls,
            average_latency_ms: stats.average_latency_ms(),
        })
    }

    pub fn best_match(&self, requested: &str) -> Option<String> {
        self.best_match_with_input(requested, &Value::Null)
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

    /// Get evolution history for a skill.
    pub fn skill_evolution(&self, name: &str) -> Vec<SkillVersionRecord> {
        self.evolution_history
            .get(name)
            .cloned()
            .unwrap_or_default()
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

    /// Remove a prompt-based skill from the registry and persist the change.
    pub fn remove_prompt_skill(&mut self, name: &str) -> Result<()> {
        self.skills.remove(name);
        self.stats.remove(name);
        self.evolution_history.remove(name);
        self.prompt_skill_data.remove(name);
        self.save_prompt_skills_to_disk()
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
        let skill = PromptBasedSkill {
            name: pf.manifest.name.clone(),
            description: pf.manifest.description.clone(),
            prompt_template: prompt_text,
            input_schema: parsed_schema.clone(),
            timeout_secs: 30,
            max_retries: 2,
        };

        self.register(Arc::new(skill))?;

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

        let mut registered = 0usize;
        let mut skipped = 0usize;
        let mut errors: Vec<String> = Vec::new();

        let read_dir = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                anyhow::bail!(
                    "failed to read agent skills directory '{}': {}",
                    dir.display(),
                    e
                );
            }
        };

        for entry in read_dir.flatten() {
            let path = entry.path();
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
                skipped += 1;
                continue;
            };

            // ── Hot-reload: skip if the file hasn't been modified ──
            let current_mtime = match fs::metadata(&md_path).and_then(|meta| meta.modified()) {
                Ok(mtime) => mtime,
                Err(e) => {
                    warn!(
                        "Failed to read metadata for {}: {} — will re-parse",
                        md_path.display(),
                        e
                    );
                    // Proceed with parsing anyway; don't skip on metadata error
                    // Use UNIX_EPOCH so the file is always re-processed.
                    SystemTime::UNIX_EPOCH
                }
            };

            // If we already know this exact path and its mtime hasn't changed,
            // skip the expensive re-parse and re-registration entirely.
            if let Some(prev_mtime) = self.skill_file_mtimes.get(&md_path) {
                if *prev_mtime == current_mtime {
                    skipped += 1;
                    continue;
                }
            }

            let content = match fs::read(&md_path) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to read {}: {}", md_path.display(), e);
                    errors.push(format!("{}: read error: {}", md_path.display(), e));
                    skipped += 1;
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
                    errors.push(err_msg);
                    skipped += 1;
                    continue;
                }
            };

            let pf = ParsedSkillFile {
                md_path,
                current_mtime,
                manifest,
            };

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
    pub fn known_skill_names(&self) -> HashSet<String> {
        self.skills.keys().cloned().collect()
    }

    /// Returns a map of prompt-based skill data (skills created via
    /// `create_skill_from_prompt`). Used to merge GUI-created skills
    /// into the imported skill list.
    pub fn prompt_skill_data(&self) -> HashMap<String, SavedPromptSkill> {
        self.prompt_skill_data.clone()
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

    for entry in read_dir.flatten() {
        let path = entry.path();
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
            skipped += 1;
            continue;
        };

        let current_mtime = match fs::metadata(&md_path).and_then(|meta| meta.modified()) {
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
                skipped += 1;
                continue;
            }
        }

        let content = match fs::read(&md_path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {}: {}", md_path.display(), e);
                errors.push(format!("{}: read error: {}", md_path.display(), e));
                skipped += 1;
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
                errors.push(err_msg);
                skipped += 1;
                continue;
            }
        };

        parsed.push(ParsedSkillFile {
            md_path,
            current_mtime,
            manifest,
        });
    }

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
