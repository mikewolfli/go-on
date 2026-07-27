//! Simple in-memory vector index for approximate nearest neighbour search.
//!
//! Provides a lightweight `VectorIndex` that stores dense embeddings alongside
//! memory entries and supports cosine-similarity search.  The index is built
//! from persistence entries that already carry an `embedding` field, so no
//! external embedding service is required at search time.
//!
//! When no embeddings are available the index falls back gracefully (empty
//! results vs. panicking).

use crate::memory::memory_persistence::MemoryEntry;
use crate::shared::math::cosine_similarity;

// ── Flat (exact) vector index ──────────────────────────────────────────────

/// A simple in-memory vector index backed by a flat vector list with
/// exact cosine-similarity scoring.  Suitable for moderate-sized memory
/// stores (up to tens of thousands of entries).
#[derive(Debug)]
pub struct VectorIndex {
    vectors: Vec<IndexEntry>,
    dimension: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexEntry {
    id: String,
    vector: Vec<f64>,
    content: String,
}

impl VectorIndex {
    /// Create a new index with the given vector dimensionality.
    ///
    /// All inserted vectors **must** match this dimension (otherwise they
    /// are silently skipped and a warning is logged).
    pub fn new(dimension: usize) -> Self {
        Self {
            vectors: Vec::new(),
            dimension,
        }
    }

    /// Insert or replace a vector entry in the index.
    ///
    /// If an entry with the same `id` already exists, it is replaced.
    /// If the vector dimension does not match the index dimension, the
    /// insert is ignored (a warning is logged).
    pub fn insert(&mut self, id: String, vector: Vec<f64>, content: String) {
        if vector.len() != self.dimension {
            tracing::warn!(
                "VectorIndex::insert: vector dimension {} != index dimension {} (id={})",
                vector.len(),
                self.dimension,
                id
            );
            return;
        }
        // Replace if already present
        if let Some(pos) = self.vectors.iter().position(|e| e.id == id) {
            self.vectors[pos] = IndexEntry {
                id,
                vector,
                content,
            };
            return;
        }
        self.vectors.push(IndexEntry {
            id,
            vector,
            content,
        });
    }

    /// Remove a vector entry from the index by id.
    pub fn remove(&mut self, id: &str) {
        self.vectors.retain(|e| e.id != id);
    }

    /// Return the top-`k` results sorted by cosine similarity (descending).
    ///
    /// Returns `Vec<(id, similarity, content)>` where similarity is in `[-1.0, 1.0]`.
    /// Returns an empty vec when the index is empty or the query vector has the
    /// wrong dimension.
    pub fn search(&self, query: &[f64], k: usize) -> Vec<(String, f64, String)> {
        if self.vectors.is_empty() || query.len() != self.dimension {
            return Vec::new();
        }
        let mut results: Vec<_> = self
            .vectors
            .iter()
            .map(|entry| {
                let sim = cosine_similarity(query, &entry.vector);
                (entry.id.clone(), sim, entry.content.clone())
            })
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
    }

    /// Build a vector index from a slice of persistence entries that have
    /// pre-computed embeddings.  Entries without an embedding are skipped.
    ///
    /// This is the primary way to construct an index from the existing
    /// warm/cold store contents.
    pub fn from_entries(entries: &[MemoryEntry], dimension: usize) -> Self {
        let mut index = Self::new(dimension);
        for entry in entries {
            if let Some(ref embedding) = entry.embedding {
                if embedding.len() == dimension {
                    let vector: Vec<f64> = embedding.iter().map(|v| *v as f64).collect();
                    index.insert(entry.id.clone(), vector, entry.content.clone());
                }
            }
        }
        index
    }

    /// Number of vectors currently indexed.
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Returns `true` when the index contains no vectors.
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// The configured vector dimensionality.
    pub fn dimension(&self) -> usize {
        self.dimension
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, usefulness: f32, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            tier: crate::memory::memory_persistence::MemoryTier::Hot,
            class: "Test".to_string(),
            content: content.to_string(),
            created_at: 1000,
            accessed_at: 1000,
            usefulness,
            embedding: None,
            access_count: 1,
            session_id: None,
            user_id: None,
        }
    }

    #[test]
    fn test_insert_and_search() {
        let mut idx = VectorIndex::new(3);
        idx.insert("a".into(), vec![1.0, 0.0, 0.0], "alpha".into());
        idx.insert("b".into(), vec![0.0, 1.0, 0.0], "beta".into());
        idx.insert("c".into(), vec![0.0, 0.0, 1.0], "gamma".into());

        let results = idx.search(&[0.9, 0.1, 0.0], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "a"); // closest to [0.9, 0.1, 0.0]
        assert_eq!(results[1].0, "b");
    }

    #[test]
    fn test_search_returns_top_k() {
        let mut idx = VectorIndex::new(2);
        idx.insert("x".into(), vec![1.0, 0.0], "x".into());
        idx.insert("y".into(), vec![0.0, 1.0], "y".into());
        idx.insert("z".into(), vec![0.5, 0.5], "z".into());

        let results = idx.search(&[1.0, 0.0], 5);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, "x");
    }

    #[test]
    fn test_search_empty_index() {
        let idx: VectorIndex = VectorIndex::new(4);
        assert!(idx.search(&[1.0, 0.0, 0.0, 0.0], 5).is_empty());
    }

    #[test]
    fn test_insert_wrong_dimension_skipped() {
        let mut idx = VectorIndex::new(3);
        idx.insert("bad".into(), vec![1.0, 0.0], "bad".into());
        assert!(idx.is_empty());
    }

    #[test]
    fn test_remove() {
        let mut idx = VectorIndex::new(2);
        idx.insert("keep".into(), vec![1.0, 0.0], "keep".into());
        idx.insert("gone".into(), vec![0.0, 1.0], "gone".into());
        idx.remove("gone");
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.search(&[1.0, 0.0], 5).len(), 1);
    }

    #[test]
    fn test_from_entries_only_embeddings() {
        let mut entries = Vec::new();
        for i in 0..5 {
            let mut e = make_entry(&format!("e-{}", i), 0.5, &format!("content {}", i));
            if i % 2 == 0 {
                // Only even entries get an embedding
                e.embedding = Some(vec![(i as f32) * 0.1, (i as f32) * 0.2]);
            }
            entries.push(e);
        }

        let idx = VectorIndex::from_entries(&entries, 2);
        assert_eq!(idx.len(), 3); // 0, 2, 4
    }

    #[test]
    fn test_from_entries_skips_wrong_dimension() {
        let mut entry = make_entry("bad", 0.5, "bad dim");
        entry.embedding = Some(vec![1.0, 2.0, 3.0]); // 3D, but index is 2D
        let idx = VectorIndex::from_entries(&[entry], 2);
        assert!(idx.is_empty());
    }
}
