//! Simple in-memory vector index for approximate nearest neighbour search.
//!
//! Provides a lightweight `VectorIndex` that stores dense embeddings alongside
//! memory entries and supports cosine-similarity search.  The index is built
//! from persistence entries that already carry an `embedding` field, so no
//! external embedding service is required at search time.
//!
//! When no embeddings are available the index falls back gracefully (empty
//! results vs. panicking).
//!
//! For large N (>500), a `ClusterIndex` wraps the flat index and uses
//! hierarchical k-means–like clustering to reduce search from O(N·D) to
//! approximately O(sqrt(N)·D) by pruning distant clusters.

use std::collections::HashMap;

use crate::memory::memory_persistence::MemoryEntry;

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

// ── Cluster-based approximate nearest neighbour index ───────────────────────
//
// For large N, we partition vectors into K = sqrt(N) clusters via a simple
// k-means–like assignment.  Search prunes to the top-M closest centroids
// (M = K/2) before scoring individual members, yielding ~O(sqrt(N)·D)
// average search cost.

/// A single cluster: centroid + member entries.
#[derive(Debug)]
#[allow(dead_code)] // F-GAP-49 — reserved for cluster-based indexing
struct Cluster {
    centroid: Vec<f64>,
    members: Vec<(String, Vec<f64>, String)>,
}

/// Approximate nearest-neighbour index that wraps a flat `VectorIndex` and
/// groups vectors into clusters for faster search.
///
/// * For small N (≤`flat_threshold`) search falls through to exact flat search.
/// * For large N, only the closest clusters are examined.
#[derive(Debug)]
#[allow(dead_code)] // F-GAP-49 — reserved for cluster index
pub struct ClusterIndex {
    flat: VectorIndex,
    clusters: Vec<Cluster>,
    /// Number of vectors above which clustering is used.
    flat_threshold: usize,
    /// Number of clusters to build (target: sqrt(N)).
    num_clusters: usize,
}

#[allow(dead_code)] // F-GAP-49 — reserved for cluster index impl
impl ClusterIndex {
    /// Create a new `ClusterIndex` wrapping the given flat index.
    ///
    /// `flat_threshold` controls when approximated search kicks in
    /// (default: 500).  When the flat index has fewer entries than this
    /// threshold, exact search is used.
    pub fn new(flat: VectorIndex, flat_threshold: usize) -> Self {
        let n = flat.len();
        let num_clusters = if n > flat_threshold {
            (n as f64).sqrt().ceil() as usize
        } else {
            1
        };
        let mut idx = Self {
            flat,
            clusters: Vec::new(),
            flat_threshold,
            num_clusters,
        };
        idx.recluster();
        idx
    }

    /// Rebuild clusters from the current flat index contents.
    #[allow(clippy::needless_range_loop)]
    pub fn recluster(&mut self) {
        self.clusters.clear();
        let entries = &self.flat.vectors;
        if entries.is_empty() {
            return;
        }
        let dim = self.flat.dimension();
        let n = entries.len();

        // Determine number of clusters (≈ sqrt(N), at least 1, at most N).
        let k = if n > self.flat_threshold {
            (n as f64).sqrt().ceil() as usize
        } else {
            1
        };
        let k = k.clamp(1, n);
        self.num_clusters = k;

        // Initialise centroids: pick k distinct entries by spreading evenly.
        let step = n / k;
        let mut centroids: Vec<Vec<f64>> = Vec::with_capacity(k);
        for i in 0..k {
            let idx = (i * step).min(n - 1);
            centroids.push(entries[idx].vector.clone());
        }

        // Run a few iterations of k-means assignment (max 10 iterations).
        let mut assignments: Vec<usize> = vec![0; n];
        for _iter in 0..10 {
            // Assign each point to the nearest centroid.
            let mut changed = false;
            for (i, entry) in entries.iter().enumerate() {
                let mut best = 0;
                let mut best_sim = f64::NEG_INFINITY;
                for (c_idx, centroid) in centroids.iter().enumerate() {
                    let sim = cosine_similarity(&entry.vector, centroid);
                    if sim > best_sim {
                        best_sim = sim;
                        best = c_idx;
                    }
                }
                if assignments[i] != best {
                    assignments[i] = best;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
            // Recompute centroids (mean of assigned vectors).
            let mut sums: Vec<Vec<f64>> = vec![vec![0.0; dim]; k];
            let mut counts: Vec<usize> = vec![0; k];
            for (i, entry) in entries.iter().enumerate() {
                let c = assignments[i];
                for d in 0..dim {
                    sums[c][d] += entry.vector[d];
                }
                counts[c] += 1;
            }
            for c in 0..k {
                if counts[c] > 0 {
                    for d in 0..dim {
                        centroids[c][d] = sums[c][d] / counts[c] as f64;
                    }
                }
            }
        }

        // Build clusters from final assignments.
        let mut cluster_map: HashMap<usize, Vec<(String, Vec<f64>, String)>> = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            cluster_map.entry(assignments[i]).or_default().push((
                entry.id.clone(),
                entry.vector.clone(),
                entry.content.clone(),
            ));
        }
        self.clusters = centroids
            .into_iter()
            .enumerate()
            .map(|(c_idx, centroid)| Cluster {
                centroid,
                members: cluster_map.remove(&c_idx).unwrap_or_default(),
            })
            .collect();
    }

    /// Search the index for the top-`k` results.
    ///
    /// For small N (≤ `flat_threshold`) this delegates to exact flat search.
    /// For large N, it scores centroids first, prunes to the top M clusters
    /// (M = max(K/2, 1)), then scores individual members within those clusters.
    pub fn search(&self, query: &[f64], k: usize) -> Vec<(String, f64, String)> {
        let n = self.flat.len();
        if n == 0 || query.len() != self.flat.dimension() {
            return Vec::new();
        }

        // Small N: use exact flat search.
        if n <= self.flat_threshold || self.clusters.is_empty() {
            return self.flat.search(query, k);
        }

        // Score centroids and pick the top M clusters.
        let mut centroid_scores: Vec<(usize, f64)> = self
            .clusters
            .iter()
            .enumerate()
            .map(|(i, c)| (i, cosine_similarity(query, &c.centroid)))
            .collect();
        centroid_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let m = (self.num_clusters / 2).max(1);
        let top_m: Vec<usize> = centroid_scores
            .into_iter()
            .take(m)
            .map(|(i, _)| i)
            .collect();

        // Score individual members in the selected clusters.
        let mut results: Vec<_> = top_m
            .iter()
            .flat_map(|&c_idx| {
                let cluster = &self.clusters[c_idx];
                cluster.members.iter().map(|(id, vec, content)| {
                    let sim = cosine_similarity(query, vec);
                    (id.clone(), sim, content.clone())
                })
            })
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
    }

    /// Number of vectors indexed.
    pub fn len(&self) -> usize {
        self.flat.len()
    }

    /// Returns `true` when the index contains no vectors.
    pub fn is_empty(&self) -> bool {
        self.flat.is_empty()
    }

    /// The configured vector dimensionality.
    pub fn dimension(&self) -> usize {
        self.flat.dimension()
    }

    /// Access the underlying flat index for direct manipulation.
    pub fn flat_mut(&mut self) -> &mut VectorIndex {
        &mut self.flat
    }

    /// Access the underlying flat index (read-only).
    #[allow(dead_code)] // F-GAP-49 — reserved for flat vector search
    pub fn flat(&self) -> &VectorIndex {
        &self.flat
    }
}

// ── Cosine similarity ──────────────────────────────────────────────────────

/// Compute cosine similarity between two equal-length vectors.
///
/// Returns `0.0` if either vector is zero-length or zero-norm.
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
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

    // ── ClusterIndex tests ────────────────────────────────────────────────

    #[test]
    fn test_cluster_index_small_n_falls_back_to_flat() {
        let mut flat = VectorIndex::new(2);
        flat.insert("a".into(), vec![1.0, 0.0], "alpha".into());
        flat.insert("b".into(), vec![0.0, 1.0], "beta".into());

        // flat_threshold=500 means N=2 < 500 → exact search
        let ci = ClusterIndex::new(flat, 500);
        let results = ci.search(&[0.9, 0.1], 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "a");
    }

    #[test]
    fn test_cluster_index_large_n_approximate() {
        let mut flat = VectorIndex::new(2);
        // Insert 600 vectors in two well-separated groups.
        // Group 0: oriented near (1, 0)
        for i in 0..300 {
            let angle = (i as f64) * 0.001;
            flat.insert(
                format!("g0-{}", i),
                vec![angle.cos(), angle.sin()],
                format!("group0-{}", i),
            );
        }
        // Group 1: oriented near (0, 1)
        for i in 0..300 {
            let angle = std::f64::consts::FRAC_PI_2 + (i as f64) * 0.001;
            flat.insert(
                format!("g1-{}", i),
                vec![angle.cos(), angle.sin()],
                format!("group1-{}", i),
            );
        }

        // flat_threshold=100 means N=600 > 100 → use clustering
        let ci = ClusterIndex::new(flat, 100);
        assert!(ci.len() == 600);

        // Query near (1, 0) — top result should be from group 0
        let results = ci.search(&[1.0, 0.0], 3);
        assert_eq!(results.len(), 3);
        for (id, _, _) in &results {
            assert!(id.starts_with("g0"), "expected g0-* but got {id}");
        }
    }

    #[test]
    fn test_cluster_index_empty() {
        let flat = VectorIndex::new(3);
        let ci = ClusterIndex::new(flat, 100);
        assert!(ci.is_empty());
        assert!(ci.search(&[1.0, 0.0, 0.0], 5).is_empty());
    }

    #[test]
    fn test_cluster_index_recluster() {
        let mut flat = VectorIndex::new(2);
        flat.insert("a".into(), vec![1.0, 0.0], "a".into());
        let mut ci = ClusterIndex::new(flat, 100);

        // Add more vectors through the flat index and recluster
        ci.flat_mut().insert("b".into(), vec![0.0, 1.0], "b".into());
        ci.flat_mut()
            .insert("c".into(), vec![-1.0, 0.0], "c".into());
        ci.recluster();

        assert_eq!(ci.len(), 3);
    }
}
