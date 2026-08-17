//! Types and helpers shared by both vector-store backends.
//!
//! The SQLite and PostgreSQL halves of `crate::memory::vector` are
//! cfg-disjoint (mutually exclusive feature gates), but they share the hit
//! types, the embedding helper, and the scoring pipeline verbatim — those
//! live here so neither backend can drift from the other.

use crate::memory::embedding_provider::{
    local_hash_embed, ConfigurableEmbeddingProvider, EmbeddingProvider,
};
use anyhow::Result;

/// Vector search hit
#[derive(Debug, Clone)]
pub struct VectorHit {
    /// Response snippet
    pub response_snippet: String,
    /// Similarity score (0.0-1.0)
    pub similarity: f32,
}

/// Precision feedback from a vector search: average similarity of returned hits.
/// Used by autotune to adjust min_query_chars and other parameters.
#[derive(Debug, Clone, Copy)]
pub struct VectorPrecisionFeedback {
    /// Average similarity of returned hits (0.0-1.0).
    pub avg_similarity: f32,
    /// Number of hits returned.
    pub hit_count: usize,
}

impl VectorPrecisionFeedback {
    pub fn new(hits: &[VectorHit]) -> Self {
        if hits.is_empty() {
            return Self {
                avg_similarity: 0.0,
                hit_count: 0,
            };
        }
        let sum: f32 = hits.iter().map(|h| h.similarity).sum();
        let avg = sum / hits.len() as f32;
        Self {
            avg_similarity: avg,
            hit_count: hits.len(),
        }
    }
}

pub(crate) fn blend_similarity_with_recency(similarity: f32, now: i64, updated_at: i64) -> f32 {
    // Recency blending: similarity carries 70% weight, recency 30%. The
    // decay factor 0.05 halves the recency term after ~20 days of age.
    const DECAY_FACTOR: f64 = 0.05;
    let age_secs = (now - updated_at).max(0) as f64;
    let age_days = age_secs / 86_400.0;
    let recency_weight = (1.0 / (1.0 + age_days * DECAY_FACTOR)) as f32;
    similarity * 0.70 + recency_weight * 0.30
}

/// Embed text using the canonical minhash implementation (avoids code duplication).
fn embed_text(text: &str, dimensions: usize) -> Vec<f32> {
    local_hash_embed(text, dimensions)
}

/// Shared embedding helper: dispatches to the configured provider or the minhash
/// fallback, and validates that the returned vector has the expected dimension.
///
/// Used by both the SQLite and PostgreSQL backends to eliminate the identical
/// 12-line dimension-checking pattern that was duplicated across every method.
pub(crate) fn embed_with_check(
    query: &str,
    dimensions: usize,
    provider: &Option<ConfigurableEmbeddingProvider>,
) -> Result<Vec<f32>> {
    if let Some(ref provider) = provider {
        let vec = provider.embed(query);
        if vec.len() != dimensions {
            anyhow::bail!(
                "Embedding dimension mismatch: got {} but store expects {} dimensions",
                vec.len(),
                dimensions,
            );
        }
        // Zero-vector guard: the OpenAI provider returns `vec![0.0; dims]` to
        // signal an API failure, and the Ollama/Qwen3 zero-signal path is
        // reachable when `fallback_to_hash` is disabled. A zero vector has
        // cosine similarity NaN against every other vector (0/0), silently
        // polluting semantic matching — reject it instead of storing/searching
        // with it. (Dimensions are always > 0 in production, so an all-zero
        // vector here is a failure signal, not a degenerate-but-legit embed.)
        if vec.iter().all(|v| *v == 0.0) {
            anyhow::bail!(
                "Embedding provider returned an all-zero vector ({} dims) — treating it as an embedding failure; refusing to store/search",
                vec.len(),
            );
        }
        Ok(vec)
    } else {
        Ok(embed_text(query, dimensions))
    }
}

fn trim_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

/// Convert `(memory_key, blended_score, response_text)` tuples into sorted,
/// truncated hits with precision feedback.
pub(crate) fn scored_to_hits(
    mut scored: Vec<(String, f32, String)>,
    top_k: usize,
    max_snippet_chars: usize,
) -> (Vec<VectorHit>, VectorPrecisionFeedback) {
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    let hits: Vec<VectorHit> = scored
        .into_iter()
        .map(|(_, blended_score, text)| VectorHit {
            similarity: blended_score,
            response_snippet: trim_chars(&text, max_snippet_chars),
        })
        .collect();
    let feedback = VectorPrecisionFeedback::new(&hits);
    (hits, feedback)
}

pub(crate) fn build_memory_key(phase: &str, query_text: &str) -> String {
    let payload = format!("{}|{}", phase, query_text.trim());
    crate::shared::sha256_hex(payload.as_bytes())
}
