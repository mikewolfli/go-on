//! In-memory HNSW index used by the SQLite vector-store backend as the search
//! fast path, plus the `spawn_blocking_vec!` macro shared by both backends.

#[cfg(not(feature = "backend-postgres"))]
use crate::shared::math::cosine_similarity_f32;
#[cfg(not(feature = "backend-postgres"))]
use fastrand;

/// Shared spawn_blocking wrapper for vector store async methods.
/// Eliminates the duplicated `spawn_blocking().await.map_err()` pattern.
macro_rules! spawn_blocking_vec {
    ($block:expr) => {
        spawn_blocking($block)
            .await
            .map_err(|e| anyhow::anyhow!("VectorStore blocking thread panicked: {e}"))?
    };
}
pub(crate) use spawn_blocking_vec;

/// HNSW node metadata
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug, Clone)]
pub(crate) struct HnswNodeMeta {
    pub(crate) memory_key: String,
    pub(crate) phase: String,
    pub(crate) response_text: String,
    pub(crate) updated_at: i64,
}

/// A (node index, distance) pair with ordering so that smaller distance sorts first.
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HnswNodeDist {
    pub(crate) idx: usize,
    pub(crate) dist: f32,
}

#[cfg(not(feature = "backend-postgres"))]
impl Eq for HnswNodeDist {}

#[cfg(not(feature = "backend-postgres"))]
impl PartialOrd for HnswNodeDist {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(not(feature = "backend-postgres"))]
impl Ord for HnswNodeDist {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.dist
            .partial_cmp(&other.dist)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Hierarchical Navigable Small World index for approximate nearest neighbor search.
///
/// Provides O(log N) search time for high-dimensional vectors.
/// Standard parameters: M=16, ef_construction=200, ef_search=50.
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug)]
pub(crate) struct HnswIndex {
    /// Stored vectors (index in this vec == node id)
    vectors: Vec<Vec<f32>>,
    /// Per-node metadata
    pub(crate) metadata: Vec<HnswNodeMeta>,
    /// Adjacency lists per layer: layers[layer][node_id] = Vec<neighbor_id>
    layers: Vec<Vec<Vec<usize>>>,
    /// Per-node random level (parallel to `vectors`/`metadata`), used to
    /// re-point `entry_point` when the current one is removed.
    node_levels: Vec<usize>,
    /// Current entry point (node id at the topmost layer)
    pub(crate) entry_point: Option<usize>,
    /// Highest layer that has any element
    max_level: usize,
    // ── HNSW parameters (constant after construction) ──
    /// Max number of connections per node on layer > 0
    m: usize,
    /// Max number of connections per node on layer 0
    m_max0: usize,
    /// Size of dynamic candidate list during construction
    ef_construction: usize,
    /// Size of dynamic candidate list during search
    pub(crate) ef_search: usize,
    /// Normalisation factor for level generation: mL = 1.0 / ln(M)
    m_l: f64,
}

#[cfg(not(feature = "backend-postgres"))]
impl HnswIndex {
    pub(crate) fn new(m: usize, ef_construction: usize, ef_search: usize) -> Self {
        let m_max0 = m * 2;
        let m_l = 1.0 / (m as f64).ln();
        Self {
            vectors: Vec::new(),
            metadata: Vec::new(),
            layers: vec![Vec::new()], // layer 0 exists and is empty
            node_levels: Vec::new(),
            entry_point: None,
            max_level: 0,
            m,
            m_max0,
            ef_construction,
            ef_search,
            m_l,
        }
    }

    fn random_level(&self) -> usize {
        let r: f64 = fastrand::f64(); // uniform in [0, 1)
        if r <= 0.0 {
            return 0;
        }
        (-r.ln() * self.m_l).floor() as usize
    }

    /// Distance between a query vector and a stored node.
    fn distance(&self, query: &[f32], node: usize) -> f32 {
        let v = &self.vectors[node];
        1.0 - cosine_similarity_f32(query, v)
    }

    /// Greedy search at a single layer, returning up to `ef` nearest neighbours.
    ///
    /// `entry` is the starting node id on this layer.
    fn search_layer(&self, query: &[f32], entry: usize, lc: usize, ef: usize) -> Vec<HnswNodeDist> {
        // Min-heap of candidates (closest first)
        let mut candidates: std::collections::BinaryHeap<std::cmp::Reverse<HnswNodeDist>> =
            std::collections::BinaryHeap::new();
        // Max-heap of results (furthest first — we track the worst distance)
        let mut results: std::collections::BinaryHeap<HnswNodeDist> =
            std::collections::BinaryHeap::new();

        let entry_dist = self.distance(query, entry);
        let entry_nd = HnswNodeDist {
            idx: entry,
            dist: entry_dist,
        };
        candidates.push(std::cmp::Reverse(entry_nd));
        results.push(entry_nd);

        let mut visited = std::collections::HashSet::new();
        visited.insert(entry);

        while let Some(std::cmp::Reverse(closest)) = candidates.pop() {
            // The furthest result is the top of the max-heap
            if let Some(furthest) = results.peek() {
                if closest.dist > furthest.dist {
                    break; // Cannot improve
                }
            }
            for &neighbor in &self.layers[lc][closest.idx] {
                if visited.contains(&neighbor) {
                    continue;
                }
                visited.insert(neighbor);
                let neighbor_dist = self.distance(query, neighbor);
                let furthest_dist = results.peek().map(|r| r.dist).unwrap_or(f32::MAX);
                if neighbor_dist < furthest_dist || results.len() < ef {
                    let nd = HnswNodeDist {
                        idx: neighbor,
                        dist: neighbor_dist,
                    };
                    candidates.push(std::cmp::Reverse(nd));
                    results.push(nd);
                    if results.len() > ef {
                        results.pop(); // Remove furthest
                    }
                }
            }
        }

        // Convert to sorted (closest-first) vec
        let mut sorted: Vec<HnswNodeDist> = results.into_sorted_vec();
        sorted.reverse(); // into_sorted_vec gives ascending; we want descending for .pop()
        sorted
    }

    /// Select the M closest neighbours from a candidate set (simple heuristic).
    fn select_neighbors_simple(
        &self,
        _q_idx: usize,
        candidates: &[HnswNodeDist],
        m: usize,
    ) -> Vec<HnswNodeDist> {
        let k = m.min(candidates.len());
        let mut sorted = candidates.to_vec();
        sorted.sort();
        sorted.truncate(k);
        sorted
    }

    /// Shrink connections for a node on a given layer, keeping only the M closest.
    fn shrink_connections(&mut self, node: usize, lc: usize, max_conn: usize) {
        let neighbors = &self.layers[lc][node];
        if neighbors.len() <= max_conn {
            return;
        }
        // Sort neighbors by distance to `node`
        let node_vec = &self.vectors[node];
        let mut dists: Vec<HnswNodeDist> = neighbors
            .iter()
            .map(|&n| HnswNodeDist {
                idx: n,
                dist: 1.0 - cosine_similarity_f32(node_vec, &self.vectors[n]),
            })
            .collect();
        dists.sort();
        dists.truncate(max_conn);
        self.layers[lc][node] = dists.into_iter().map(|nd| nd.idx).collect();
    }

    /// Insert a single vector with its metadata into the index.
    pub(crate) fn insert(&mut self, vector: Vec<f32>, meta: HnswNodeMeta) {
        let q_idx = self.vectors.len();
        let level = self.random_level();

        // Ensure layers exist up to `level`
        while self.layers.len() <= level {
            self.layers.push(Vec::new());
        }
        // Ensure each layer has adjacency entries for all existing nodes
        for lc in 0..self.layers.len() {
            while self.layers[lc].len() <= q_idx {
                self.layers[lc].push(Vec::new());
            }
        }

        self.vectors.push(vector.clone());
        self.metadata.push(meta);
        self.node_levels.push(level);

        if self.entry_point.is_none() {
            // First element
            self.entry_point = Some(q_idx);
            self.max_level = level;
            return;
        }

        let ep = self
            .entry_point
            .expect("HNSW entry_point must be set before insert");

        // Phase 1: traverse from top layer down to level+1 greedily (ef=1)
        let mut curr_ep = ep;
        for lc in (level + 1..=self.max_level).rev() {
            if lc < self.layers.len() && self.layers[lc].len() > curr_ep {
                let result = self.search_layer(&vector, curr_ep, lc, 1);
                if let Some(nearest) = result.first() {
                    curr_ep = nearest.idx;
                }
            }
        }

        // Phase 2: insert on each layer from min(level, max_level) down to 0
        let top = level.min(self.max_level);
        for lc in (0..=top).rev() {
            let candidates = self.search_layer(&vector, curr_ep, lc, self.ef_construction);
            let m_lc = if lc == 0 { self.m_max0 } else { self.m };
            let neighbors = self.select_neighbors_simple(q_idx, &candidates, m_lc);

            // Connect q → neighbors
            self.layers[lc][q_idx] = neighbors.iter().map(|nd| nd.idx).collect();

            // Connect neighbors → q (bidirectional)
            for nd in &neighbors {
                let n_idx = nd.idx;
                if lc < self.layers.len() && self.layers[lc].len() > n_idx {
                    self.layers[lc][n_idx].push(q_idx);
                    // Shrink if needed
                    let m_shrink = if lc == 0 { self.m_max0 } else { self.m };
                    self.shrink_connections(n_idx, lc, m_shrink);
                }
            }
        }

        // Update global entry point if the new element has a higher level
        if level > self.max_level {
            self.max_level = level;
            self.entry_point = Some(q_idx);
        }
    }

    /// Remove all nodes matching a memory_key from the index.
    ///
    /// Zeroes the vectors and clears metadata so they are filtered out during
    /// distance computations (`search` skips entries with empty memory_key).
    /// Removing ALL matches (not just the first) is required: `upsert` re-inserts
    /// a node per memory_key, and eviction can hit the same key twice in a row —
    /// leaving any match behind would let the search fast path return stale
    /// content or duplicate keys that the SQLite path would not return.
    pub(crate) fn remove(&mut self, memory_key: &str) {
        let mut removed_entry_point = false;
        for (pos, meta) in self.metadata.iter_mut().enumerate() {
            if meta.memory_key == memory_key {
                // Zero out the vector (distance will be ~1.0, effectively invisible)
                self.vectors[pos].fill(0.0);
                // Clear metadata so the node won't be matched again
                *meta = HnswNodeMeta {
                    memory_key: String::new(),
                    phase: String::new(),
                    response_text: String::new(),
                    updated_at: 0,
                };
                if self.entry_point == Some(pos) {
                    removed_entry_point = true;
                }
            }
        }
        // If the entry point itself was removed, point to the highest-level
        // remaining live node so search does not start from a dead (zeroed)
        // node. Searches still filter dead nodes, but starting from a live one
        // avoids navigating from a vector of zeros.
        if removed_entry_point {
            self.repair_entry_point();
        }
    }

    /// Re-point `entry_point`/`max_level` to the highest-level live node.
    ///
    /// If no live node remains, the index is empty and `entry_point` is cleared
    /// (search checks `vectors.is_empty()` and the valid set before use).
    fn repair_entry_point(&mut self) {
        let mut best: Option<(usize, usize)> = None; // (level, idx)
        for (idx, meta) in self.metadata.iter().enumerate() {
            if meta.memory_key.is_empty() {
                continue;
            }
            let level = self.node_levels.get(idx).copied().unwrap_or(0);
            if best.is_none_or(|(bl, _)| level > bl) {
                best = Some((level, idx));
            }
        }
        match best {
            Some((level, idx)) => {
                self.entry_point = Some(idx);
                self.max_level = level;
            }
            None => {
                self.entry_point = None;
                self.max_level = 0;
            }
        }
    }

    /// Search the index, returning up to `ef` nearest neighbours sorted by distance.
    ///
    /// Filters out removed entries (those with empty memory_key metadata).
    pub(crate) fn search(&self, query: &[f32], ef: usize) -> Vec<HnswNodeDist> {
        if self.vectors.is_empty() {
            return Vec::new();
        }
        // Build a set of valid (non-removed) node indices for post-filtering.
        let valid: std::collections::HashSet<usize> = self
            .metadata
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.memory_key.is_empty())
            .map(|(i, _)| i)
            .collect();
        if valid.is_empty() {
            return Vec::new();
        }
        let ep = self
            .entry_point
            .expect("HNSW entry_point must be set before search; check vectors.is_empty()");

        // Greedy search from top layer down to layer 1 (ef=1 per layer)
        let mut curr_ep = ep;
        for lc in (1..=self.max_level).rev() {
            if lc < self.layers.len() && self.layers[lc].len() > curr_ep {
                let result = self.search_layer(query, curr_ep, lc, 1);
                if let Some(nearest) = result.first() {
                    curr_ep = nearest.idx;
                }
            }
        }

        // Search layer 0 with ef
        let ef_actual = ef.max(self.ef_search);
        let results = self.search_layer(query, curr_ep, 0, ef_actual);

        // Filter out removed entries (empty memory_key)
        results
            .into_iter()
            .filter(|nd| valid.contains(&nd.idx))
            .collect()
    }
}
