//! Shared math utilities.

// The f32 cosine similarity variant is only used by the SQLite HNSW index.
// In postgres mode, pgvector handles similarity natively via SQL.
// Allow dead_code here to prevent warnings when only backend-postgres is enabled.
#![cfg_attr(feature = "backend-postgres", allow(dead_code))]

macro_rules! define_cosine_similarity {
    ($name:ident, $float:ty) => {
        #[allow(dead_code)]
        /// Compute cosine similarity between two equal-length vectors.
        ///
        /// Returns `0.0` if either vector is empty, lengths differ, or either
        /// vector has zero norm.
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

define_cosine_similarity!(cosine_similarity, f64);
define_cosine_similarity!(cosine_similarity_f32, f32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_f64_identical() {
        let v = vec![1.0, 0.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn test_cosine_similarity_f64_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_similarity(&a, &b)).abs() < 1e-12);
    }

    #[test]
    fn test_cosine_similarity_f64_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_f64_mismatched_length() {
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }

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
