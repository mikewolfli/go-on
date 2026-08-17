//! Vector storage and search
//!
//! Conditionally compiled:
//! - `backend-sqlite` (local, simple-server): rusqlite-backed, sync API
//! - `backend-postgres` (multi-users-server): postgres + pgvector-backed sync API
//!
//! # Backend-pair contract (debt #13 verdict: keep, do not trait-ify)
//!
//! The two backend impls mirror each other method-for-method with aligned
//! signatures (`upsert`, `search`, `get_phase_summary`, `upsert_phase_summary`,
//! `memory_entry_count`, `summary_entry_count`, `clear_all`). Semantics are
//! kept identical: eviction keeps the newest `max_entries` by `updated_at`;
//! scoring / recency blending / `min_similarity` match. Only SQL dialect
//! differs (`PARAM_PREFIX`, distance operator, DELETE/LIMIT shapes) and is
//! already confined via shared helpers (`embed_with_check`, `build_memory_key`,
//! `scored_to_hits`, `blend_similarity_with_recency`, `spawn_blocking_vec!`).
//! Trait-ifying adds no value under the mutually-exclusive feature gates
//! (compile_error! below) — the contract is enforced by convention: NEW
//! methods must be implemented in BOTH backends with identical semantics.

// Ensure features are mutually exclusive
#[cfg(all(feature = "backend-sqlite", feature = "backend-postgres"))]
compile_error!("features 'backend-sqlite' and 'backend-postgres' cannot be enabled simultaneously");

/// Parameter placeholder prefix for the active backend.
#[cfg(not(feature = "backend-postgres"))]
const PARAM_PREFIX: &str = "?";
#[cfg(feature = "backend-postgres")]
const PARAM_PREFIX: &str = "$";

/// Column list for `phase_summary` (shared between backends).
const PHASE_SUMMARY_COLUMNS: &str = "phase, summary_text, updated_at";

mod shared;
mod hnsw;
#[cfg(not(feature = "backend-postgres"))]
mod sqlite;
#[cfg(feature = "backend-postgres")]
mod postgres;

pub use shared::{VectorHit, VectorPrecisionFeedback};
#[cfg(not(feature = "backend-postgres"))]
pub use sqlite::VectorStore;
#[cfg(feature = "backend-postgres")]
pub use postgres::VectorStore;

// Test-only wiring: the sqlite test-suite (vector/tests.rs) is a sibling
// module of the backend halves and reaches these through `super::` — the
// imports exist solely so the moved-verbatim tests keep resolving (no
// production code depends on them).
#[cfg(all(test, not(feature = "backend-postgres")))]
use crate::acp::prelude::now_ts;
#[cfg(all(test, not(feature = "backend-postgres")))]
use crate::memory::embedding_provider::local_hash_embed;
#[cfg(all(test, not(feature = "backend-postgres")))]
pub(crate) use sqlite::{embedding_blob, SqliteVectorMode};

#[cfg(all(test, not(feature = "backend-postgres")))]
mod tests;
