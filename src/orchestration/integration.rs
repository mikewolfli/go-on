//! Integration Hub — reserved for future wiring.
//! Gated behind `sub-bus-tool-future` feature.
#![cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code, unused_imports))]

use crate::orchestration::cache_warming::CacheWarmingEngine;
use crate::orchestration::complexity_estimator::ComplexityEstimator;
use crate::orchestration::diagnostic_feedback::DiagnosticFeedbackEngine;
use crate::orchestration::session_context::SessionContextManager;
use crate::orchestration::tool_lock::ToolLockManager;
use crate::orchestration::tool_recommender::ToolRecommender;

/// Initializes all subsystems during application startup.
pub struct SystemIntegration {
    pub session_context: SessionContextManager,
    pub cache_warming: CacheWarmingEngine,
    pub complexity_estimator: ComplexityEstimator,
    pub diagnostic_feedback: DiagnosticFeedbackEngine,
    pub tool_recommender: ToolRecommender,
    pub tool_lock: ToolLockManager,
}

impl SystemIntegration {
    pub fn new() -> Self {
        Self {
            session_context: SessionContextManager::default(),
            cache_warming: CacheWarmingEngine::default(),
            complexity_estimator: ComplexityEstimator::new(),
            diagnostic_feedback: DiagnosticFeedbackEngine::new(),
            tool_recommender: ToolRecommender::new(),
            tool_lock: ToolLockManager::new(),
        }
    }
}

impl Default for SystemIntegration {
    fn default() -> Self {
        Self::new()
    }
}
