//! Shared role types used across core and orchestration layers.
//!
//! This module exists to break the circular dependency between
//! `core::config::types` (which needs `RoleDefinition` for config deserialization)
//! and `orchestration::roles` (which defines `RoleDefinition`).
//!
//! Both layers import from this shared location instead of from each other.

use serde::{Deserialize, Serialize};

/// Full definition for a custom agent role
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub industry: String,
    /// Keywords used by rank_execution_agents; mirrors the built-in role keyword lists
    pub keywords: Vec<String>,
    /// Allowed tool names for this role
    pub allowed_tools: Vec<String>,
    /// Max tool calls per turn
    pub max_tool_calls: usize,
    /// Token budget per turn
    pub token_budget: usize,
    /// Timeout in seconds
    pub timeout_seconds: u64,
}
