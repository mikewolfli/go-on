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

pub mod cache;
pub mod memory;
pub mod memory_response_cache;
pub mod semantic_cache;
pub mod vector;
