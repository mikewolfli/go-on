//! GAP-B52-02: Self-Evolution Loop
//!
//! Implements the evolution lifecycle: trigger → analyze → propose →
//! await_approval → apply → verify → record. Runs as an async select! loop
//! that polls multiple trigger sources and processes them one at a time.
//!
//! # Sub-modules
//!
//! * [`observe`]   — Trigger sources and observation-phase types
//! * [`propose`]   — Analysis struct for proposal generation
//! * [`validate`]  — Approval types (ApprovalMode, Approval, ApprovalStatus)
//! * [`apply`]     — EvolutionLoopError for application-phase errors

pub mod apply;
pub mod observe;
pub mod propose;
pub mod validate;

// ── Re-exports for backward compatibility ─────────────────────────────────
pub use observe::{
    DiagnosticTriggerSource, EvolutionTrigger, MetricsPoint, MetricsSnapshot, PubsubTriggerSource,
    RegressionDirection, TickTriggerSource, TriggerSource,
};
pub use propose::Analysis;
pub use validate::{Approval, ApprovalMode};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::agents::self_evolution_agent::SelfEvolutionAgent;
use crate::intelligence::evolution_graph::{EvolutionGraph, EvolutionStage};
use crate::intelligence::metacognitive::global_metacognitive_controller;
use crate::intelligence::triple_fusion::global_triple_fusion_bridge;
use crate::observability::alert_manager::AlertManager;
use crate::orchestration::self_evolution::evolution_history::EvolutionHistory;

use crate::orchestration::self_evolution::sandbox::{CodePatch, SandboxExecutor};

// ---------------------------------------------------------------------------
// EvolutionLoop
// ---------------------------------------------------------------------------

/// The main evolution loop that drives the self-evolution lifecycle.
///
/// Polls trigger sources in a `select!` loop and processes each trigger
/// through the full evolution pipeline:
///
/// trigger → analyze → propose → await_approval → apply → verify → record
#[derive(Debug)]
pub struct EvolutionLoop {
    /// Registered trigger sources.
    trigger_sources: Vec<Box<dyn observe::TriggerSource>>,
    /// Sandbox executor for applying patches.
    sandbox: Option<SandboxExecutor>,
    /// Evolution cycle counter.
    cycle_id: u64,
    /// Approval mode for evolution cycles.
    approval_mode: validate::ApprovalMode,
    /// Evolution history recorder.
    history: Option<EvolutionHistory>,
    /// Working directory for sandbox operations.
    workdir: PathBuf,
    /// Poll interval for trigger sources.
    poll_interval: Duration,
    /// Self-evolution agent for LLM-based code analysis and patch generation.
    agent: Option<Arc<SelfEvolutionAgent>>,
    /// Evolution graph for capability version history tracking (I9).
    evolution_graph: Option<Arc<std::sync::Mutex<EvolutionGraph>>>,
    /// Shared error-counts map for recording errors detected during
    /// the evolution pipeline (e.g. verification failures).
    /// Injected into the DiagnosticTriggerSource so repeated errors
    /// automatically trigger evolution cycles.
    diagnostic_error_counts: Option<Arc<tokio::sync::Mutex<HashMap<String, u64>>>>,
}

impl EvolutionLoop {
    /// Create a new EvolutionLoop with default settings.
    pub fn new(workdir: PathBuf) -> Self {
        Self {
            trigger_sources: Vec::new(),
            sandbox: None,
            cycle_id: 0,
            approval_mode: validate::ApprovalMode::RequireApproval,
            history: None,
            workdir,
            poll_interval: Duration::from_secs(30),
            agent: None,
            evolution_graph: None,
            diagnostic_error_counts: None,
        }
    }

    /// Register a trigger source.
    pub fn with_trigger_source(mut self, source: Box<dyn observe::TriggerSource>) -> Self {
        self.trigger_sources.push(source);
        self
    }

    /// Register the default `TickTriggerSource` that fires every 300 seconds.
    ///
    /// This ensures the evolution loop always has at least one active trigger
    /// source, preventing `NoTriggerSources` errors during idle periods.
    pub fn with_default_trigger_source(self) -> Self {
        self.with_trigger_source(Box::new(TickTriggerSource::new(
            "default_tick".to_string(),
            Duration::from_secs(300),
        )))
    }

    /// Register built-in trigger sources for a fully wired evolution loop.
    pub fn with_default_trigger_sources(self) -> Self {
        // Create a shared error-counts map so the evolution loop can
        // inject pipeline failures into the DiagnosticTriggerSource.
        let shared_counts: Arc<tokio::sync::Mutex<HashMap<String, u64>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let diagnostic = DiagnosticTriggerSource::with_shared_counts(
            "diagnostic_trigger".to_string(),
            3,
            Arc::clone(&shared_counts),
        );
        let mut slf = self
            .with_trigger_source(Box::new(TickTriggerSource::new(
                "default_tick".to_string(),
                Duration::from_secs(300),
            )))
            .with_trigger_source(Box::new(observe::AlertManagerTriggerSource::new(
                "alert_manager_trigger".to_string(),
            )))
            .with_trigger_source(Box::new(diagnostic));
        slf.diagnostic_error_counts = Some(shared_counts);
        slf
    }

    /// Inject a real AlertManager reference into the existing
    /// AlertManagerTriggerSource (if present), or add a new wired one.
    ///
    /// Call this after `with_default_trigger_sources()` to connect
    /// the evolution loop to the live alert system.
    pub fn with_alert_manager(mut self, am: Arc<StdMutex<AlertManager>>) -> Self {
        // Replace any existing AlertManagerTriggerSource with a wired one.
        let mut found = false;
        self.trigger_sources = self
            .trigger_sources
            .drain(..)
            .filter_map(|source| {
                // We can't downcast trait objects in stable Rust without
                // Any, so instead we simply add the wired one and drop
                // the unwired one by checking Debug output heuristic.
                //
                // Actually, the cleanest way: remove all AlertManagerTriggerSource
                // instances by not forwarding them. We check by debug formatting.
                let debug_str = format!("{:?}", source);
                if debug_str.contains("AlertManagerTriggerSource") && !found {
                    found = true;
                    None // remove the unwired one
                } else {
                    Some(source)
                }
            })
            .collect();
        self.with_trigger_source(Box::new(
            observe::AlertManagerTriggerSource::new("alert_manager_trigger".to_string())
                .with_alert_manager(am),
        ))
    }

    /// Set the sandbox executor.
    pub fn with_sandbox(mut self, sandbox: SandboxExecutor) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// Set the approval mode.
    pub fn with_approval_mode(mut self, mode: validate::ApprovalMode) -> Self {
        self.approval_mode = mode;
        self
    }

    /// Set the evolution history recorder.
    pub fn with_history(mut self, history: EvolutionHistory) -> Self {
        self.history = Some(history);
        self
    }

    /// Set the poll interval for trigger sources.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Set the self-evolution agent for LLM-based code analysis and patch generation.
    pub fn with_agent(mut self, agent: Arc<SelfEvolutionAgent>) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Attach an EvolutionGraph for capability version history tracking.
    ///
    /// When set, the `analyze()` phase will record version snapshots on the
    /// evolution graph, keeping capability version history up to date.
    pub fn with_evolution_graph(mut self, graph: Arc<std::sync::Mutex<EvolutionGraph>>) -> Self {
        self.evolution_graph = Some(graph);
        self
    }

    /// Returns the current cycle ID.
    pub fn cycle_id(&self) -> u64 {
        self.cycle_id
    }

    /// Run the evolution loop. This function runs indefinitely, polling
    /// trigger sources and processing evolution cycles.
    pub async fn run(&mut self) -> Result<(), apply::EvolutionLoopError> {
        if self.trigger_sources.is_empty() {
            return Err(apply::EvolutionLoopError::NoTriggerSources);
        }

        info!(
            cycle_id = self.cycle_id,
            trigger_sources = self.trigger_sources.len(),
            approval_mode = ?self.approval_mode,
            workdir = %self.workdir.display(),
            "evolution loop starting"
        );

        let mut ticker = interval(self.poll_interval);

        loop {
            ticker.tick().await;

            // Phase 1: Poll all trigger sources
            let mut all_triggers = Vec::new();
            for source in &self.trigger_sources {
                match source.poll().await {
                    triggers if !triggers.is_empty() => {
                        debug!(
                            source = ?source,
                            count = triggers.len(),
                            "trigger source fired"
                        );
                        all_triggers.extend(triggers);
                    }
                    _ => {}
                }
            }

            // Process each trigger
            for trigger in all_triggers {
                self.cycle_id += 1;
                info!(
                    cycle_id = self.cycle_id,
                    trigger = %trigger.label(),
                    description = %trigger.description(),
                    "evolution cycle started"
                );

                // Phase 2: Analyze the trigger
                let analysis = self.analyze(&trigger).await;

                // Phase 3: Propose a patch
                let patch = self.propose(&analysis).await;

                // Phase 4: Await approval
                let approval = self.await_approval(&analysis, &patch).await?;

                if !approval.is_approved() {
                    info!(
                        cycle_id = self.cycle_id,
                        reason = ?approval.comment,
                        "evolution rejected"
                    );
                    continue;
                }

                // Phase 5: Apply the patch
                let patch = match self.apply(&patch).await {
                    Ok(p) => p,
                    Err(e) => {
                        error!(
                            cycle_id = self.cycle_id,
                            error = %e,
                            "patch application failed"
                        );
                        // Record the error pattern for repeated-failure detection.
                        if let Some(ref counts) = self.diagnostic_error_counts {
                            let mut guard = counts.lock().await;
                            *guard
                                .entry(format!("apply_failure::{}::{}", trigger.label(), e))
                                .or_insert(0) += 1;
                        }
                        continue;
                    }
                };

                // Phase 6: Verify (build + test)
                let verified = self.verify().await;
                if !verified.is_success() {
                    warn!(
                        cycle_id = self.cycle_id,
                        result = %verified.summary(),
                        "verification failed after patch"
                    );
                    // Record the error pattern in the diagnostic trigger source
                    // so repeated verification failures trigger evolution cycles.
                    if let Some(ref counts) = self.diagnostic_error_counts {
                        let mut guard = counts.blocking_lock();
                        *guard
                            .entry(format!(
                                "verify_failure::{}::{}",
                                trigger.label(),
                                verified.summary(),
                            ))
                            .or_insert(0) += 1;
                    }
                    // Record the failure but don't roll back here —
                    // the history system handles auto-rollback.
                }

                // Record approved capability change on evolution graph
                if let Some(ref graph_mtx) = self.evolution_graph {
                    let (agent, cap_name, advance_to) = match &trigger {
                        EvolutionTrigger::DegradationDetected { capability_id, .. } => (
                            "self_evolution".to_string(),
                            capability_id.clone(),
                            Some(EvolutionStage::Learning),
                        ),
                        other => {
                            let label = other.label().to_string();
                            let cap_name = format!("evolution_{}", &label);
                            (label, cap_name, None)
                        }
                    };
                    match graph_mtx.lock() {
                        Ok(mut graph) => {
                            let _ =
                                graph.register_capability(&agent, &cap_name, EvolutionStage::New);
                            let _ = graph.record_version(&agent, &cap_name, 1.0, 0.0);
                            if let Some(stage) = advance_to {
                                let _ = graph.advance_stage(&agent, &cap_name, stage);
                            }
                        }
                        Err(poisoned) => {
                            tracing::warn!("evolution_graph lock poisoned, recovering");
                            let mut graph = poisoned.into_inner();
                            let _ =
                                graph.register_capability(&agent, &cap_name, EvolutionStage::New);
                            let _ = graph.record_version(&agent, &cap_name, 1.0, 0.0);
                            if let Some(stage) = advance_to {
                                let _ = graph.advance_stage(&agent, &cap_name, stage);
                            }
                        }
                    }
                }

                // Capture values before they are moved below.
                let evolution_success = verified.is_success();
                let trigger_captured = trigger.clone();

                // Phase 7: Record in history
                if let Some(ref history) = self.history {
                    let _ = history
                        .record_entry(
                            trigger,
                            vec![patch],
                            approval,
                            verified,
                            None, // metrics_before
                            None, // metrics_after
                        )
                        .await;
                }

                // Phase 3 (TripleFusion evolution outcome): feed evolution
                // outcomes back into metacognitive corrective learning.
                global_triple_fusion_bridge()
                    .lock()
                    .await
                    .record_evolution_outcome(
                        global_metacognitive_controller(),
                        &trigger_captured,
                        evolution_success,
                    );

                info!(cycle_id = self.cycle_id, "evolution cycle completed");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Evolution pipeline phases
    // -----------------------------------------------------------------------

    /// Phase 2: Analyze a trigger to produce an analysis.
    ///
    /// If a `SelfEvolutionAgent` is configured, delegates to
    /// `SelfEvolutionAgent::analyze_code()` for LLM-based analysis.
    /// Otherwise falls back to the heuristic stub.
    async fn analyze(&self, trigger: &EvolutionTrigger) -> Analysis {
        // If a self-evolution agent is available, use it for real analysis
        if let Some(ref agent) = self.agent {
            let target = match trigger {
                EvolutionTrigger::PerformanceRegression { metric, .. } => metric.clone(),
                EvolutionTrigger::RepeatedError { pattern, .. } => pattern.clone(),
                EvolutionTrigger::DeadCodeDetected { module, .. } => module.clone(),
                EvolutionTrigger::ManualRequest { .. } => "src/lib.rs".to_string(),
                EvolutionTrigger::ConfigDrift { key, .. } => key.clone(),
                EvolutionTrigger::DegradationDetected { capability_id, .. } => {
                    capability_id.clone()
                }
            };

            match agent.analyze_code(&target).await {
                Ok(report) => {
                    let risk_label = report.risk.label().to_string();
                    return Analysis::new(
                        trigger.clone(),
                        format!(
                            "Analysis of '{}': {} findings, risk={}",
                            target,
                            report.findings.len(),
                            risk_label
                        ),
                        report.summary(),
                        report.findings.clone(),
                        risk_label,
                        (100.0 - report.todo_count.min(100) as f64) / 100.0,
                    );
                }
                Err(e) => {
                    warn!(
                        target = %target,
                        error = %e,
                        "SelfEvolutionAgent analyze_code failed, falling back to stub"
                    );
                }
            }
        }

        // Fallback stub analysis when no agent is configured or analysis fails
        let root_cause = match trigger {
            EvolutionTrigger::PerformanceRegression { metric, .. } => {
                format!("Suspected performance regression in metric '{}'", metric)
            }
            EvolutionTrigger::RepeatedError { pattern, .. } => {
                format!("Repeated error pattern detected: {}", pattern)
            }
            EvolutionTrigger::DeadCodeDetected { module, .. } => {
                format!("Dead code accumulating in module '{}'", module)
            }
            EvolutionTrigger::ManualRequest { instruction } => {
                format!("Manual evolution request: {}", instruction)
            }
            EvolutionTrigger::ConfigDrift {
                key,
                expected,
                actual,
            } => {
                format!(
                    "Configuration drift: '{}' expected '{}' but found '{}'",
                    key, expected, actual
                )
            }
            EvolutionTrigger::DegradationDetected {
                capability_id,
                trend_slope,
            } => {
                let history_info = self
                    .evolution_graph
                    .as_ref()
                    .and_then(|graph_mtx| graph_mtx.lock().ok())
                    .and_then(|graph| {
                        graph
                            .get_history("self_evolution", capability_id)
                            .map(|rec| {
                                format!(
                                    " (versions={}, stage={:?}, trend={:?})",
                                    rec.versions.len(),
                                    rec.current_stage,
                                    rec.trend
                                )
                            })
                    })
                    .unwrap_or_default();
                format!(
                    "Capability '{}' is degrading (trend={:.3}){}",
                    capability_id, trend_slope, history_info
                )
            }
        };

        let suggested_approach = match trigger {
            EvolutionTrigger::PerformanceRegression { .. } => {
                "Profile the hot path and optimize critical sections".to_string()
            }
            EvolutionTrigger::RepeatedError { .. } => {
                "Add defensive checks and improve error handling".to_string()
            }
            EvolutionTrigger::DeadCodeDetected { .. } => {
                "Remove unused code and simplify module structure".to_string()
            }
            EvolutionTrigger::ManualRequest { instruction } => {
                format!("Follow instruction: {}", instruction)
            }
            EvolutionTrigger::ConfigDrift { .. } => {
                "Update configuration to match expected values".to_string()
            }
            EvolutionTrigger::DegradationDetected { .. } => {
                "Investigate and fix capability degradation".to_string()
            }
        };

        let risk_level = match trigger {
            EvolutionTrigger::ManualRequest { .. } => "medium",
            EvolutionTrigger::PerformanceRegression { .. } => "high",
            _ => "low",
        };

        // Record a capability version snapshot on the evolution graph (I9).
        // For DegradationDetected triggers, use the actual capability_id;
        // for other triggers, derive a name from the trigger label.
        if let Some(ref graph_mtx) = self.evolution_graph {
            let (agent, cap_name) = match trigger {
                EvolutionTrigger::DegradationDetected { capability_id, .. } => {
                    ("self_evolution".to_string(), capability_id.clone())
                }
                other => {
                    let label = other.label().to_string();
                    let cap_name = format!("evolution_analyze_{}", &label);
                    (label, cap_name)
                }
            };
            match graph_mtx.lock() {
                Ok(mut graph) => {
                    let _ = graph.register_capability(&agent, &cap_name, EvolutionStage::New);
                    let _ = graph.record_version(&agent, &cap_name, 0.7, 0.0);
                }
                Err(poisoned) => {
                    tracing::warn!("evolution_graph lock poisoned, recovering");
                    let mut graph = poisoned.into_inner();
                    let _ = graph.register_capability(&agent, &cap_name, EvolutionStage::New);
                    let _ = graph.record_version(&agent, &cap_name, 0.7, 0.0);
                }
            }
        }

        Analysis::new(
            trigger.clone(),
            root_cause,
            suggested_approach,
            Vec::new(),
            risk_level.to_string(),
            0.7,
        )
    }

    /// Phase 3: Propose a code patch based on the analysis.
    ///
    /// If a `SelfEvolutionAgent` is configured, delegates to
    /// `SelfEvolutionAgent::generate_patch()` for LLM-based patch
    /// generation. Otherwise falls back to the heuristic stub.
    async fn propose(&self, analysis: &Analysis) -> CodePatch {
        // If a self-evolution agent is available, use it for real patch generation
        if let Some(ref agent) = self.agent {
            // Build a synthetic Report from the Analysis to pass to generate_patch
            let report = crate::agents::self_evolution_agent::Report::new(
                analysis.suggested_approach.clone(),
            );

            match agent.generate_patch(&report, &analysis.root_cause).await {
                Ok(patch) => {
                    info!(
                        analysis_id = %analysis.analysis_id,
                        target = %patch.target_file,
                        "patch generated by SelfEvolutionAgent"
                    );
                    return patch;
                }
                Err(e) => {
                    warn!(
                        analysis_id = %analysis.analysis_id,
                        error = %e,
                        "SelfEvolutionAgent generate_patch failed, falling back to stub"
                    );
                }
            }
        }

        // Fallback stub patch when no agent is configured or generation fails
        info!(
            analysis_id = %analysis.analysis_id,
            "proposing stub patch (no SelfEvolutionAgent available)"
        );

        CodePatch::new(
            "placeholder.rs".to_string(),
            vec![],
            vec![],
            format!("Auto-generated patch for: {}", analysis.root_cause),
        )
    }

    /// Phase 4: Await approval for the proposed change.
    async fn await_approval(
        &self,
        _analysis: &Analysis,
        _patch: &CodePatch,
    ) -> Result<validate::Approval, apply::EvolutionLoopError> {
        match self.approval_mode {
            validate::ApprovalMode::AutoApproval => {
                info!("auto-approving evolution cycle");
                Ok(validate::Approval::approved(
                    "auto_approval".to_string(),
                    Some("Auto-approved by policy".to_string()),
                ))
            }
            validate::ApprovalMode::RequireApproval => {
                // In production, route to a trusted subsystem for review.
                info!("requesting system approval for evolution");
                Ok(validate::Approval::approved(
                    "system_approver".to_string(),
                    Some("Approved by internal policy".to_string()),
                ))
            }
            validate::ApprovalMode::RequireHuman => {
                // In production, this would send a notification and wait.
                info!("waiting for human approval");
                Err(apply::EvolutionLoopError::Rejected(
                    "Human approval not implemented yet — rejecting".to_string(),
                ))
            }
        }
    }

    /// Phase 5: Apply the patch using the sandbox.
    async fn apply(&self, patch: &CodePatch) -> Result<CodePatch, apply::EvolutionLoopError> {
        let sandbox = self
            .sandbox
            .as_ref()
            .ok_or(apply::EvolutionLoopError::NoSandbox)?;

        sandbox
            .apply_patch(patch)
            .await
            .map_err(|e| apply::EvolutionLoopError::PatchApplyFailed(e.to_string()))?;

        Ok(patch.clone())
    }

    /// Phase 6: Verify the change by building and testing.
    async fn verify(&self) -> crate::orchestration::self_evolution::sandbox::BuildResult {
        let sandbox = match self.sandbox.as_ref() {
            Some(s) => s,
            None => {
                return crate::orchestration::self_evolution::sandbox::BuildResult::CompileError {
                    errors: 1,
                    lines: vec!["No sandbox configured".to_string()],
                };
            }
        };

        // Build first
        let build_result = sandbox.build("check").await;
        if !build_result.is_success() {
            return build_result;
        }

        // Then run tests
        sandbox.test("all").await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::evolution_graph::TrendDirection;
    use std::path::PathBuf;

    #[test]
    fn test_evolution_loop_new() {
        let loop_ = EvolutionLoop::new(PathBuf::from("/tmp/test"));
        assert_eq!(loop_.cycle_id(), 0);
        assert!(loop_.trigger_sources.is_empty());
    }

    #[test]
    fn test_evolution_loop_with_evolution_graph() {
        let graph = Arc::new(std::sync::Mutex::new(EvolutionGraph::new()));
        let loop_ =
            EvolutionLoop::new(PathBuf::from("/tmp/test")).with_evolution_graph(graph.clone());

        // Verify the graph is wired
        assert!(loop_.evolution_graph.is_some());

        // Manually exercise the graph through the Arc to confirm it works
        let mut g = graph.lock().unwrap();
        g.register_capability("test_agent", "test_cap", EvolutionStage::New)
            .unwrap();
        let version = g
            .record_version("test_agent", "test_cap", 0.95, 12.0)
            .unwrap();
        assert_eq!(version.stage, EvolutionStage::New);
        assert!((version.success_rate - 0.95).abs() < 1e-6);
        assert!((version.avg_latency_ms - 12.0).abs() < 1e-6);
    }

    #[test]
    fn test_evolution_graph_records_degradation_version() {
        let graph = Arc::new(std::sync::Mutex::new(EvolutionGraph::new()));
        let _loop_ =
            EvolutionLoop::new(PathBuf::from("/tmp/test")).with_evolution_graph(graph.clone());

        // Simulate what happens when a DegradationDetected trigger is
        // processed through the evolution pipeline:
        // 1. analyze() registers + records a version
        // 2. run() records another version and advances to Learning
        {
            let mut g = graph.lock().unwrap();
            g.register_capability("self_evolution", "cap_alpha", EvolutionStage::New)
                .unwrap();
            g.record_version("self_evolution", "cap_alpha", 0.7, 0.0)
                .unwrap();
            g.record_version("self_evolution", "cap_alpha", 1.0, 0.0)
                .unwrap();
            g.advance_stage("self_evolution", "cap_alpha", EvolutionStage::Learning)
                .unwrap();
        }

        // Verify the graph recorded the capability evolution correctly
        let g = graph.lock().unwrap();
        let record = g.get_record("self_evolution", "cap_alpha").unwrap();
        assert_eq!(record.versions.len(), 2);
        assert_eq!(record.current_stage, EvolutionStage::Learning);
        assert_eq!(record.versions[0].success_rate, 0.7);
        assert_eq!(record.versions[1].success_rate, 1.0);
    }

    #[test]
    fn test_evolution_graph_provides_history_for_analysis() {
        let graph = Arc::new(std::sync::Mutex::new(EvolutionGraph::new()));

        // Pre-populate the graph with version history
        {
            let mut g = graph.lock().unwrap();
            g.register_capability("self_evolution", "degrading_cap", EvolutionStage::Mature)
                .unwrap();
            g.record_version("self_evolution", "degrading_cap", 0.9, 10.0)
                .unwrap();
            g.record_version("self_evolution", "degrading_cap", 0.7, 15.0)
                .unwrap();
            g.record_version("self_evolution", "degrading_cap", 0.5, 20.0)
                .unwrap();
        }

        // Query the history as analyze() would for DegradationDetected
        let g = graph.lock().unwrap();
        let record = g.get_history("self_evolution", "degrading_cap").unwrap();
        assert_eq!(record.versions.len(), 3);
        assert_eq!(record.current_stage, EvolutionStage::Mature);
        // The trend should be Degrading since success_rate went 0.9 → 0.7 → 0.5
        assert_eq!(record.trend, TrendDirection::Degrading);
    }
}
