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

// NOTE: MemoryRetrievalEngine (GAP-B52-13) is implemented and ready for
// integration. It should be injected into AcpServer via ServerBuilder once
// the server startup path is updated to pass the persistence layer.
// Calling code example:
//   let engine = MemoryRetrievalEngine::new(persistence);
//           let router = MemoryRetrievalRouter::new(engine);
//           builder = builder.with_memory_retrieval(Arc::new(router));

/// Create a `MemoryRetrievalEngine` wired for server injection.
///
/// Called from `ServerBuilder::build()` when a `memory_persistence` is
/// configured but no explicit `memory_retrieval_engine` has been set.
/// Build and wire a MemoryRetrievalEngine from an existing MemoryPersistence.
/// TODO-BLUE64: Wire into production server builder path.
#[allow(dead_code)]
pub fn wire_memory_retrieval(
    persistence: crate::memory::memory_persistence::MemoryPersistence,
) -> crate::memory::memory_retrieval::MemoryRetrievalEngine {
    crate::memory::memory_retrieval::MemoryRetrievalEngine::new(persistence)
}
pub mod vector;
