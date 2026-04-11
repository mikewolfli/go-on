//! Promotion extension interface.
//!
//! This module intentionally keeps only stable extension points for future
//! memory-promotion pipelines. Main-chain promotion currently uses
//! `MemoryStore::promote()` directly; plugin wiring can be added behind this
//! interface without changing call sites again.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PromotionStage {
    Raw,
    Curated,
    Indexed,
    ProjectState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionItem {
    pub id: String,
    pub content: String,
    pub confidence: f32,
    pub stage: PromotionStage,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromotionContext {
    pub task: Option<String>,
    pub phase: Option<String>,
    pub executor: Option<String>,
}

pub trait PromotionPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn promote(&self, item: &PromotionItem, context: &PromotionContext) -> Option<PromotionItem>;
}

/// No-op default implementation used as a stable fallback extension point.
pub struct NoopPromotionPlugin;

impl PromotionPlugin for NoopPromotionPlugin {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn promote(&self, item: &PromotionItem, _context: &PromotionContext) -> Option<PromotionItem> {
        Some(item.clone())
    }
}
