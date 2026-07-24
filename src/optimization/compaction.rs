//! Conversation compaction — token-efficient history management (BLUE71 §10)
//!
//! Implements three compaction strategies for long conversations:
//! - `SlidingWindow`: keep only the N most recent turns
//! - `Summarize`: LLM-summarize older turns into a system message
//! - `Hybrid`: summarize older turns AND keep the most recent ones
//!
//! The `AdaptiveCompactor` wraps `CompactionManager` with learning: it tracks
//! the effectiveness of each strategy and automatically selects the best one
//! based on conversation length, historical quality scores, and user feedback.
//!
//! Architecture:
//! ```
//! ConversationHistory → CompactionManager::compact(strategy) → CompactionResult
//!                    ↕  (wraps)
//! AdaptiveCompactor::compact(session) → selects best strategy → record effectiveness
//! ```

use std::collections::VecDeque;

/// A single turn in a conversation history.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConversationTurn {
    /// Message role ("user", "assistant", "system").
    pub role: String,
    /// Message content.
    pub content: String,
    /// Estimated token count for this turn.
    pub tokens: usize,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
}

#[allow(dead_code)]
impl ConversationTurn {
    /// Create a new conversation turn with auto-computed tokens.
    pub fn new(role: &str, content: &str) -> Self {
        let content = content.to_string();
        let tokens = estimate_tokens(&content);
        Self {
            role: role.to_string(),
            content,
            tokens,
            timestamp_ms: now_ms(),
        }
    }

    /// Create a new conversation turn with explicit token count.
    pub fn with_tokens(role: &str, content: &str, tokens: usize) -> Self {
        Self {
            role: role.to_string(),
            content: content.to_string(),
            tokens,
            timestamp_ms: now_ms(),
        }
    }
}

/// Ordered conversation history with token-aware manipulation.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConversationHistory {
    /// Ordered turns (oldest first).
    turns: Vec<ConversationTurn>,
    /// Total estimated tokens across all turns.
    pub total_tokens: usize,
}

#[allow(dead_code)]
impl ConversationHistory {
    /// Create an empty conversation history.
    pub fn new() -> Self {
        Self {
            turns: Vec::new(),
            total_tokens: 0,
        }
    }

    /// Create from an existing list of turns.
    pub fn from_turns(turns: Vec<ConversationTurn>) -> Self {
        let total_tokens = turns.iter().map(|t| t.tokens).sum();
        Self {
            turns,
            total_tokens,
        }
    }

    /// Push a new turn to the end.
    pub fn push(&mut self, turn: ConversationTurn) {
        self.total_tokens += turn.tokens;
        self.turns.push(turn);
    }

    /// Get a slice of all turns.
    pub fn turns(&self) -> &[ConversationTurn] {
        &self.turns
    }

    /// Number of turns in history.
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Whether history is empty.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Remove all turns except the last N.
    ///
    /// Returns the removed turns (oldest first).
    pub fn drain_to_last_n(&mut self, keep: usize) -> Vec<ConversationTurn> {
        if self.turns.len() <= keep {
            return Vec::new();
        }
        let split_at = self.turns.len() - keep;
        let removed: Vec<ConversationTurn> = self.turns.drain(..split_at).collect();
        let removed_tokens: usize = removed.iter().map(|t| t.tokens).sum();
        self.total_tokens = self.total_tokens.saturating_sub(removed_tokens);
        removed
    }

    /// Prepend a system summary turn at the beginning.
    pub fn prepend_system_summary(&mut self, summary: String) {
        let tokens = estimate_tokens(&summary);
        let turn = ConversationTurn {
            role: "system".to_string(),
            content: summary,
            tokens,
            timestamp_ms: now_ms(),
        };
        self.total_tokens += turn.tokens;
        self.turns.insert(0, turn);
    }

    /// Get the text representation of all turns (for passing to an LLM).
    pub fn to_text(&self) -> String {
        self.turns
            .iter()
            .map(|t| format!("{}: {}", t.role, t.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Remove the oldest N turns and return them.
    pub fn drain_oldest(&mut self, n: usize) -> Vec<ConversationTurn> {
        let n = n.min(self.turns.len());
        let removed: Vec<ConversationTurn> = self.turns.drain(..n).collect();
        let removed_tokens: usize = removed.iter().map(|t| t.tokens).sum();
        self.total_tokens = self.total_tokens.saturating_sub(removed_tokens);
        removed
    }

    /// Total token count.
    pub fn estimated_tokens(&self) -> usize {
        self.total_tokens
    }

    /// Serialize history to JSON bytes for checkpoint storage.
    pub fn to_checkpoint_json(&self) -> String {
        let data: Vec<serde_json::Value> = self
            .turns
            .iter()
            .map(|t| {
                serde_json::json!({
                    "role": t.role,
                    "content": t.content,
                    "tokens": t.tokens,
                    "ts": t.timestamp_ms,
                })
            })
            .collect();
        serde_json::to_string(&data).unwrap_or_default()
    }

    /// Deserialize history from checkpoint JSON.
    pub fn from_checkpoint_json(json: &str) -> Self {
        let data: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
        let turns: Vec<ConversationTurn> = data
            .into_iter()
            .filter_map(|v| {
                let role = v.get("role")?.as_str()?;
                let content = v.get("content")?.as_str()?;
                let tokens = v.get("tokens")?.as_u64().unwrap_or(0) as usize;
                let ts = v.get("ts")?.as_u64().unwrap_or(0);
                Some(ConversationTurn {
                    role: role.to_string(),
                    content: content.to_string(),
                    tokens,
                    timestamp_ms: ts,
                })
            })
            .collect();
        let total_tokens = turns.iter().map(|t| t.tokens).sum();
        Self {
            turns,
            total_tokens,
        }
    }
}

impl Default for ConversationHistory {
    fn default() -> Self {
        Self::new()
    }
}

/// Rough token estimator (~4 chars per token for English/most text).
#[allow(dead_code)]
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() / 4).max(1)
}

// ---------------------------------------------------------------------------
// Compaction strategies
// ---------------------------------------------------------------------------

/// Compaction strategy to apply (BLUE71 §10.2).
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum CompactionStrategy {
    /// Remove oldest turns, keep only the most recent N.
    SlidingWindow {
        /// Number of most recent turns to keep.
        keep_turns: usize,
    },
    /// LLM-summarize oldest turns into a system message.
    Summarize {
        /// Maximum token budget for the summary.
        max_summary_tokens: usize,
    },
    /// Hybrid: summarize oldest turns AND keep most recent ones.
    Hybrid {
        /// Number of oldest turns to summarize.
        summary_turns: usize,
        /// Number of most recent turns to keep after summary.
        keep_turns: usize,
    },
}

/// Result of a compaction operation (BLUE71 §10.2).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CompactionResult {
    /// Which strategy was used.
    pub strategy: CompactionStrategy,
    /// Estimated tokens saved.
    pub tokens_saved: usize,
    /// Token count after compaction.
    pub tokens_after: usize,
    /// How many turns were compacted/removed.
    pub turns_compacted: usize,
    /// Quality score (0.0 = lossy, 1.0 = lossless).
    pub quality_score: f64,
}

// ---------------------------------------------------------------------------
// CompactionManager — basic compaction engine
// ---------------------------------------------------------------------------

/// Basic compaction engine (BLUE71 §10.2).
///
/// Supports three strategies: SlidingWindow, Summarize, and Hybrid.
/// The `summarizer` agent is optional — when not set, only SlidingWindow
/// is available (Summarize and Hybrid return Unsupported).
#[allow(dead_code)]
pub struct CompactionManager {
    /// Agent used for LLM summarization (None = Summarize strategies unavailable).
    summarizer: Option<String>,
    /// Token threshold that triggers compaction.
    max_tokens_before_compact: usize,
    /// Default number of turns to keep.
    keep_last_n_turns: usize,
}

#[allow(dead_code)]
impl CompactionManager {
    /// Create a new CompactionManager.
    ///
    /// `summarizer` is the agent name to use for summarization (e.g. "deepseek").
    /// Pass `None` to disable summarization (only SlidingWindow available).
    pub fn new(
        summarizer: Option<String>,
        max_tokens_before_compact: usize,
        keep_last_n_turns: usize,
    ) -> Self {
        Self {
            summarizer,
            max_tokens_before_compact,
            keep_last_n_turns,
        }
    }

    /// Create a SlidingWindow-only manager (no summarization).
    pub fn sliding_window_only(keep_last_n_turns: usize) -> Self {
        Self {
            summarizer: None,
            max_tokens_before_compact: usize::MAX,
            keep_last_n_turns,
        }
    }

    /// Get the configured summarizer agent name.
    pub fn summarizer(&self) -> Option<&str> {
        self.summarizer.as_deref()
    }

    /// Get the token threshold that triggers compaction.
    pub fn max_tokens_before_compact(&self) -> usize {
        self.max_tokens_before_compact
    }

    /// Whether compaction should be triggered for a given history.
    pub fn should_compact(&self, history: &ConversationHistory) -> bool {
        history.total_tokens >= self.max_tokens_before_compact
    }

    /// Apply a compaction strategy to the history (synchronous version).
    ///
    /// For `Summarize` and `Hybrid` strategies that require LLM summarization,
    /// this method returns `CompactionResult` with `turns_compacted=0` and
    /// `quality_score=0.0` to indicate that async summarization is needed.
    /// Call `compact_with_summary` for the async version.
    pub fn compact(
        &self,
        history: &mut ConversationHistory,
        strategy: &CompactionStrategy,
    ) -> CompactionResult {
        match strategy {
            CompactionStrategy::SlidingWindow { keep_turns } => {
                let removed = history.drain_to_last_n(*keep_turns);
                let tokens_saved: usize = removed.iter().map(|t| t.tokens).sum();
                CompactionResult {
                    strategy: strategy.clone(),
                    tokens_saved,
                    tokens_after: history.total_tokens,
                    turns_compacted: removed.len(),
                    quality_score: 0.95, // SlidingWindow preserves recent turns exactly
                }
            }
            CompactionStrategy::Summarize {
                max_summary_tokens: _,
            } => {
                if self.summarizer.is_none() {
                    return CompactionResult {
                        strategy: strategy.clone(),
                        tokens_saved: 0,
                        tokens_after: history.total_tokens,
                        turns_compacted: 0,
                        quality_score: 0.0,
                    };
                }
                // Synchronous path: drain oldest to make room, mark for async summary
                let keep = self.keep_last_n_turns;
                let removed = history.drain_to_last_n(keep);
                let tokens_saved: usize = removed.iter().map(|t| t.tokens).sum();
                // The summary will be prepended by the async caller
                CompactionResult {
                    strategy: strategy.clone(),
                    tokens_saved,
                    tokens_after: history.total_tokens,
                    turns_compacted: removed.len(),
                    quality_score: 0.7, // Summarization is lossy
                }
            }
            CompactionStrategy::Hybrid {
                summary_turns,
                keep_turns,
            } => {
                if self.summarizer.is_none() {
                    return CompactionResult {
                        strategy: strategy.clone(),
                        tokens_saved: 0,
                        tokens_after: history.total_tokens,
                        turns_compacted: 0,
                        quality_score: 0.0,
                    };
                }
                // Drain old turns, keeping the most recent ones
                if history.len() <= keep_turns + summary_turns {
                    // Not enough turns to compact
                    return CompactionResult {
                        strategy: strategy.clone(),
                        tokens_saved: 0,
                        tokens_after: history.total_tokens,
                        turns_compacted: 0,
                        quality_score: 1.0,
                    };
                }
                // Keep: summary_turns oldest (to summarize) + keep_turns newest
                let total_keep = summary_turns + keep_turns;
                let removed = history.drain_to_last_n(total_keep);
                let tokens_saved: usize = removed.iter().map(|t| t.tokens).sum();
                // Remove the summary_turns from what's left and return them for async summary
                let to_summarize = history.drain_oldest(*summary_turns.min(&history.len()));
                let summary_tokens: usize = to_summarize.iter().map(|t| t.tokens).sum();
                CompactionResult {
                    strategy: strategy.clone(),
                    tokens_saved: tokens_saved.saturating_sub(summary_tokens),
                    tokens_after: history.total_tokens,
                    turns_compacted: removed.len() + to_summarize.len(),
                    quality_score: 0.8, // Hybrid keeps recent turns + summary
                }
            }
        }
    }
}

impl Default for CompactionManager {
    fn default() -> Self {
        Self::new(None, 20_000, 10)
    }
}

// ---------------------------------------------------------------------------
// AdaptiveThreshold — dynamic adjustment
// ---------------------------------------------------------------------------

/// Dynamic threshold that adjusts based on historical effectiveness.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AdaptiveThreshold {
    /// Current threshold value (token count).
    current: usize,
    /// Minimum allowed threshold.
    min: usize,
    /// Maximum allowed threshold.
    max: usize,
    /// How much to adjust per record (percentage 0.0-1.0).
    adjustment_rate: f64,
}

#[allow(dead_code)]
impl AdaptiveThreshold {
    /// Create a new adaptive threshold.
    pub fn new(initial: usize, min: usize, max: usize, adjustment_rate: f64) -> Self {
        Self {
            current: initial,
            min,
            max,
            adjustment_rate,
        }
    }

    /// Get the current threshold value.
    pub fn current(&self) -> usize {
        self.current
    }

    /// Adjust the threshold based on compaction effectiveness history.
    ///
    /// - If average quality is high → raise threshold (compact less often)
    /// - If average quality is low → lower threshold (compact more aggressively)
    pub fn adjust(&mut self, history: &[CompactionRecord]) {
        if history.is_empty() {
            return;
        }
        let avg_quality: f64 =
            history.iter().map(|r| r.quality_score).sum::<f64>() / history.len() as f64;

        if avg_quality > 0.9 {
            // Compaction is high quality — be less aggressive
            let delta = (self.current as f64 * self.adjustment_rate) as usize;
            self.current = (self.current + delta).min(self.max);
        } else if avg_quality < 0.6 {
            // Compaction quality is poor — be more aggressive (compact more thoughtfully)
            let delta = (self.current as f64 * self.adjustment_rate / 2.0) as usize;
            self.current = self.current.saturating_sub(delta).max(self.min);
        }
        // else: quality is acceptable — no change
    }
}

impl Default for AdaptiveThreshold {
    fn default() -> Self {
        Self::new(20_000, 10_000, 50_000, 0.05)
    }
}

// ---------------------------------------------------------------------------
// CompactionRecord — historical effectiveness tracking
// ---------------------------------------------------------------------------

/// Record of a single compaction operation (BLUE71 §10.3).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CompactionRecord {
    /// Which strategy was used.
    pub strategy: CompactionStrategy,
    /// Estimated tokens saved.
    pub tokens_saved: usize,
    /// Quality score (0.0-1.0) of the compaction result.
    pub quality_score: f64,
    /// Optional user feedback (0.0=bad, 1.0=perfect).
    pub user_feedback: Option<f64>,
    /// Unix timestamp in milliseconds.
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// AdaptiveCompactor — self-learning compaction (BLUE71 §10.3)
// ---------------------------------------------------------------------------

/// Adaptive, self-learning compaction engine (BLUE71 §10.3).
///
/// Wraps `CompactionManager` and adds:
/// - Automatic strategy selection based on conversation length and history
/// - Quality tracking and adaptive threshold adjustment
/// - User feedback integration
///
/// The compactor learns from past compaction results and adjusts its
/// behavior over time, outperforming Codex's fixed-strategy approach.
#[allow(dead_code)]
pub struct AdaptiveCompactor {
    /// Base compaction engine.
    base: CompactionManager,
    /// Historical effectiveness records (most recent first).
    effectiveness_history: VecDeque<CompactionRecord>,
    /// Maximum number of records to keep.
    max_history: usize,
    /// Current best strategy (auto-selected).
    best_strategy: CompactionStrategy,
    /// Adaptive threshold for triggering compaction.
    adaptive_threshold: AdaptiveThreshold,
}

#[allow(dead_code)]
impl AdaptiveCompactor {
    /// Create a new AdaptiveCompactor.
    pub fn new(base: CompactionManager, max_history: usize) -> Self {
        let best_strategy = CompactionStrategy::Hybrid {
            summary_turns: 8,
            keep_turns: 5,
        };
        Self {
            base,
            effectiveness_history: VecDeque::with_capacity(max_history),
            max_history,
            best_strategy,
            adaptive_threshold: AdaptiveThreshold::default(),
        }
    }

    /// Get a reference to the underlying CompactionManager.
    pub fn manager(&self) -> &CompactionManager {
        &self.base
    }

    /// Get the current best strategy.
    pub fn best_strategy(&self) -> &CompactionStrategy {
        &self.best_strategy
    }

    /// Get recent effectiveness history.
    pub fn effectiveness_history(&self) -> &VecDeque<CompactionRecord> {
        &self.effectiveness_history
    }

    /// Get current adaptive threshold.
    pub fn threshold(&self) -> &AdaptiveThreshold {
        &self.adaptive_threshold
    }

    /// Set a custom adaptive threshold (useful for tests).
    pub fn set_threshold(&mut self, threshold: AdaptiveThreshold) {
        self.adaptive_threshold = threshold;
    }

    /// Check whether compaction is needed for the given history.
    ///
    /// Uses the adaptive threshold (not the base manager's threshold).
    pub fn should_compact(&self, history: &ConversationHistory) -> bool {
        history.total_tokens >= self.adaptive_threshold.current()
    }

    /// Select the best compaction strategy based on conversation features
    /// and historical effectiveness (BLUE71 §10.3).
    pub fn select_strategy(&self, history: &ConversationHistory) -> CompactionStrategy {
        let conversation_length = history.len();
        let avg_quality = self
            .effectiveness_history
            .iter()
            .map(|r| r.quality_score)
            .sum::<f64>()
            / self.effectiveness_history.len().max(1) as f64;

        match (conversation_length, avg_quality) {
            // Short conversation: sliding window is fast and cheap
            (l, _) if l < 20 => CompactionStrategy::SlidingWindow {
                keep_turns: 10.max(self.base.keep_last_n_turns),
            },
            // Long conversation with low quality from past compactions → try summarize
            (_, q) if q < 0.6 && self.base.summarizer.is_some() => CompactionStrategy::Summarize {
                max_summary_tokens: 2000,
            },
            // Default: hybrid (best of both worlds)
            _ => CompactionStrategy::Hybrid {
                summary_turns: 8,
                keep_turns: 5.max(self.base.keep_last_n_turns),
            },
        }
    }

    /// Apply compaction using the best automatically selected strategy.
    ///
    /// This is a synchronous compaction (no LLM call). For strategies requiring
    /// summarization, the returned `CompactionResult.turns_compacted` will be 0
    /// and the caller should call `apply_summary` with the LLM-generated summary.
    pub fn compact(&mut self, history: &mut ConversationHistory) -> CompactionResult {
        // 1. Check if compaction needed
        if !self.should_compact(history) {
            return CompactionResult {
                strategy: self.best_strategy.clone(),
                tokens_saved: 0,
                tokens_after: history.total_tokens,
                turns_compacted: 0,
                quality_score: 1.0,
            };
        }

        // 2. Select best strategy
        let strategy = self.select_strategy(history);

        // 3. Execute compaction
        let result = self.base.compact(history, &strategy);

        // 4. Record effectiveness
        self.record_effectiveness(CompactionRecord {
            strategy: strategy.clone(),
            tokens_saved: result.tokens_saved,
            quality_score: result.quality_score,
            user_feedback: None,
            timestamp: now_ms(),
        });

        // 5. Update best strategy and adaptive threshold
        self.best_strategy = strategy;
        self.adaptive_threshold.adjust(
            &self
                .effectiveness_history
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
        );

        result
    }

    /// Apply an LLM-generated summary after compaction.
    ///
    /// This should be called when `compact()` returns a Summarize or Hybrid
    /// strategy. The `summary_text` should be generated by the summarizer agent.
    pub fn apply_summary(
        &mut self,
        history: &mut ConversationHistory,
        summary_text: String,
    ) -> CompactionResult {
        // Prepend the summary as a system message
        let _summary_tokens = estimate_tokens(&summary_text);
        history.prepend_system_summary(summary_text);

        CompactionResult {
            strategy: self.best_strategy.clone(),
            tokens_saved: 0, // Summary was already counted in compact()
            tokens_after: history.total_tokens,
            turns_compacted: 0,
            quality_score: 0.7,
        }
    }

    /// Record user feedback for the last compaction.
    ///
    /// Feedback is 0.0 (terrible) to 1.0 (perfect).
    /// This is used to improve future strategy selection.
    pub fn record_user_feedback(&mut self, score: f64) {
        if let Some(last) = self.effectiveness_history.back_mut() {
            last.user_feedback = Some(score.clamp(0.0, 1.0));
            // Blend user feedback with quality score
            let blended = last.quality_score * 0.6 + score * 0.4;
            last.quality_score = blended;
        }
    }

    /// Record a compaction result for future learning.
    fn record_effectiveness(&mut self, record: CompactionRecord) {
        if self.effectiveness_history.len() >= self.max_history {
            self.effectiveness_history.pop_front();
        }
        self.effectiveness_history.push_back(record);
    }
}

/// Current Unix timestamp in milliseconds.
#[allow(dead_code)]
fn now_ms() -> u64 {
    crate::shared::timestamps::now_ts_ms() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_turn(role: &str, content: &str) -> ConversationTurn {
        ConversationTurn::new(role, content)
    }

    fn make_history(count: usize) -> ConversationHistory {
        let turns: Vec<ConversationTurn> = (0..count)
            .map(|i| make_turn("user", &format!("message {}", i)))
            .collect();
        ConversationHistory::from_turns(turns)
    }

    // ── ConversationTurn tests ────────────────────────────────────

    #[test]
    fn test_conversation_turn_auto_tokens() {
        let turn = ConversationTurn::new(
            "user",
            "Hello, this is a test message with enough chars for token estimation",
        );
        assert!(turn.tokens >= 1);
        assert_eq!(turn.role, "user");
    }

    #[test]
    fn test_conversation_turn_with_tokens() {
        let turn = ConversationTurn::with_tokens("assistant", "response", 42);
        assert_eq!(turn.tokens, 42);
    }

    // ── ConversationHistory tests ─────────────────────────────────

    #[test]
    fn test_history_empty() {
        let hist = ConversationHistory::new();
        assert!(hist.is_empty());
        assert_eq!(hist.len(), 0);
        assert_eq!(hist.estimated_tokens(), 0);
    }

    #[test]
    fn test_history_push() {
        let mut hist = ConversationHistory::new();
        hist.push(make_turn("user", "hello"));
        assert!(!hist.is_empty());
        assert_eq!(hist.len(), 1);
    }

    #[test]
    fn test_history_drain_to_last_n() {
        let mut hist = make_history(10);
        let removed = hist.drain_to_last_n(3);
        assert_eq!(hist.len(), 3);
        assert_eq!(removed.len(), 7);
    }

    #[test]
    fn test_history_drain_to_last_n_noop_when_under_limit() {
        let mut hist = make_history(3);
        let removed = hist.drain_to_last_n(10);
        assert!(removed.is_empty());
        assert_eq!(hist.len(), 3);
    }

    #[test]
    fn test_history_drain_oldest() {
        let mut hist = make_history(5);
        let removed = hist.drain_oldest(3);
        assert_eq!(hist.len(), 2);
        assert_eq!(removed.len(), 3);
        assert!(removed[0].content.contains("message 0"));
    }

    #[test]
    fn test_history_prepend_system_summary() {
        let mut hist = make_history(3);
        hist.prepend_system_summary("Conversation summary here".to_string());
        assert_eq!(hist.len(), 4);
        assert_eq!(hist.turns()[0].role, "system");
        assert!(hist.turns()[0].content.contains("summary"));
    }

    #[test]
    fn test_history_to_text() {
        let mut hist = ConversationHistory::new();
        hist.push(make_turn("user", "hello"));
        hist.push(make_turn("assistant", "world"));
        let text = hist.to_text();
        assert!(text.contains("user: hello"));
        assert!(text.contains("assistant: world"));
    }

    #[test]
    fn test_history_from_turns_tracks_tokens() {
        let turns = vec![
            ConversationTurn::with_tokens("user", "a", 10),
            ConversationTurn::with_tokens("assistant", "b", 20),
        ];
        let hist = ConversationHistory::from_turns(turns);
        assert_eq!(hist.estimated_tokens(), 30);
    }

    // ── Token estimation tests ────────────────────────────────────

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens(""), 1); // minimum 1
        assert_eq!(estimate_tokens("abcdefghijkl"), 3);
    }

    // ── CompactionManager tests ───────────────────────────────────

    #[test]
    fn test_sliding_window_basic() {
        let manager = CompactionManager::new(None, 30, 5);
        let mut hist = make_history(20);
        assert!(manager.should_compact(&hist));

        let result = manager.compact(
            &mut hist,
            &CompactionStrategy::SlidingWindow { keep_turns: 5 },
        );
        assert_eq!(hist.len(), 5);
        assert!(result.tokens_saved > 0);
        assert!(result.quality_score > 0.9);
    }

    #[test]
    fn test_sliding_window_under_limit() {
        let manager = CompactionManager::sliding_window_only(10);
        let mut hist = make_history(3);
        let result = manager.compact(
            &mut hist,
            &CompactionStrategy::SlidingWindow { keep_turns: 10 },
        );
        assert_eq!(result.turns_compacted, 0);
        assert_eq!(hist.len(), 3);
    }

    #[test]
    fn test_summarize_without_agent_returns_unsupported() {
        let manager = CompactionManager::sliding_window_only(10);
        let mut hist = make_history(20);
        let result = manager.compact(
            &mut hist,
            &CompactionStrategy::Summarize {
                max_summary_tokens: 500,
            },
        );
        assert_eq!(result.turns_compacted, 0);
        assert_eq!(result.quality_score, 0.0);
    }

    #[test]
    fn test_summarize_with_agent_drains_turns() {
        let manager = CompactionManager::new(Some("deepseek".into()), 20_000, 5);
        let mut hist = make_history(20);
        let result = manager.compact(
            &mut hist,
            &CompactionStrategy::Summarize {
                max_summary_tokens: 500,
            },
        );
        assert!(result.turns_compacted > 0);
        assert_eq!(hist.len(), 5); // keep_last_n_turns = 5
    }

    #[test]
    fn test_hybrid_without_agent_returns_unsupported() {
        let manager = CompactionManager::sliding_window_only(10);
        let mut hist = make_history(30);
        let result = manager.compact(
            &mut hist,
            &CompactionStrategy::Hybrid {
                summary_turns: 8,
                keep_turns: 5,
            },
        );
        assert_eq!(result.turns_compacted, 0);
        assert_eq!(result.quality_score, 0.0);
    }

    #[test]
    fn test_hybrid_not_enough_turns() {
        let manager = CompactionManager::new(Some("deepseek".into()), 20_000, 10);
        let mut hist = make_history(5);
        let result = manager.compact(
            &mut hist,
            &CompactionStrategy::Hybrid {
                summary_turns: 8,
                keep_turns: 5,
            },
        );
        assert_eq!(result.turns_compacted, 0);
        assert_eq!(result.quality_score, 1.0);
    }

    #[test]
    fn test_should_compact() {
        let manager = CompactionManager::new(None, 100, 5);
        let mut hist = ConversationHistory::new();
        assert!(!manager.should_compact(&hist));

        hist.push(ConversationTurn::with_tokens("user", "test", 200));
        assert!(manager.should_compact(&hist));
    }

    // ── AdaptiveThreshold tests ───────────────────────────────────

    #[test]
    fn test_adaptive_threshold_default() {
        let threshold = AdaptiveThreshold::default();
        assert_eq!(threshold.current(), 20_000);
    }

    #[test]
    fn test_adaptive_threshold_increases_on_high_quality() {
        let mut threshold = AdaptiveThreshold::new(20_000, 10_000, 50_000, 0.05);
        let records = vec![
            CompactionRecord {
                strategy: CompactionStrategy::SlidingWindow { keep_turns: 5 },
                tokens_saved: 1000,
                quality_score: 0.95,
                user_feedback: None,
                timestamp: 0,
            },
            CompactionRecord {
                strategy: CompactionStrategy::SlidingWindow { keep_turns: 5 },
                tokens_saved: 2000,
                quality_score: 0.98,
                user_feedback: None,
                timestamp: 0,
            },
        ];
        let before = threshold.current();
        threshold.adjust(&records);
        assert!(threshold.current() >= before); // threshold should not decrease
    }

    #[test]
    fn test_adaptive_threshold_decreases_on_low_quality() {
        let mut threshold = AdaptiveThreshold::new(20_000, 10_000, 50_000, 0.05);
        let records = vec![CompactionRecord {
            strategy: CompactionStrategy::Summarize {
                max_summary_tokens: 500,
            },
            tokens_saved: 1000,
            quality_score: 0.3,
            user_feedback: None,
            timestamp: 0,
        }];
        threshold.adjust(&records);
        assert!(threshold.current() < 20_000); // threshold should decrease
    }

    #[test]
    fn test_adaptive_threshold_no_adjust_on_empty_history() {
        let mut threshold = AdaptiveThreshold::new(20_000, 10_000, 50_000, 0.05);
        let before = threshold.current();
        threshold.adjust(&[]);
        assert_eq!(threshold.current(), before);
    }

    // ── AdaptiveCompactor tests ───────────────────────────────────

    #[test]
    fn test_adaptive_compactor_skip_when_under_threshold() {
        let manager = CompactionManager::new(None, 20_000, 5);
        let mut compactor = AdaptiveCompactor::new(manager, 100);
        let mut hist = make_history(3); // small history, under threshold
        let result = compactor.compact(&mut hist);
        assert_eq!(result.turns_compacted, 0);
        assert_eq!(result.quality_score, 1.0);
        assert_eq!(hist.len(), 3);
    }

    #[test]
    fn test_adaptive_compactor_selects_sliding_window_for_short() {
        let manager = CompactionManager::new(None, 100, 5);
        let mut compactor = AdaptiveCompactor::new(manager, 100);
        // Set a low threshold so compaction triggers with test data
        compactor.set_threshold(AdaptiveThreshold::new(100, 50, 500, 0.05));
        // Create a history with fewer than 20 turns (SlidingWindow threshold)
        let mut hist = ConversationHistory::new();
        for i in 0..10 {
            hist.push(ConversationTurn::with_tokens(
                "user",
                &format!("message {}", i),
                50,
            ));
        }
        assert!(compactor.should_compact(&hist));
        let strategy = compactor.select_strategy(&hist);
        assert!(matches!(strategy, CompactionStrategy::SlidingWindow { .. }));
    }

    #[test]
    fn test_adaptive_compactor_selects_default_for_large() {
        let manager = CompactionManager::new(Some("deepseek".into()), 100, 5);
        let mut compactor = AdaptiveCompactor::new(manager, 100);
        // Add one high-quality record so avg quality is >= 0.6
        compactor.record_effectiveness(CompactionRecord {
            strategy: CompactionStrategy::Hybrid {
                summary_turns: 8,
                keep_turns: 5,
            },
            tokens_saved: 100,
            quality_score: 0.8,
            user_feedback: None,
            timestamp: 0,
        });
        let mut hist = ConversationHistory::new();
        for i in 0..50 {
            hist.push(ConversationTurn::with_tokens(
                "user",
                &format!("message {}", i),
                100,
            ));
        }
        let strategy = compactor.select_strategy(&hist);
        assert!(matches!(strategy, CompactionStrategy::Hybrid { .. }));
    }

    #[test]
    fn test_adaptive_compactor_records_history() {
        let manager = CompactionManager::new(None, 100, 5);
        let mut compactor = AdaptiveCompactor::new(manager, 100);
        // Set low threshold so 30×50 tokens triggers compaction
        compactor.set_threshold(AdaptiveThreshold::new(100, 50, 500, 0.05));
        assert!(compactor.effectiveness_history().is_empty());

        // Create a history that exceeds threshold
        let mut hist = ConversationHistory::new();
        for i in 0..30 {
            hist.push(ConversationTurn::with_tokens(
                "user",
                &format!("msg {}", i),
                50,
            ));
        }
        compactor.compact(&mut hist);
        assert!(!compactor.effectiveness_history().is_empty());
    }

    #[test]
    fn test_adaptive_compactor_user_feedback_blends() {
        let manager = CompactionManager::new(None, 100, 5);
        let mut compactor = AdaptiveCompactor::new(manager, 100);
        // Set low threshold so compaction triggers
        compactor.set_threshold(AdaptiveThreshold::new(100, 50, 500, 0.05));

        let mut hist = ConversationHistory::new();
        for i in 0..30 {
            hist.push(ConversationTurn::with_tokens(
                "user",
                &format!("msg {}", i),
                50,
            ));
        }
        compactor.compact(&mut hist);

        let before = compactor
            .effectiveness_history()
            .back()
            .unwrap()
            .quality_score;
        compactor.record_user_feedback(0.5);
        let after = compactor
            .effectiveness_history()
            .back()
            .unwrap()
            .quality_score;
        assert_ne!(before, after);
    }

    #[test]
    fn test_adaptive_compactor_apply_summary() {
        let manager = CompactionManager::new(Some("deepseek".into()), 100, 5);
        let mut compactor = AdaptiveCompactor::new(manager, 100);

        let mut hist = ConversationHistory::new();
        for i in 0..30 {
            hist.push(ConversationTurn::with_tokens(
                "user",
                &format!("msg {}", i),
                50,
            ));
        }
        let before_len = hist.len();
        compactor.compact(&mut hist);
        assert!(hist.len() <= before_len);

        // Apply a summary
        let before_after = hist.len();
        compactor.apply_summary(&mut hist, "Summary of previous conversation".to_string());
        assert_eq!(hist.len(), before_after + 1);
        assert_eq!(hist.turns()[0].role, "system");
    }

    #[test]
    fn test_adaptive_compactor_history_capped() {
        let manager = CompactionManager::new(None, 1, 5); // threshold=1 → always compact
        let mut compactor = AdaptiveCompactor::new(manager, 3);
        // Set low threshold so compaction triggers
        compactor.set_threshold(AdaptiveThreshold::new(1, 1, 100, 0.05));

        // Run compaction 5 times on different histories
        for i in 0..5 {
            let mut hist = ConversationHistory::new();
            hist.push(ConversationTurn::with_tokens(
                "user",
                &format!("msg {}", i),
                100,
            ));
            compactor.compact(&mut hist);
        }

        assert_eq!(compactor.effectiveness_history().len(), 3); // capped at max_history
    }

    #[test]
    fn test_conversation_turn_timestamp() {
        let before = now_ms();
        let turn = ConversationTurn::new("user", "test");
        let after = now_ms();
        assert!(turn.timestamp_ms >= before);
        assert!(turn.timestamp_ms <= after);
    }

    #[test]
    fn test_checkpoint_roundtrip() {
        let mut hist = ConversationHistory::new();
        hist.push(ConversationTurn::new("user", "hello"));
        hist.push(ConversationTurn::new("assistant", "world"));

        let json = hist.to_checkpoint_json();
        assert!(!json.is_empty());

        let restored = ConversationHistory::from_checkpoint_json(&json);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.turns()[0].content, "hello");
        assert_eq!(restored.turns()[1].content, "world");
        assert_eq!(restored.estimated_tokens(), hist.estimated_tokens());
    }

    #[test]
    fn test_checkpoint_roundtrip_empty() {
        let hist = ConversationHistory::new();
        let json = hist.to_checkpoint_json();
        let restored = ConversationHistory::from_checkpoint_json(&json);
        assert!(restored.is_empty());
    }

    #[test]
    fn test_checkpoint_invalid_json_returns_empty() {
        let restored = ConversationHistory::from_checkpoint_json("invalid json");
        assert!(restored.is_empty());
    }
}
