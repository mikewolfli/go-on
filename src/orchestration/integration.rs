//! Integration Hub — Wires orphaned modules into the main execution path.
//! Called during system initialization from main.rs.

use crate::orchestration::cache_warming::CacheWarmingEngine;
use crate::orchestration::complexity_estimator::ComplexityEstimator;
use crate::orchestration::diagnostic_feedback::DiagnosticFeedbackEngine;
use crate::orchestration::session_context::SessionContextManager;
use crate::orchestration::tool_lock::ToolLockManager;
use crate::orchestration::tool_recommender::ToolRecommender;

/// Initializes all subsystems during application startup.
#[allow(dead_code)]
pub struct SystemIntegration {
    #[allow(dead_code)]
    pub session_context: SessionContextManager,
    #[allow(dead_code)]
    pub cache_warming: CacheWarmingEngine,
    #[allow(dead_code)]
    pub complexity_estimator: ComplexityEstimator,
    #[allow(dead_code)]
    pub diagnostic_feedback: DiagnosticFeedbackEngine,
    #[allow(dead_code)]
    pub tool_recommender: ToolRecommender,
    #[allow(dead_code)]
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
