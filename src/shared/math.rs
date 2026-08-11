//! Shared math utilities.

// The f32 cosine similarity variant is used by the token cache L2, semantic
// response cache, SQLite HNSW index, and skill semantic matching. In postgres
// mode, pgvector handles vector-store similarity natively via SQL.
// The allow(dead_code) is scoped to the macro-generated cosine_similarity_f32
// (the only item without a postgres caller) so future code in this file still
// gets dead-code checking.

macro_rules! define_cosine_similarity {
    ($name:ident, $float:ty) => {
        /// Compute cosine similarity between two equal-length vectors.
        ///
        /// Returns `0.0` if either vector is empty, lengths differ, or either
        /// vector has zero norm.
        // In postgres mode pgvector handles vector-store similarity natively,
        // so the f32 helper has no caller there; the allow is scoped to this
        // generated function only, keeping dead-code checking for the rest of
        // the file.
        #[cfg_attr(feature = "backend-postgres", allow(dead_code))]
        pub fn $name(a: &[$float], b: &[$float]) -> $float {
            if a.len() != b.len() || a.is_empty() {
                return 0.0;
            }
            let dot: $float = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
            let norm_a: $float = a.iter().map(|x| x * x).sum::<$float>().sqrt();
            let norm_b: $float = b.iter().map(|x| x * x).sum::<$float>().sqrt();
            if norm_a == 0.0 || norm_b == 0.0 {
                0.0
            } else {
                dot / (norm_a * norm_b)
            }
        }
    };
}

define_cosine_similarity!(cosine_similarity_f32, f32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_f32_identical() {
        let v = vec![1.0f32, 0.0];
        assert!((cosine_similarity_f32(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_f32_orthogonal() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!((cosine_similarity_f32(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_f32_empty() {
        assert_eq!(cosine_similarity_f32(&[], &[]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_f32_zero_norm() {
        let a = vec![0.0f32, 0.0];
        let b = vec![1.0f32, 0.0];
        assert_eq!(cosine_similarity_f32(&a, &b), 0.0);
    }
}
