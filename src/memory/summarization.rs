//! Memory summarization and compression for long-term retention.
//!
//! Uses progressive summarization: when memory entries for a conversation
//! exceed a threshold, they are summarized into a compressed form so that
//! important context is retained without unbounded growth.

use crate::memory::memory_persistence::{MemoryEntry, MemoryTier};

/// Configuration for memory summarization.
#[allow(dead_code)] // F-GAP reserved
#[derive(Debug, Clone)]
pub struct SummarizationConfig {
    /// Maximum number of entries before summarization is triggered.
    pub max_entries_before_summary: usize,
    /// Maximum summary text length in characters.
    pub max_summary_chars: usize,
    /// Whether to use LLM-based summarization (vs simple truncation/composition).
    pub use_llm_summarization: bool,
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            max_entries_before_summary: 20,
            max_summary_chars: 4096,
            use_llm_summarization: true,
        }
    }
}

/// A progressive memory summarizer that compresses groups of entries
/// into a compact summary when the group size exceeds the configured
/// threshold.
#[allow(dead_code)] // F-GAP reserved
pub struct MemorySummarizer {
    config: SummarizationConfig,
}

impl MemorySummarizer {
    /// Create a new summarizer with the given configuration.
    #[allow(dead_code)] // F-GAP reserved
    pub fn new(config: SummarizationConfig) -> Self {
        Self { config }
    }

    /// Summarize a list of memory entries, returning a compressed representation.
    ///
    /// When the entry count is at or below the threshold, the entries are
    /// returned as-is (`SummarizedMemory::Full`).  Above the threshold, the
    /// most useful / most recently accessed entries are retained and a synthetic
    /// summary entry is appended (`SummarizedMemory::Compressed`).
    #[allow(dead_code)] // F-GAP reserved
    pub async fn summarize(&self, entries: &[MemoryEntry]) -> SummarizedMemory {
        if entries.len() <= self.config.max_entries_before_summary {
            return SummarizedMemory::Full(entries.to_vec());
        }

        // Use LLM-based summarization when configured
        if self.config.use_llm_summarization {
            let summary = llm_summarize(entries).await;
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            return SummarizedMemory::Compressed(vec![MemoryEntry {
                id: format!("llm-summary-{}", now_secs * 1000),
                tier: MemoryTier::Hot,
                class: "LLMSummarized".to_string(),
                content: summary,
                created_at: now_secs,
                accessed_at: now_secs,
                usefulness: 1.0,
                embedding: None,
                access_count: 1,
                session_id: None,
            }]);
        }

        // Sort by usefulness (descending), then by accessed_at (descending, most recent first).
        let mut sorted = entries.to_vec();
        sorted.sort_by(|a, b| {
            b.usefulness
                .partial_cmp(&a.usefulness)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.accessed_at.cmp(&a.accessed_at))
        });

        let keep_count = self.config.max_entries_before_summary / 2;
        let mut compressed: Vec<MemoryEntry> = sorted.into_iter().take(keep_count).collect();

        // ── Build a summary text from the discarded entries ──
        let summary_text = {
            let discarded = entries.len() - keep_count;
            let snippets: Vec<String> = entries
                .iter()
                .skip(keep_count)
                .map(|e| e.content.chars().take(100).collect::<String>())
                .collect();
            let joined = snippets.join(" | ");
            let truncated: String = joined.chars().take(self.config.max_summary_chars).collect();
            format!("[Summarized {} older entries: {}]", discarded, truncated)
        };

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        compressed.push(MemoryEntry {
            id: format!("summary-{}", now_secs * 1000),
            tier: MemoryTier::Hot,
            class: "Summarized".to_string(),
            content: summary_text,
            created_at: now_secs,
            accessed_at: now_secs,
            usefulness: 1.0, // summaries are always high-value for retrieval
            embedding: None,
            access_count: 1,
            session_id: None,
        });

        SummarizedMemory::Compressed(compressed)
    }

    /// Convenience: return `true` when the entry count exceeds the threshold
    /// and summarization would actually reduce the set.
    #[allow(dead_code)] // F-GAP reserved
    pub fn should_summarize(&self, entry_count: usize) -> bool {
        entry_count > self.config.max_entries_before_summary
    }
}

/// LLM-based summarization of memory entries.
///
/// Builds a prompt from the given entries and produces a concise summary.
/// Currently uses a simple text-truncation approach with token counting
/// as the default fallback. To use a real LLM, inject an LLM client and
/// call it with the built prompt instead.
///
/// TODO-BLUE64: Replace the fallback concatenation with an actual LLM
/// call when an LLM client is available in this module.
pub async fn llm_summarize(entries: &[MemoryEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    // Build a prompt that a real LLM would receive
    let mut prompt = String::from("Please summarize the following memory entries:\n\n");
    for entry in entries {
        prompt.push_str(&format!(
            "- [{}] (usefulness: {:.2}) {}\n",
            entry.class, entry.usefulness, entry.content
        ));
    }
    prompt.push_str("\nProvide a concise summary capturing the key information.");

    // Fallback: simple token-counting and text truncation.
    // A real LLM client would call an API here and return the response.
    const AVG_CHARS_PER_TOKEN: usize = 4;
    const MAX_TOKENS: usize = 1024;
    let max_chars = MAX_TOKENS * AVG_CHARS_PER_TOKEN;

    // Build a truncated summary from the entry contents
    let mut summary = String::new();
    let mut char_count = 0;
    for entry in entries {
        let snippet = entry.content.chars().take(200).collect::<String>();
        if char_count + snippet.len() > max_chars {
            let remaining = max_chars.saturating_sub(char_count);
            if remaining > 20 {
                let truncated: String = snippet.chars().take(remaining).collect();
                summary.push_str(&truncated);
                summary.push_str("...");
            }
            break;
        }
        summary.push_str(&snippet);
        summary.push('\n');
        char_count += snippet.len() + 1;
    }

    if summary.is_empty() {
        // Absolute fallback: just use the first entry's truncated content
        entries
            .first()
            .map(|e| e.content.chars().take(500).collect())
            .unwrap_or_default()
    } else {
        format!(
            "[LLM summarization pending — LLM client not injected; using fallback]\n\nSummarized {} entries:\n{}",
            entries.len(),
            summary
        )
    }
}

/// The result of a summarization operation.
#[allow(dead_code)] // F-GAP reserved
#[derive(Debug, Clone)]
pub enum SummarizedMemory {
    /// The original set of entries was small enough to keep as-is.
    Full(Vec<MemoryEntry>),
    /// The entries were compressed: a subset of high-value entries plus
    /// a synthetic summary entry that captures the discarded content.
    Compressed(Vec<MemoryEntry>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::memory_persistence::{MemoryEntry, MemoryTier};

    fn make_entry(id: &str, usefulness: f32, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            tier: MemoryTier::Hot,
            class: "Test".to_string(),
            content: content.to_string(),
            created_at: 1000,
            accessed_at: 1000,
            usefulness,
            embedding: None,
            access_count: 1,
            session_id: None,
        }
    }

    #[tokio::test]
    async fn test_below_threshold_returns_full() {
        let config = SummarizationConfig {
            max_entries_before_summary: 10,
            max_summary_chars: 512,
            use_llm_summarization: false,
        };
        let summarizer = MemorySummarizer::new(config);
        let entries: Vec<MemoryEntry> = (0..5)
            .map(|i| make_entry(&format!("id-{}", i), 0.5, &format!("entry {}", i)))
            .collect();

        match summarizer.summarize(&entries).await {
            SummarizedMemory::Full(e) => assert_eq!(e.len(), 5),
            SummarizedMemory::Compressed(_) => panic!("expected Full variant"),
        }
    }

    #[tokio::test]
    async fn test_above_threshold_produces_compressed() {
        let config = SummarizationConfig {
            max_entries_before_summary: 10,
            max_summary_chars: 512,
            use_llm_summarization: false,
        };
        let summarizer = MemorySummarizer::new(config);
        let entries: Vec<MemoryEntry> = (0..15)
            .map(|i| {
                make_entry(
                    &format!("id-{}", i),
                    0.5 - (i as f32) * 0.03,
                    &format!("entry {}", i),
                )
            })
            .collect();

        match summarizer.summarize(&entries).await {
            SummarizedMemory::Compressed(c) => {
                // keep_count = 10/2 = 5, plus 1 summary entry = 6
                assert_eq!(c.len(), 6, "expected 5 kept + 1 summary = 6 entries");
                assert!(
                    c.iter().any(|e| e.id.starts_with("summary-")),
                    "summary entry should be present"
                );
                // The summary entry should have usefulness 1.0
                let summary = c
                    .iter()
                    .find(|e| e.id.starts_with("summary-"))
                    .expect("summary entry should be present");
                assert_eq!(summary.usefulness, 1.0);
                assert_eq!(summary.class, "Summarized");
            }
            SummarizedMemory::Full(_) => panic!("expected Compressed variant"),
        }
    }

    #[test]
    fn test_should_summarize() {
        let config = SummarizationConfig {
            max_entries_before_summary: 5,
            ..Default::default()
        };
        let summarizer = MemorySummarizer::new(config);
        assert!(!summarizer.should_summarize(3));
        assert!(!summarizer.should_summarize(5));
        assert!(summarizer.should_summarize(6));
    }

    #[tokio::test]
    async fn test_summary_contains_snippets() {
        let config = SummarizationConfig {
            max_entries_before_summary: 4,
            max_summary_chars: 1024,
            use_llm_summarization: false,
        };
        let summarizer = MemorySummarizer::new(config);
        let entries: Vec<MemoryEntry> = (0..8)
            .map(|i| make_entry(&format!("id-{}", i), 0.5, &format!("unique-content-{}", i)))
            .collect();

        match summarizer.summarize(&entries).await {
            SummarizedMemory::Compressed(c) => {
                let summary = c
                    .iter()
                    .find(|e| e.id.starts_with("summary-"))
                    .expect("summary entry should be present");
                // The summary should mention discarded entries
                let keep_count = 4 / 2; // threshold / 2 = 2
                for i in 0..8 {
                    if i >= keep_count {
                        assert!(
                            summary.content.contains(&format!("unique-content-{}", i)),
                            "summary should mention entry {}",
                            i
                        );
                    }
                }
            }
            _ => panic!("expected Compressed"),
        }
    }
}
