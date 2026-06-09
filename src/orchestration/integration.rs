//! Integration Hub — reserved for future wiring.
//! Gated behind `sub-bus-tool-future` feature.
// F-GAP-51: dead_code allowed on items when sub-bus-tool-future is disabled

#[cfg(feature = "sub-bus-tool-future")]
use crate::orchestration::cache_warming::CacheWarmingEngine;
#[cfg(feature = "sub-bus-tool-future")]
use crate::orchestration::complexity_estimator::ComplexityEstimator;
#[cfg(feature = "sub-bus-tool-future")]
use crate::orchestration::diagnostic_feedback::DiagnosticFeedbackEngine;
#[cfg(feature = "sub-bus-tool-future")]
use crate::orchestration::session_context::SessionContextManager;
#[cfg(feature = "sub-bus-tool-future")]
use crate::orchestration::tool_lock::ToolLockManager;
#[cfg(feature = "sub-bus-tool-future")]
use crate::orchestration::tool_recommender::ToolRecommender;

/// Initializes all subsystems during application startup.
#[cfg(feature = "sub-bus-tool-future")]
#[allow(dead_code)] // Reserved — wired via SystemIntegration init
pub struct SystemIntegration {
    pub session_context: SessionContextManager,
    pub cache_warming: CacheWarmingEngine,
    pub complexity_estimator: ComplexityEstimator,
    pub diagnostic_feedback: DiagnosticFeedbackEngine,
    pub tool_recommender: ToolRecommender,
    pub tool_lock: ToolLockManager,
}

#[cfg(feature = "sub-bus-tool-future")]
#[allow(dead_code)] // F-GAP-49 — reserved for future tool-bus integration
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

#[cfg(feature = "sub-bus-tool-future")]
impl Default for SystemIntegration {
    fn default() -> Self {
        Self::new()
    }
}
