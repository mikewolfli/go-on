//! Shared DetectionSeverity enum used across security sub-modules.
//!
//! Both `content_safety` and `prompt_injection` previously defined identical
//! private enums (`SafetySeverity` / `InjectionSeverity`).  This module
//! provides a single canonical type that each sub-module re-exports under its
//! own alias.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum DetectionSeverity {
    Low,
    Medium,
    High,
    Critical,
}
