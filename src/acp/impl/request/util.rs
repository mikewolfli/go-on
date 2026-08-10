//! Utility functions for ACP request handling.
//!
//! Extracted from `request.rs` to reduce the size of the main module.
//! Contains small helper functions used by various sub-modules.

use crate::i18n::runtime::tf;
use crate::memory::vector::VectorStore;
use std::sync::Arc;

/// Collect relevant context snippets from the vector store by searching
/// across multiple phases (execution phase and semantic phase).
///
/// The per-phase searches are independent, so they run concurrently via
/// `join_all` (results are processed in original phase order, preserving the
/// dedup + max_snippets semantics of the former serial loop).
pub(super) async fn collect_vector_context_snippets(
    store: Arc<VectorStore>,
    search_phases: &[String],
    subtask_description: &str,
    max_snippets: usize,
) -> Vec<String> {
    // Fire all phase searches concurrently; join_all preserves input order, so
    // the dedup/cap pass below sees phases in their original sequence.
    let phase_results = futures_util::future::join_all(search_phases.iter().map(|phase| {
        let store = Arc::clone(&store);
        async move {
            store
                .search(phase, subtask_description, max_snippets, 0.25, 512)
                .await
        }
    }))
    .await;

    let mut snippets: Vec<String> = Vec::new();
    for result in phase_results {
        if let Ok((hits, _)) = result {
            for hit in hits {
                let snippet = hit.response_snippet.trim();
                if snippet.is_empty() {
                    continue;
                }
                if !snippets.iter().any(|existing| existing == snippet) {
                    snippets.push(snippet.to_string());
                }
                if snippets.len() >= max_snippets {
                    break;
                }
            }
        }
        if snippets.len() >= max_snippets {
            break;
        }
    }
    snippets
}

/// Generate a compact, human-readable session identifier from a task description.
pub(super) fn session_id_for_task(task: &str) -> String {
    let compact = task
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(24)
        .collect::<String>();
    let id = if compact.is_empty() {
        "session"
    } else {
        compact.as_str()
    };
    tf("info.request.session_id_format", &[("id", id)])
}
