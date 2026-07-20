//! Memory management modules for caching, vector storage, and response caching.
//!
//! This module contains components responsible for managing various types of
//! memory and caching in the ACP proxy system, including:
//!
//! - **Cache**: General-purpose caching mechanisms
//! - **Memory**: Core memory management and persistence
//! - **Memory Response Cache**: Specialized caching for AI responses
//! - **Vector**: Vector storage and similarity search operations

#![allow(clippy::module_inception)]

use std::sync::Arc;

pub mod agent_memory_bus;
pub mod cache;
pub mod embedding_provider;
pub mod memory;
pub mod memory_bridge;
pub mod memory_persistence;
pub mod memory_response_cache;
pub mod memory_retrieval;
pub mod semantic_cache;
pub mod summarization;
pub mod vector_index;

/// Create a `MemoryRetrievalEngine` wired for server injection.
///
/// Called from `ServerBuilder::build()` / `wire_server()` when a
/// `MemoryPersistence` is configured but no explicit
/// `memory_retrieval_engine` has been set via the builder.
/// Build and wire a MemoryRetrievalEngine from an existing MemoryPersistence.
///
/// S5 optimization: wraps persistence in Arc so the same SQLite warm store
/// connection is shared with the server's main MemoryPersistence, avoiding
/// a redundant second connection + DDL overhead (~30-50ms).
pub fn wire_memory_retrieval(
    persistence: Arc<crate::memory::memory_persistence::MemoryPersistence>,
) -> crate::memory::memory_retrieval::MemoryRetrievalEngine {
    crate::memory::memory_retrieval::MemoryRetrievalEngine::new(persistence)
}
pub mod vector;
