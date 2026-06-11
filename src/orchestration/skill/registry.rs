use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::i18n::runtime::tf;

use super::execution::{normalize_name, extract_intent_tokens, name_similarity, semantic_similarity, PromptBasedSkill};

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

    pub fn score_of(&self, name: &str) -> Option<f64> {
        self.stats.get(name).map(SkillRuntimeStats::score)
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

    /// Create a new skill by composing two existing skills.
    ///
    /// The composed skill first executes `skill_a`, then passes its output
    /// as input to `skill_b`.
    pub fn compose_skills(
        &mut self,
        name: &str,
        description: &str,
        skill_a: &str,
        skill_b: &str,
    ) -> Result<()> {
        if self.skills.contains_key(name) {
            anyhow::bail!(
                "{}",
                tf("error.skill_already_registered", &[("name", name)])
            );
        }
        if !self.skills.contains_key(skill_a) {
            anyhow::bail!("{}", tf("error.skill_not_found", &[("name", skill_a)]));
        }
        if !self.skills.contains_key(skill_b) {
            anyhow::bail!("{}", tf("error.skill_not_found", &[("name", skill_b)]));
        }

        // Create a shared registry for ComposedSkill to reference at execution time.
        // This allows the composed skill to look up its sub-skills dynamically.
        let composed_registry = Arc::new(Mutex::new(SkillRegistry::default()));
        // Clone the looked-up skills into the composed registry so they can be
        // resolved at execution time without holding the main registry lock.
        if let Some(sa) = self.skills.get(skill_a) {
            let _ = composed_registry.lock().map(|mut r| r.register(sa.clone()));
        }
        if let Some(sb) = self.skills.get(skill_b) {
            let _ = composed_registry.lock().map(|mut r| r.register(sb.clone()));
        }

        let skill = super::execution::ComposedSkill {
            name: name.to_string(),
            description: format!("{}: {} \u{2192} {}", description, skill_a, skill_b),
            skill_a: skill_a.to_string(),
            skill_b: skill_b.to_string(),
            registry: composed_registry,
        };

        self.register(Arc::new(skill))?;

        self.evolution_history
            .entry(name.to_string())
            .or_default()
            .push(SkillVersionRecord {
                skill_name: name.to_string(),
                version: 1,
                change_description: format!("Composed from '{}' and '{}'", skill_a, skill_b),
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

    /// List all known skill names (for discovery).
    pub fn list_skills(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
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
            self.prompt_skill_data.insert(name.clone(), entry);
            self.skills.insert(name.clone(), Arc::new(ps));
            self.stats.entry(name).or_default();
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
}
