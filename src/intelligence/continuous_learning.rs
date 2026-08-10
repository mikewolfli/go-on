//! BLUE38 F-GAP-24: Continuous Learning Center
//!
//! A thread-safe module that prevents catastrophic forgetting and manages
//! lifelong learning through memory consolidation, forgetting-curve tracking,
//! curriculum scheduling, and experience replay.
//!
//! All mutable state is guarded behind `Arc<Mutex<>>`.

use crate::agents::agent::{Agent, Message, StreamingSender};

/// A thread-safe handle for injecting an agent into the ContinuousLearningCenter
/// after it has been moved into a background task.
pub type AgentInjector = Arc<Mutex<Option<Arc<dyn Agent>>>>;
use crate::i18n::tf;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Lock a std::sync::Mutex with poison recovery.
/// Uses shared `crate::lock_or_recover!` macro.
fn lock_guard<T>(mtx: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    crate::lock_or_recover!(mtx, "continuous_learning")
}

// ---------------------------------------------------------------------------
// Core data structures
// ---------------------------------------------------------------------------

/// A consolidated memory that the system retains for future reuse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidatedMemory {
    /// Unique identifier for this memory.
    pub id: String,
    /// A key used to group or query related memories.
    pub pattern_key: String,
    /// The serialised content of the memory.
    pub data: String,
    /// A measure of how important this memory is (0.0 – 1.0).
    pub importance: f64,
    /// Epoch millisecond when consolidation happened.
    pub consolidated_ms: u64,
    /// How many times this memory has been accessed.
    pub access_count: u64,
    /// Epoch millisecond of the last access.
    pub last_accessed_ms: u64,
}

/// A semantic pattern extracted from consolidated memory data via LLM-like distillation.
///
/// Captures the key terms (keywords) that characterize a memory's content,
/// along with a confidence score and observation frequency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticPattern {
    /// Unique identifier for this pattern
    pub pattern_id: String,
    /// Top-ranked keywords that define this semantic pattern
    pub keywords: Vec<String>,
    /// Confidence in the extracted pattern (0.0 – 1.0)
    pub confidence: f64,
    /// How many times this pattern has been observed
    pub frequency: usize,
}

/// The forgetting curve for a given memory, modelling strength decay over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgettingCurve {
    /// The memory this curve belongs to.
    pub memory_id: String,
    /// The strength immediately after consolidation.
    pub original_strength: f64,
    /// The strength at the current time (decayed).
    pub current_strength: f64,
    /// Epoch millisecond when the memory was last reinforced.
    pub last_reinforced_ms: u64,
    /// The exponential decay rate (per hour).
    pub decay_rate: f64,
}

/// Tracks consecutive low retention scores for forgetting risk detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgettingRiskRecord {
    /// The memory ID this risk record belongs to.
    pub memory_id: String,
    /// Number of consecutive checks where retention_score < 0.1.
    pub consecutive_critical: u32,
    /// Number of consecutive checks where retention_score < 0.3 (but >= 0.1).
    pub consecutive_low: u32,
    /// The last recorded retention score.
    pub last_score: f64,
    /// Unix millisecond timestamp of the last assessment.
    pub last_assessed_ms: u64,
    /// Whether this memory has been flagged for fast eviction.
    pub flagged_for_eviction: bool,
}

impl ForgettingRiskRecord {
    fn new(memory_id: String) -> Self {
        Self {
            memory_id,
            consecutive_critical: 0,
            consecutive_low: 0,
            last_score: 1.0,
            last_assessed_ms: crate::shared::timestamps::now_ts_ms() as u64,
            flagged_for_eviction: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the `ContinuousLearningCenter`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuousLearningConfig {
    /// Maximum number of consolidated memories retained.
    pub max_memories: usize,
    /// Maximum number of learning tasks tracked at once.
    pub max_tasks: usize,
    /// Default decay rate (per hour) for the forgetting curve.
    pub default_decay_rate: f64,
    /// Minimum importance threshold for memory retention.
    pub min_retention_importance: f64,
    /// Number of curriculum stages.
    pub curriculum_stages: u32,
    /// Tasks needed per curriculum stage.
    pub tasks_per_stage: u32,
}

impl Default for ContinuousLearningConfig {
    fn default() -> Self {
        Self {
            max_memories: 5000,
            max_tasks: 1000,
            default_decay_rate: 0.05,
            min_retention_importance: 0.1,
            curriculum_stages: 5,
            tasks_per_stage: 10,
        }
    }
}

// ---------------------------------------------------------------------------
// Continuous Learning Center
// ---------------------------------------------------------------------------

/// The central coordinator for lifelong learning, guarding task management,
/// memory consolidation, forgetting-curve tracking, and curriculum scheduling
/// behind a thread-safe `Arc<Mutex<>>`.
pub struct ContinuousLearningCenter {
    config: ContinuousLearningConfig,
    state: Arc<Mutex<CenterState>>,
    /// Optional agent used for LLM-based semantic distillation.
    /// When `None`, TF-IDF keyword extraction is used as a fallback.
    /// Uses `AgentInjector` so the agent can be injected after the center
    /// has been moved into a background task (e.g. after agent registry init).
    agent: AgentInjector,
}

impl std::fmt::Debug for ContinuousLearningCenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let has_agent = self
            .agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        f.debug_struct("ContinuousLearningCenter")
            .field("config", &self.config)
            .field("state", &self.state)
            .field("agent", &if has_agent { "<agent>" } else { "<none>" })
            .finish()
    }
}

impl Clone for ContinuousLearningCenter {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            agent: Arc::clone(&self.agent),
        }
    }
}

/// Internal mutable state held by the centre.
#[derive(Debug, Default)]
struct CenterState {
    memories: HashMap<String, ConsolidatedMemory>,
    forgetting_curves: HashMap<String, ForgettingCurve>,
    forgetting_risks: HashMap<String, ForgettingRiskRecord>,
    /// Extracted semantic patterns from LLM distillation
    semantic_patterns: HashMap<String, SemanticPattern>,
    next_memory_id: u64,
    next_pattern_id: u64,
}

impl ContinuousLearningCenter {
    /// Creates a new centre with the given configuration.
    pub fn new(config: ContinuousLearningConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(CenterState {
                memories: HashMap::new(),
                forgetting_curves: HashMap::new(),
                forgetting_risks: HashMap::new(),
                semantic_patterns: HashMap::new(),
                next_memory_id: 1,
                next_pattern_id: 1,
            })),
            agent: Arc::new(Mutex::new(None)),
        }
    }

    /// Sets the agent used for LLM-based semantic distillation.
    /// Can be called after construction, even after the center has been
    /// moved into a background task, because `agent` is behind `Arc<Mutex<>>`.
    pub fn inject_agent(&self, agent: Arc<dyn Agent>) {
        let mut guard = self.agent.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(agent);
    }

    // ── Memory consolidation ───────────────────────────────────────────────

    /// Consolidates a new experience into memory and returns its generated ID.
    ///
    /// This also creates a forgetting curve entry for the new memory.
    pub fn consolidate_experience(
        &self,
        pattern_key: &str,
        data: &str,
        importance: f64,
    ) -> Result<String> {
        let importance = importance.clamp(0.0, 1.0);
        let mut state = self.state.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("continuous_learning state lock poisoned, recovering");
            poisoned.into_inner()
        });
        // Evict the least-important memory when at capacity.
        if state.memories.len() >= self.config.max_memories {
            if let Some(oldest_id) = state
                .memories
                .iter()
                .min_by(|(_, a), (_, b)| {
                    let importance_cmp = a
                        .importance
                        .partial_cmp(&b.importance)
                        .unwrap_or(std::cmp::Ordering::Equal);
                    if importance_cmp != std::cmp::Ordering::Equal {
                        importance_cmp
                    } else {
                        a.consolidated_ms.cmp(&b.consolidated_ms)
                    }
                })
                .map(|(id, _)| id.clone())
            {
                state.memories.remove(&oldest_id);
                state.forgetting_curves.remove(&oldest_id);
            }
        }

        let id = format!("mem-{}", state.next_memory_id);
        state.next_memory_id += 1;

        let now = crate::shared::timestamps::now_ts_ms() as u64;
        let memory = ConsolidatedMemory {
            id: id.clone(),
            pattern_key: pattern_key.to_string(),
            data: data.to_string(),
            importance,
            consolidated_ms: now,
            access_count: 0,
            last_accessed_ms: now,
        };

        // Create the forgetting curve for this memory.
        let curve = ForgettingCurve {
            memory_id: id.clone(),
            original_strength: importance,
            current_strength: importance,
            last_reinforced_ms: now,
            decay_rate: self.config.default_decay_rate,
        };

        state.memories.insert(id.clone(), memory);
        state.forgetting_curves.insert(id.clone(), curve);
        Ok(id)
    }

    // consolidate_experience_with_distill was removed — it had zero production
    // callers and created a LazyLock<Runtime> + block_on anti-pattern violating
    // principles #23 and #24.  Callers that need async distillation should await
    // llm_distill() directly in their async context.

    // ── LLM Distillation ───────────────────────────────────────────────────

    /// Distills consolidated memories into semantic patterns.
    ///
    /// When an LLM agent is configured (`self.agent` is `Some`), uses the
    /// agent's chat endpoint to perform semantic pattern extraction via a
    /// structured prompt.  Otherwise falls back to TF-IDF keyword extraction.
    ///
    /// Returns the number of new patterns extracted.
    pub async fn llm_distill(&self) -> usize {
        let agent_opt = self.agent.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(ref agent) = agent_opt {
            // LLM-based distillation — collect all memories and ask the agent
            // to extract semantic patterns.
            let memory_snapshots: Vec<String> = {
                let state = lock_guard(&self.state);
                state
                    .memories
                    .values()
                    .map(|m| format!("ID={}: {}", m.pattern_key, m.data))
                    .collect()
            };

            if memory_snapshots.is_empty() {
                return 0;
            }

            let prompt = format!(
                r#"You are a semantic pattern extractor. Analyse the following consolidated
memories and extract up to 5 distinct semantic patterns.

For each pattern, return a JSON object with these fields:
- "keywords": array of 3-5 representative keywords
- "confidence": float between 0.0 and 1.0
- "frequency": integer count (how many memories match this pattern)

Return ONLY a JSON array, no markdown or other text.

Memories:
---
{}
---"#,
                memory_snapshots.join("\n")
            );

            let messages = vec![
                Message {
                    role: "system".to_string(),
                    content: "You are a semantic pattern extraction assistant.".to_string(),
                },
                Message {
                    role: "user".to_string(),
                    content: prompt,
                },
            ];

            // Collect streamed response using proper async patterns.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let sender = StreamingSender::new(tx);

            let result = agent.chat(messages, None, None, sender).await;

            if result.is_err() {
                tracing::warn!("continuous_learning: LLM distillation chat failed");
                return 0;
            }

            // Collect all streamed tokens into one response string.
            let mut buf = String::new();
            while let Some(token) = rx.recv().await {
                buf.push_str(&token);
            }
            let response = buf;

            if response.is_empty() {
                return 0;
            }

            // Parse the LLM response as JSON array of patterns.
            let cleaned = response
                .trim()
                .strip_prefix("```json")
                .or_else(|| response.trim().strip_prefix("```"))
                .and_then(|s| s.strip_suffix("```"))
                .unwrap_or(response.trim());

            let llm_patterns: Vec<serde_json::Value> = match serde_json::from_str(cleaned) {
                Ok(v) => v,
                Err(_) => {
                    // Try to find a JSON array anywhere in the response
                    if let Some(start) = cleaned.find('[') {
                        if let Some(end) = cleaned[start..].rfind(']') {
                            let sub = &cleaned[start..=start + end];
                            serde_json::from_str(sub).unwrap_or_default()
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    }
                }
            };

            // Persist the LLM-extracted patterns.
            let mut state = lock_guard(&self.state);
            let mut count = 0usize;
            for p in &llm_patterns {
                let keywords: Vec<String> = p
                    .get("keywords")
                    .and_then(|k| k.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                if keywords.is_empty() {
                    continue;
                }

                let confidence = p
                    .get("confidence")
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.5)
                    .clamp(0.0, 1.0);

                let frequency = p
                    .get("frequency")
                    .and_then(|f| f.as_u64())
                    .unwrap_or(1)
                    .max(1) as usize;

                let pattern = SemanticPattern {
                    pattern_id: format!("pat-{}-llm", state.next_pattern_id),
                    keywords,
                    confidence,
                    frequency,
                };

                state.next_pattern_id += 1;
                state
                    .semantic_patterns
                    .insert(pattern.pattern_id.clone(), pattern);
                count += 1;
            }

            count
        } else {
            // No agent — fall back to TF-IDF keyword extraction.
            let patterns = self.extract_semantic_patterns();
            patterns.len()
        }
    }

    /// Analyzes all stored memories and extracts semantic patterns using
    /// TF-IDF scoring.
    ///
    /// Internal steps:
    /// 1. Tokenize each memory's `data` field into lowercase alphanumeric terms
    /// 2. Compute term frequency (TF) per memory
    /// 3. Compute inverse document frequency (IDF) across all memories
    /// 4. Score each term as TF × IDF
    /// 5. Keep the top-5 keywords per memory as a `SemanticPattern`
    ///
    /// Extracted patterns are both returned and persisted in the center's
    /// semantic pattern store.
    fn extract_semantic_patterns(&self) -> Vec<SemanticPattern> {
        // Collect memory data into owned Vec to avoid borrow conflicts
        // when mutating state later.
        let memory_snapshots: Vec<(String, String)> = {
            let state = lock_guard(&self.state);
            state
                .memories
                .values()
                .map(|m| (m.id.clone(), m.data.clone()))
                .collect()
        };

        if memory_snapshots.is_empty() {
            return Vec::new();
        }

        // Tokenize all memories and compute document frequency.
        let mut all_tokens: Vec<Vec<String>> = Vec::with_capacity(memory_snapshots.len());
        let mut doc_freq: HashMap<String, usize> = HashMap::new();

        for (_, data) in &memory_snapshots {
            let tokens: Vec<String> = data
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty() && s.len() > 2)
                .map(|s| s.to_string())
                .collect();

            let unique_tokens: HashSet<&str> = tokens.iter().map(|s| s.as_str()).collect();
            for token in unique_tokens {
                *doc_freq.entry(token.to_string()).or_insert(0) += 1;
            }
            all_tokens.push(tokens);
        }

        let n_docs = memory_snapshots.len() as f64;
        let mut patterns = Vec::new();

        for (i, (mem_id, data)) in memory_snapshots.iter().enumerate() {
            let tokens = &all_tokens[i];
            let total_terms = tokens.len() as f64;
            if total_terms < 1.0 {
                continue;
            }

            let mut term_freq: HashMap<&str, usize> = HashMap::new();
            for token in tokens {
                *term_freq.entry(token).or_insert(0) += 1;
            }

            let mut scored: Vec<(String, f64)> = term_freq
                .iter()
                .map(|(term, &freq)| {
                    let tf = freq as f64 / total_terms;
                    let df = doc_freq.get(*term).copied().unwrap_or(1) as f64;
                    let idf = (n_docs / df).ln() + 1.0;
                    let score = tf * idf;
                    (term.to_string(), score)
                })
                .collect();

            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let top_k: Vec<(String, f64)> = scored.into_iter().take(5).collect();
            if top_k.is_empty() {
                continue;
            }

            let avg_confidence = top_k.iter().map(|(_, s)| s).sum::<f64>() / top_k.len() as f64;

            // Now lock state for mutation — no outstanding immutable borrows.
            let mut state = lock_guard(&self.state);

            let pattern = SemanticPattern {
                pattern_id: format!("pat-{}", state.next_pattern_id),
                keywords: top_k.into_iter().map(|(k, _)| k).collect(),
                confidence: avg_confidence.clamp(0.0, 1.0),
                frequency: state
                    .semantic_patterns
                    .values()
                    .filter(|p| {
                        data.to_lowercase()
                            .contains(&p.keywords.first().cloned().unwrap_or_default())
                    })
                    .count()
                    .max(1),
            };

            state.next_pattern_id += 1;
            state
                .semantic_patterns
                .insert(pattern.pattern_id.clone(), pattern.clone());
            patterns.push(pattern);
            let _ = mem_id;
        }

        patterns
    }

    /// Reinforces a memory by resetting its forgetting curve strength.
    pub fn reinforce_memory(&self, memory_id: &str) -> Result<()> {
        let mut state = lock_guard(&self.state);

        let curve = state
            .forgetting_curves
            .get_mut(memory_id)
            .with_context(|| {
                tf(
                    "error.continuous_learning.memory_not_found",
                    &[("id", memory_id)],
                )
            })?;

        let now = crate::shared::timestamps::now_ts_ms() as u64;
        curve.current_strength = curve.original_strength;
        curve.last_reinforced_ms = now;

        // Update the memory's access stats in a separate borrow scope.
        if let Some(memory) = state.memories.get_mut(memory_id) {
            memory.access_count += 1;
            memory.last_accessed_ms = now;
        }

        Ok(())
    }

    // ── Forgetting detection ───────────────────────────────────────────────

    /// Ebbinghaus exponential-decay retention strength (single implementation).
    ///
    /// `strength = base * exp(-decay_rate * elapsed_hours)`. All four callers
    /// (detect_forgetting / estimate_retention / retention_score /
    /// detect_forgetting_risk) previously inlined this formula with subtly
    /// different data sources; this is now the one shared helper.
    fn retention_strength(base: f64, decay_rate: f64, now_ms: u64, last_ms: u64) -> f64 {
        let elapsed_ms = now_ms.saturating_sub(last_ms);
        let elapsed_hours = elapsed_ms as f64 / 3_600_000.0;
        base * (-decay_rate * elapsed_hours).exp()
    }

    /// Detects all memories whose current forgetting-curve strength has
    /// dropped below `min_retention_importance` and returns them.
    pub fn detect_forgetting(&self) -> Vec<ForgettingCurve> {
        let state = lock_guard(&self.state);
        let now = crate::shared::timestamps::now_ts_ms() as u64;
        state
            .forgetting_curves
            .values()
            .filter(|curve| {
                Self::retention_strength(
                    curve.original_strength,
                    curve.decay_rate,
                    now,
                    curve.last_reinforced_ms,
                ) < self.config.min_retention_importance
            })
            .cloned()
            .collect()
    }

    // ── Experience replay ──────────────────────────────────────────────────

    /// Returns the `count` most important memories for replay (ordered by
    /// importance descending then by last-accessed ascending).
    pub fn replay_important_memories(&self, count: usize) -> Vec<ConsolidatedMemory> {
        let state = lock_guard(&self.state);
        let mut memories: Vec<_> = state.memories.values().cloned().collect();
        // Sort by importance descending, then by last-accessed ascending (LRU bias).
        memories.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.last_accessed_ms.cmp(&b.last_accessed_ms))
        });
        memories.truncate(count);
        memories
    }

    // ── Retention estimation ───────────────────────────────────────────────

    /// Estimates the current retention strength for a given memory using the
    /// exponential forgetting curve:
    ///
    /// `current_strength = original_strength * exp(-decay_rate * elapsed_hours)`
    pub fn estimate_retention(&self, memory_id: &str) -> f64 {
        let state = lock_guard(&self.state);
        match state.forgetting_curves.get(memory_id) {
            Some(curve) => {
                let now = crate::shared::timestamps::now_ts_ms() as u64;
                Self::retention_strength(
                    curve.original_strength,
                    curve.decay_rate,
                    now,
                    curve.last_reinforced_ms,
                )
            }
            None => 0.0,
        }
    }

    // ── Retention scoring (GAP-B52-14) ────────────────────────────────────

    /// Compute the retention score for a given memory entry using the
    /// **Ebbinghaus forgetting curve** formula at a given point in time.
    ///
    /// The formula is derived from the exponential decay model:
    ///
    /// ```text
    /// S(t) = I * exp(-d * Δt)
    /// ```
    ///
    /// where:
    /// - `I` = original strength (importance)
    /// - `d` = decay rate (per hour)
    /// - `Δt` = hours elapsed since last reinforcement
    ///
    /// Returns a value in `[0.0, 1.0]`.
    pub fn retention_score(&self, entry: &ConsolidatedMemory, now: u64) -> f64 {
        let state = lock_guard(&self.state);
        match state.forgetting_curves.get(&entry.id) {
            Some(curve) => Self::retention_strength(
                entry.importance,
                curve.decay_rate,
                now,
                curve.last_reinforced_ms,
            ),
            None => {
                // No curve → score based purely on recency of consolidation.
                Self::retention_strength(
                    entry.importance,
                    self.config.default_decay_rate,
                    now,
                    entry.consolidated_ms,
                )
            }
        }
    }

    /// Detect memories at risk of being forgotten (retention score < 0.3)
    /// and update their `ForgettingRiskRecord`.
    ///
    /// Memories with score < 0.3 are returned for replay consideration.
    /// Memories with score < 0.1 for 3 consecutive checks are flagged for
    /// fast eviction.
    ///
    /// Returns the list of `ForgettingRiskRecord` entries that are currently
    /// at risk (score < 0.3).
    pub fn detect_forgetting_risk(&self) -> Vec<ForgettingRiskRecord> {
        let mut state = lock_guard(&self.state);
        let now = crate::shared::timestamps::now_ts_ms() as u64;
        let mut at_risk = Vec::new();

        // Collect memory IDs to assess.
        let memory_ids: Vec<String> = state.memories.keys().cloned().collect();

        for id in memory_ids {
            let (importance, consolidated_ms) = match state.memories.get(&id) {
                Some(m) => (m.importance, m.consolidated_ms),
                None => continue,
            };

            // Compute retention score using the Ebbinghaus formula.
            let score = match state.forgetting_curves.get(&id) {
                Some(c) => {
                    Self::retention_strength(importance, c.decay_rate, now, c.last_reinforced_ms)
                }
                None => Self::retention_strength(
                    importance,
                    self.config.default_decay_rate,
                    now,
                    consolidated_ms,
                ),
            };

            // Update or create the forgetting risk record.
            let record = state
                .forgetting_risks
                .entry(id.clone())
                .or_insert_with(|| ForgettingRiskRecord::new(id.clone()));

            // Update consecutive counters.
            if score < 0.1 {
                record.consecutive_critical += 1;
                record.consecutive_low = 0;
            } else if score < 0.3 {
                record.consecutive_low += 1;
                record.consecutive_critical = 0;
            } else {
                // Score is healthy; reset counters.
                record.consecutive_critical = 0;
                record.consecutive_low = 0;
            }

            record.last_score = score;
            record.last_assessed_ms = now;

            // Flag for fast eviction if < 0.1 for 3 consecutive checks.
            if record.consecutive_critical >= 3 {
                record.flagged_for_eviction = true;
            }

            // Collect if at risk (score < 0.3).
            if score < 0.3 {
                at_risk.push(record.clone());
            }
        }

        at_risk
    }

    /// Returns the IDs of memories that should be fast-evicted.
    ///
    /// A memory is a candidate for fast eviction when its retention score
    /// has been < 0.1 for 3 consecutive assessments.
    pub fn fast_evict_candidates(&self) -> Vec<String> {
        let state = lock_guard(&self.state);
        state
            .forgetting_risks
            .values()
            .filter(|r| r.flagged_for_eviction)
            .map(|r| r.memory_id.clone())
            .collect()
    }

    /// Returns the consolidated importance of a memory (0.0 if unknown).
    fn memory_importance(&self, memory_id: &str) -> f64 {
        let state = lock_guard(&self.state);
        state
            .memories
            .get(memory_id)
            .map(|m| m.importance)
            .unwrap_or(0.0)
    }

    /// Perform a forgetting review cycle with full learning loop integration:
    /// 1. LLM distillation — create semantic summaries from consolidated memories.
    /// 2. Detect forgetting (raw `detect_forgetting`) and reinforce decaying memories.
    /// 3. Replay important memories via `replay_important_memories()` for spaced repetition.
    /// 4. Detect forgetting risks and reinforce at-risk memories (original logic).
    /// 5. Fast-evict memories with 3+ consecutive critical scores.
    ///
    /// Returns `(replayed, evicted, patterns_extracted)`.
    ///
    /// # Unified forgetting policy
    ///
    /// The rescue loop (steps 2/5) and the eviction loop (steps 4-6) share one
    /// discriminator: only memories consolidated with importance at or above
    /// `min_retention_importance` are worth rescuing. Memories below the
    /// retention threshold — or already flagged for eviction — are deliberately
    /// not reinforced, so their consecutive-critical counter can reach the
    /// 3-assessment eviction threshold. This prevents the two loops from
    /// fighting each other (rescuing a low-value memory every cycle resets its
    /// decay clock and the eviction path can never fire).
    pub async fn review_cycle(&self, _agent: &str) -> (usize, usize, usize) {
        // Step 1: LLM distillation — semantic summarisation instead of JSON string rotation.
        let patterns = self.llm_distill().await;

        // Step 2: Detect forgetting (raw forgetting-curve check) and reinforce
        // only the memories the eviction loop is not responsible for.
        let forgotten = self.detect_forgetting();
        let rescue_skip: std::collections::HashSet<String> = {
            let state = lock_guard(&self.state);
            forgotten
                .iter()
                .filter(|curve| {
                    let importance = state
                        .memories
                        .get(&curve.memory_id)
                        .map(|m| m.importance)
                        .unwrap_or(0.0);
                    let flagged = state
                        .forgetting_risks
                        .get(&curve.memory_id)
                        .map(|r| r.flagged_for_eviction)
                        .unwrap_or(false);
                    flagged || importance < self.config.min_retention_importance
                })
                .map(|curve| curve.memory_id.clone())
                .collect()
        };
        for curve in &forgotten {
            if rescue_skip.contains(&curve.memory_id) {
                continue; // Owned by the eviction loop — do not rescue.
            }
            let _ = self.reinforce_memory(&curve.memory_id);
        }

        // Step 3: Replay important memories (spaced repetition).
        let _important = self.replay_important_memories(5);

        // Step 4: Detect forgetting risks.
        let at_risk = self.detect_forgetting_risk();

        // Step 5: Replay important at-risk memories (same unified policy as
        // step 2 — never rescue what the eviction loop owns).
        let mut replayed = 0usize;
        for record in &at_risk {
            if record.flagged_for_eviction {
                continue; // Will be evicted instead.
            }
            if self.memory_importance(&record.memory_id) < self.config.min_retention_importance {
                continue; // Low-value memory — owned by the eviction loop.
            }
            if self.reinforce_memory(&record.memory_id).is_ok() {
                replayed += 1;
            }
        }

        // Step 6: Fast-evict memories flagged for eviction.
        let evict_ids = self.fast_evict_candidates();
        let evicted = evict_ids.len();
        {
            let mut state = lock_guard(&self.state);
            for id in &evict_ids {
                state.memories.remove(id);
                state.forgetting_curves.remove(id);
                state.forgetting_risks.remove(id);
            }
        }

        (replayed, evicted, patterns)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::test as async_test;

    /// Helper: builds a default centre for testing.
    fn test_center() -> ContinuousLearningCenter {
        ContinuousLearningCenter::new(ContinuousLearningConfig::default())
    }

    // ── 1. Empty state ────────────────────────────────────────────────────

    #[test]
    fn test_empty_state() {
        let center = test_center();
        assert!(center.detect_forgetting().is_empty());
        assert!(center.replay_important_memories(10).is_empty());
    }

    // ── 2. Consolidate / Reinforce memories ─────────────────────────────────

    #[test]
    fn test_consolidate_experience() -> Result<()> {
        let center = test_center();
        let mem_id = center.consolidate_experience("pattern-a", "some data", 0.9)?;
        assert!(mem_id.starts_with("mem-"));

        let replayed = center.replay_important_memories(10);
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].data, "some data");
        Ok(())
    }

    #[test]
    fn test_reinforce_memory() -> Result<()> {
        let center = test_center();
        let mem_id = center.consolidate_experience("pattern-b", "reinforce me", 0.7)?;

        // Before reinforcement, strength should be roughly original.
        let before = center.estimate_retention(&mem_id);
        assert!((before - 0.7).abs() < 0.01 || before <= 0.7);

        center.reinforce_memory(&mem_id)?;
        let after = center.estimate_retention(&mem_id);
        assert!((after - 0.7).abs() < 0.01);

        Ok(())
    }

    // ── 5. LLM Distillation ────────────────────────────────────────────────

    #[test]
    fn test_semantic_pattern_struct() {
        let pattern = SemanticPattern {
            pattern_id: "pat-1".to_string(),
            keywords: vec!["error".to_string(), "timeout".to_string()],
            confidence: 0.85,
            frequency: 3,
        };
        assert_eq!(pattern.pattern_id, "pat-1");
        assert_eq!(pattern.keywords.len(), 2);
        assert!((pattern.confidence - 0.85).abs() < 1e-9);
        assert_eq!(pattern.frequency, 3);
    }

    #[async_test]
    async fn test_llm_distill_empty_center() {
        let center = test_center();
        let count = center.llm_distill().await;
        assert_eq!(count, 0);
    }

    #[async_test]
    async fn test_llm_distill_extracts_patterns() -> Result<()> {
        let center = test_center();
        center.consolidate_experience(
            "sys-error",
            "system timeout error detected during database connection",
            0.9,
        )?;
        center.consolidate_experience(
            "net-fail",
            "network failure timeout connecting to remote host",
            0.7,
        )?;
        center.consolidate_experience(
            "auth-ok",
            "user authentication successful session token issued",
            0.5,
        )?;

        let count = center.llm_distill().await;
        // Each of the 3 memories should produce a pattern
        assert_eq!(count, 3);

        // Check patterns are queryable from center state
        let state = lock_guard(&center.state);
        assert_eq!(state.semantic_patterns.len(), 3);
        for pattern in state.semantic_patterns.values() {
            assert!(!pattern.keywords.is_empty());
            assert!(pattern.confidence >= 0.0);
            assert!(pattern.confidence <= 1.0);
            assert!(pattern.frequency >= 1);
        }
        Ok(())
    }

    // ── 6. Detect forgetting ──────────────────────────────────────────────

    #[test]
    fn test_detect_forgetting() -> Result<()> {
        // Use a config with a high threshold so a fresh memory with low
        // importance will appear to be forgotten.
        let config = ContinuousLearningConfig {
            min_retention_importance: 0.9,
            default_decay_rate: 1.0, // very fast decay
            ..ContinuousLearningConfig::default()
        };
        let center = ContinuousLearningCenter::new(config);
        center.consolidate_experience("fading", "data", 0.3)?;

        let forgotten = center.detect_forgetting();
        // With decay_rate 1.0 and threshold 0.9, the memory (strength 0.3
        // originally) should already be below threshold after 0 hours.
        assert!(
            !forgotten.is_empty(),
            "expected at least one forgotten memory"
        );
        Ok(())
    }

    // ── 7. Replay ──────────────────────────────────────────────────────────

    #[test]
    fn test_replay_important_memories() -> Result<()> {
        let center = test_center();
        center.consolidate_experience("a", "low", 0.2)?;
        center.consolidate_experience("b", "high", 0.9)?;
        center.consolidate_experience("c", "mid", 0.5)?;

        let replayed = center.replay_important_memories(2);
        assert_eq!(replayed.len(), 2);
        // Should return the two most important.
        assert_eq!(replayed[0].data, "high");
        assert_eq!(replayed[1].data, "mid");
        Ok(())
    }

    // ── 8. Retention estimation ────────────────────────────────────────────

    #[test]
    fn test_estimate_retention() -> Result<()> {
        let center = test_center();
        let mem_id = center.consolidate_experience("ret", "data", 0.8)?;

        // Immediately after consolidation, retention ≈ original strength.
        let retention = center.estimate_retention(&mem_id);
        assert!((retention - 0.8).abs() < 0.01);

        Ok(())
    }

    // ── 9. Forgetting curve decay ──────────────────────────────────────────

    #[test]
    fn test_forgetting_curve_decay_formula() {
        // Verifies: current_strength = original * exp(-decay * elapsed_hours)
        let original: f64 = 1.0;
        let decay_rate: f64 = 0.1;
        let elapsed_hours: f64 = 10.0;
        let strength = original * (-decay_rate * elapsed_hours).exp();
        let expected = (-1.0_f64).exp(); // e^-1 ≈ 0.3679
        assert!((strength - expected).abs() < 0.001);
    }

    // ── 10. Eviction vs rescue unification ─────────────────────────────────

    #[async_test]
    async fn test_low_value_memory_is_evicted_after_consecutive_critical_cycles() {
        let config = ContinuousLearningConfig {
            default_decay_rate: 1.0, // fast decay
            ..ContinuousLearningConfig::default()
        };
        let center = ContinuousLearningCenter::new(config);
        // Consolidated with importance below min_retention_importance (0.1):
        // the rescue loop must NOT keep it alive; the eviction loop owns it.
        let mem_id = center
            .consolidate_experience("low-value", "data", 0.05)
            .expect("consolidation should succeed");

        // Cycle 1-2: consecutive critical counts accumulate (no rescue).
        let (replayed_1, evicted_1, _) = center.review_cycle("test-agent").await;
        assert_eq!(
            evicted_1, 0,
            "not evicted before 3 consecutive critical checks"
        );
        assert_eq!(
            replayed_1, 0,
            "low-value memory must not be replayed/rescued"
        );
        assert!(center.fast_evict_candidates().is_empty());

        let (_, evicted_2, _) = center.review_cycle("test-agent").await;
        assert_eq!(evicted_2, 0);
        assert!(center.fast_evict_candidates().is_empty());

        // Cycle 3: flagged for eviction and removed.
        let (_, evicted_3, _) = center.review_cycle("test-agent").await;
        assert_eq!(evicted_3, 1, "3 consecutive critical checks must evict");
        // The memory (and its curve) are gone.
        assert!(center.fast_evict_candidates().is_empty());
        assert_eq!(center.estimate_retention(&mem_id), 0.0);
        assert!(!center
            .detect_forgetting()
            .iter()
            .any(|c| c.memory_id == mem_id));
    }

    #[async_test]
    async fn test_high_value_memory_is_rescued_and_never_evicted() {
        let config = ContinuousLearningConfig {
            default_decay_rate: 1.0, // fast decay so it sits at-risk initially
            ..ContinuousLearningConfig::default()
        };
        let center = ContinuousLearningCenter::new(config);
        // Consolidated with importance above the retention threshold but below
        // 0.3, so it is at-risk on the first assessment.
        let mem_id = center
            .consolidate_experience("high-value", "data", 0.25)
            .expect("consolidation should succeed");

        // Cycle 1: at-risk (< 0.3) but above the critical threshold (0.1) —
        // the rescue loop replays it instead of letting it decay.
        let (replayed_1, evicted_1, _) = center.review_cycle("test-agent").await;
        assert_eq!(evicted_1, 0);
        assert_eq!(replayed_1, 1, "high-value at-risk memory must be replayed");

        // Across many cycles the memory is rescued each round, so the
        // 3-consecutive-critical eviction never fires.
        for _ in 0..5 {
            center.review_cycle("test-agent").await;
        }
        assert!(center.fast_evict_candidates().is_empty());
        assert!(center.estimate_retention(&mem_id) >= 0.1);
    }
}
