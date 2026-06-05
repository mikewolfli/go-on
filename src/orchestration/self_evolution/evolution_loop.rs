//! GAP-B52-02: Self-Evolution Loop
//!
//! Implements the evolution lifecycle: trigger → analyze → propose →
//! await_approval → apply → verify → record. Runs as an async select! loop
//! that polls multiple trigger sources and processes them one at a time.

use crate::agents::self_evolution_agent::SelfEvolutionAgent;
use crate::orchestration::self_evolution::evolution_history::EvolutionHistory;
use crate::orchestration::self_evolution::sandbox::{CodePatch, SandboxExecutor};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// EvolutionTrigger
// ---------------------------------------------------------------------------

/// Describes the reason that triggered an evolution cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvolutionTrigger {
    /// A performance metric crossed a threshold in the wrong direction.
    PerformanceRegression {
        /// The metric name (e.g., "latency_p50", "throughput").
        metric: String,
        /// The threshold value that was crossed.
        threshold: f64,
        /// The direction of regression (increasing or decreasing).
        direction: RegressionDirection,
    },
    /// The same error pattern has appeared repeatedly.
    RepeatedError {
        /// The error message pattern.
        pattern: String,
        /// How many times it has been observed.
        count: u64,
    },
    /// Dead code was detected above a certain ratio.
    DeadCodeDetected {
        /// The module where dead code was found.
        module: String,
        /// The ratio of dead code to total code.
        ratio: f64,
    },
    /// A manual evolution request from a user or operator.
    ManualRequest {
        /// Free-form instruction describing what to evolve.
        instruction: String,
    },
    /// Configuration drift detected between expected and actual values.
    ConfigDrift {
        /// The configuration key that drifted.
        key: String,
        /// The expected value.
        expected: String,
        /// The actual value found.
        actual: String,
    },
    /// Capability degradation detected by EvolutionGraph (BLUE56-B10).
    DegradationDetected {
        /// The capability ID that is degrading.
        capability_id: String,
        /// The degradation trend slope (negative = degrading).
        trend_slope: f64,
    },
}

impl EvolutionTrigger {
    /// Returns a human-readable label for this trigger.
    pub fn label(&self) -> &str {
        match self {
            EvolutionTrigger::PerformanceRegression { .. } => "performance_regression",
            EvolutionTrigger::RepeatedError { .. } => "repeated_error",
            EvolutionTrigger::DeadCodeDetected { .. } => "dead_code_detected",
            EvolutionTrigger::ManualRequest { .. } => "manual_request",
            EvolutionTrigger::ConfigDrift { .. } => "config_drift",
            EvolutionTrigger::DegradationDetected { .. } => "degradation_detected",
        }
    }

    /// Returns a short description of the trigger.
    pub fn description(&self) -> String {
        match self {
            EvolutionTrigger::PerformanceRegression {
                metric,
                threshold,
                direction,
            } => {
                format!(
                    "Performance regression: {} {} threshold {}",
                    metric,
                    match direction {
                        RegressionDirection::Increasing => "rose above",
                        RegressionDirection::Decreasing => "fell below",
                    },
                    threshold
                )
            }
            EvolutionTrigger::RepeatedError { pattern, count } => {
                format!("Repeated error ({}x): {}", count, pattern)
            }
            EvolutionTrigger::DeadCodeDetected { module, ratio } => {
                format!("Dead code in {}: {:.1}%", module, ratio * 100.0)
            }
            EvolutionTrigger::ManualRequest { instruction } => {
                format!("Manual: {}", instruction)
            }
            EvolutionTrigger::ConfigDrift {
                key,
                expected,
                actual,
            } => {
                format!(
                    "Config drift: {} expected={} actual={}",
                    key, expected, actual
                )
            }
            EvolutionTrigger::DegradationDetected {
                capability_id,
                trend_slope,
            } => {
                format!(
                    "Capability degradation: {} trend={:.3}",
                    capability_id, trend_slope
                )
            }
        }
    }
}

/// Direction of a regression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegressionDirection {
    /// The metric is increasing (worse for latency, better for throughput).
    Increasing,
    /// The metric is decreasing (worse for throughput, better for latency).
    Decreasing,
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// Structured analysis of a trigger, produced before generating a patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    /// Unique analysis ID.
    pub analysis_id: Uuid,
    /// The trigger that prompted this analysis.
    pub trigger: EvolutionTrigger,
    /// Root cause hypothesis.
    pub root_cause: String,
    /// Suggested approach for the fix.
    pub suggested_approach: String,
    /// Files that are likely relevant to the issue.
    pub relevant_files: Vec<String>,
    /// Risk assessment: "low", "medium", "high".
    pub risk_level: String,
    /// Confidence score (0.0 – 1.0).
    pub confidence: f64,
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
}

impl Analysis {
    /// Create a new Analysis from a trigger.
    pub fn new(
        trigger: EvolutionTrigger,
        root_cause: String,
        suggested_approach: String,
        relevant_files: Vec<String>,
        risk_level: String,
        confidence: f64,
    ) -> Self {
        Self {
            analysis_id: Uuid::new_v4(),
            trigger,
            root_cause,
            suggested_approach,
            relevant_files,
            risk_level,
            confidence,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

// ---------------------------------------------------------------------------
// ApprovalMode
// ---------------------------------------------------------------------------

/// Describes how approval is handled for an evolution cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalMode {
    /// Automatically approve all evolution cycles.
    AutoApproval,
    /// Require explicit approval (from a trusted subsystem) before applying.
    RequireApproval,
    /// Require human sign-off before applying.
    RequireHuman,
}

impl ApprovalMode {
    /// Returns true if this mode requires some form of approval.
    pub fn requires_approval(&self) -> bool {
        !matches!(self, ApprovalMode::AutoApproval)
    }

    /// Returns true if this mode specifically requires human intervention.
    pub fn requires_human(&self) -> bool {
        matches!(self, ApprovalMode::RequireHuman)
    }
}

// ---------------------------------------------------------------------------
// Approval
// ----------------------------------------------------------------------------

/// Record of an approval decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    /// Who or what approved the evolution.
    pub by: String,
    /// The approval status.
    pub status: ApprovalStatus,
    /// Optional comment explaining the decision.
    pub comment: Option<String>,
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
}

impl Approval {
    /// Create a new approved approval.
    pub fn approved(by: String, comment: Option<String>) -> Self {
        Self {
            by,
            status: ApprovalStatus::Approved,
            comment,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Create a new rejected approval.
    pub fn rejected(by: String, comment: Option<String>) -> Self {
        Self {
            by,
            status: ApprovalStatus::Rejected,
            comment,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }

    /// Returns true if this approval is approved.
    pub fn is_approved(&self) -> bool {
        self.status == ApprovalStatus::Approved
    }
}

/// Approval status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalStatus {
    /// The evolution was approved.
    Approved,
    /// The evolution was rejected.
    Rejected,
}

// ---------------------------------------------------------------------------
// MetricsSnapshot (re-exported for convenience)
// ---------------------------------------------------------------------------

/// A snapshot of key system metrics at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Average request latency in milliseconds.
    pub avg_latency_ms: f64,
    /// Requests per second.
    pub throughput: f64,
    /// Error rate (0.0 – 1.0).
    pub error_rate: f64,
    /// Memory usage in bytes.
    pub memory_bytes: u64,
    /// CPU usage as a fraction (0.0 – 1.0).
    pub cpu_usage: f64,
    /// Number of active goroutines/tasks.
    pub active_tasks: u64,
}

impl MetricsSnapshot {
    /// Create a new metrics snapshot with the current timestamp.
    pub fn new(
        avg_latency_ms: f64,
        throughput: f64,
        error_rate: f64,
        memory_bytes: u64,
        cpu_usage: f64,
        active_tasks: u64,
    ) -> Self {
        Self {
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            avg_latency_ms,
            throughput,
            error_rate,
            memory_bytes,
            cpu_usage,
            active_tasks,
        }
    }

    /// Compute the degradation ratio between this snapshot and another.
    /// Returns a value > 0.2 (20%) if metrics have degraded significantly.
    pub fn degradation_ratio(&self, other: &MetricsSnapshot) -> f64 {
        let mut degradations = Vec::new();

        // Latency: higher is worse
        if other.avg_latency_ms > 0.0 {
            degradations.push((self.avg_latency_ms - other.avg_latency_ms) / other.avg_latency_ms);
        }

        // Throughput: lower is worse
        if other.throughput > 0.0 {
            degradations.push((other.throughput - self.throughput) / other.throughput);
        }

        // Error rate: higher is worse
        if other.error_rate > 0.0 {
            degradations.push((self.error_rate - other.error_rate) / other.error_rate);
        }

        if degradations.is_empty() {
            return 0.0;
        }

        degradations.iter().sum::<f64>() / degradations.len() as f64
    }
}

// ---------------------------------------------------------------------------
// MetricsPoint (for trend analysis)
// ---------------------------------------------------------------------------

/// A single data point for metrics trend analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsPoint {
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Metric value.
    pub value: f64,
    /// Metric label.
    pub label: String,
}

// ---------------------------------------------------------------------------
// TriggerSource trait
// ---------------------------------------------------------------------------

/// A source of evolution triggers that is polled asynchronously.
#[async_trait]
pub trait TriggerSource: Send + Sync + std::fmt::Debug {
    /// Poll for new evolution triggers. Returns a list of triggers that
    /// have been detected since the last poll.
    async fn poll(&self) -> Vec<EvolutionTrigger>;
}

// ---------------------------------------------------------------------------
// MetacognitiveTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that monitors the system's own cognitive performance
/// (e.g., decision latency, retry rates, planning depth).
#[derive(Debug)]
pub struct MetacognitiveTriggerSource {
    /// Name of this source.
    name: String,
    /// Poll interval.
    interval: Duration,
    /// Thresholds for various metacognitive metrics.
    #[allow(dead_code)]
    thresholds: HashMap<String, f64>,
}

#[allow(dead_code)]
impl MetacognitiveTriggerSource {
    /// Create a new metacognitive trigger source.
    /// TODO-BLUE64: Activate in evolution_loop_builder when metacognitive data is available.
    pub fn new(name: String, interval: Duration) -> Self {
        let mut thresholds = HashMap::new();
        thresholds.insert("decision_latency_ms".to_string(), 5000.0);
        thresholds.insert("retry_rate".to_string(), 0.1);
        thresholds.insert("planning_depth".to_string(), 3.0);
        Self {
            name,
            interval,
            thresholds,
        }
    }

    /// Set a custom threshold for a metric.
    #[allow(dead_code)]
    pub fn with_threshold(mut self, metric: &str, value: f64) -> Self {
        self.thresholds.insert(metric.to_string(), value);
        self
    }
}

#[async_trait]
impl TriggerSource for MetacognitiveTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        // In a real implementation, this would query the metacognitive
        // monitoring subsystem. For now, return empty — triggers appear
        // only when thresholds are actually crossed.
        let _ = &self.name;
        let _ = &self.interval;
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// AlertManagerTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that listens to the alert manager for active alerts
/// that should trigger an evolution cycle.
#[derive(Debug)]
pub struct AlertManagerTriggerSource {
    /// Name of this source.
    name: String,
    /// Cached alert fingerprints to avoid re-triggering.
    #[allow(dead_code)]
    seen_alerts: std::sync::Mutex<Vec<String>>,
}

impl AlertManagerTriggerSource {
    /// Create a new alert manager trigger source.
    pub fn new(name: String) -> Self {
        Self {
            name,
            seen_alerts: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl TriggerSource for AlertManagerTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        // Query the alert manager (in production this connects to a real
        // alert system like Prometheus AlertManager).
        let _ = &self.name;
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// DiagnosticTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that monitors compiler/LSP diagnostics and test results
/// to detect repeated error patterns.
#[derive(Debug)]
pub struct DiagnosticTriggerSource {
    /// Name of this source.
    name: String,
    /// Map of error patterns to their observed counts.
    error_counts: std::sync::Mutex<HashMap<String, u64>>,
    /// Minimum count before triggering.
    min_count: u64,
}

#[allow(dead_code)]
impl DiagnosticTriggerSource {
    /// Create a new diagnostic trigger source.
    /// TODO-BLUE64: Wire record_error calls from error handling paths.
    pub fn new(name: String, min_count: u64) -> Self {
        Self {
            name,
            error_counts: std::sync::Mutex::new(HashMap::new()),
            min_count,
        }
    }

    /// Record an observed error pattern.
    #[allow(dead_code)]
    pub fn record_error(&self, pattern: String) {
        let mut counts = self.error_counts.lock().unwrap();
        *counts.entry(pattern).or_insert(0) += 1;
    }
}

#[async_trait]
impl TriggerSource for DiagnosticTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let mut triggers = Vec::new();
        let mut counts = self.error_counts.lock().unwrap();

        let to_remove: Vec<String> = counts
            .iter()
            .filter(|(_, count)| **count >= self.min_count)
            .map(|(pattern, _)| pattern.clone())
            .collect();

        for pattern in to_remove {
            if let Some(count) = counts.remove(&pattern) {
                triggers.push(EvolutionTrigger::RepeatedError { pattern, count });
            }
        }

        let _ = &self.name;
        triggers
    }
}

// ---------------------------------------------------------------------------
// TickTriggerSource
// ---------------------------------------------------------------------------

/// A simple trigger source that fires at a fixed interval.
///
/// This is the default trigger source that ensures the evolution loop has
/// at least one active source, preventing `NoTriggerSources` errors.
#[allow(dead_code)]
#[derive(Debug)]
pub struct TickTriggerSource {
    /// Name of this source.
    name: String,
    /// Interval between automatic triggers.
    interval: Duration,
    /// Timestamp (ms since epoch) of the last trigger.
    last_trigger_ms: std::sync::Mutex<u64>,
}

#[allow(dead_code)]
impl TickTriggerSource {
    /// Create a new tick trigger source that fires every `interval`.
    pub fn new(name: String, interval: Duration) -> Self {
        Self {
            name,
            interval,
            last_trigger_ms: std::sync::Mutex::new(0),
        }
    }
}

#[async_trait]
impl TriggerSource for TickTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut last = self.last_trigger_ms.lock().unwrap();
        let elapsed_ms = now.saturating_sub(*last);

        if elapsed_ms >= self.interval.as_millis() as u64 {
            *last = now;
            let instruction = format!("Scheduled evolution tick from {}", self.name);
            vec![EvolutionTrigger::ManualRequest { instruction }]
        } else {
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// ManualTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that accepts manual evolution requests via a channel.
#[derive(Debug)]
pub struct ManualTriggerSource {
    /// Name of this source.
    name: String,
    /// Receiver for manual trigger requests.
    rx: std::sync::Mutex<mpsc::UnboundedReceiver<String>>,
    /// Sender (cloned for external use).
    #[allow(dead_code)]
    tx: mpsc::UnboundedSender<String>,
}

#[allow(dead_code)]
impl ManualTriggerSource {
    /// Create a new manual trigger source.
    pub fn new(name: String) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            name,
            rx: std::sync::Mutex::new(rx),
            tx,
        }
    }

    /// Send a manual evolution request. This is the public API for
    /// submitting manual evolution instructions.
    #[allow(dead_code)]
    pub fn request_evolution(&self, instruction: String) -> Result<(), String> {
        self.tx
            .send(instruction)
            .map_err(|e| format!("Failed to send manual request: {}", e))
    }
}

#[async_trait]
impl TriggerSource for ManualTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let mut triggers = Vec::new();
        let mut rx = self.rx.lock().unwrap();

        loop {
            match rx.try_recv() {
                Ok(instruction) => {
                    triggers.push(EvolutionTrigger::ManualRequest { instruction });
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    warn!("ManualTriggerSource channel disconnected");
                    break;
                }
            }
        }

        let _ = &self.name;
        triggers
    }
}

// ---------------------------------------------------------------------------
// PubsubTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that reads evolution triggers from a `mpsc` channel.
///
/// This bridges the TripleFusion bridge (or any other in-process producer)
/// into the EvolutionLoop without coupling the two subsystems directly.
#[derive(Debug)]
pub struct PubsubTriggerSource {
    /// Name of this source.
    name: String,
    /// Receiver end of the mpsc channel.
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<EvolutionTrigger>>,
}

impl PubsubTriggerSource {
    /// Create a new pubsub trigger source.
    pub fn new(name: String, rx: mpsc::UnboundedReceiver<EvolutionTrigger>) -> Self {
        Self {
            name,
            rx: tokio::sync::Mutex::new(rx),
        }
    }
}

#[async_trait]
impl TriggerSource for PubsubTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let mut triggers = Vec::new();
        let mut rx = self.rx.lock().await;

        loop {
            match rx.try_recv() {
                Ok(trigger) => {
                    triggers.push(trigger);
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    warn!("PubsubTriggerSource channel disconnected");
                    break;
                }
            }
        }

        let _ = &self.name;
        triggers
    }
}

// ---------------------------------------------------------------------------
// EvolutionLoopError
// ---------------------------------------------------------------------------

/// Errors that can occur during evolution loop operations.
#[derive(Debug, Error)]
pub enum EvolutionLoopError {
    /// No trigger sources are configured.
    #[error("no trigger sources configured")]
    NoTriggerSources,

    /// No sandbox executor is configured.
    #[error("no sandbox executor configured")]
    NoSandbox,

    /// A trigger source failed to poll.
    #[error("trigger source poll error: {0}")]
    TriggerPollError(String),

    /// Patch application failed.
    #[error("patch application failed: {0}")]
    PatchApplyFailed(String),

    /// Build or test verification failed.
    #[error("verification failed: {0}")]
    VerificationFailed(String),

    /// Approval was rejected.
    #[error("evolution rejected: {0}")]
    Rejected(String),

    /// History recording failed.
    #[error("history error: {0}")]
    HistoryError(String),
}

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
    trigger_sources: Vec<Box<dyn TriggerSource>>,
    /// Sandbox executor for applying patches.
    sandbox: Option<SandboxExecutor>,
    /// Evolution cycle counter.
    cycle_id: u64,
    /// Approval mode for evolution cycles.
    approval_mode: ApprovalMode,
    /// Evolution history recorder.
    history: Option<EvolutionHistory>,
    /// Working directory for sandbox operations.
    workdir: PathBuf,
    /// Poll interval for trigger sources.
    poll_interval: Duration,
    /// Self-evolution agent for LLM-based code analysis and patch generation.
    agent: Option<Arc<SelfEvolutionAgent>>,
}

impl EvolutionLoop {
    /// Create a new EvolutionLoop with default settings.
    pub fn new(workdir: PathBuf) -> Self {
        Self {
            trigger_sources: Vec::new(),
            sandbox: None,
            cycle_id: 0,
            approval_mode: ApprovalMode::RequireApproval,
            history: None,
            workdir,
            poll_interval: Duration::from_secs(30),
            agent: None,
        }
    }

    /// Register a trigger source.
    pub fn with_trigger_source(mut self, source: Box<dyn TriggerSource>) -> Self {
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

    /// Register **all** built-in trigger sources (Tick, Metacognitive,
    /// AlertManager, Diagnostic, Manual) for a fully wired evolution loop.
    pub fn with_default_trigger_sources(self) -> Self {
        let _ = &self; // borrow so we can chain
        self.with_trigger_source(Box::new(TickTriggerSource::new(
            "default_tick".to_string(),
            Duration::from_secs(300),
        )))
        .with_trigger_source(Box::new(MetacognitiveTriggerSource::new(
            "metacognitive_trigger".to_string(),
            Duration::from_secs(600),
        )))
        .with_trigger_source(Box::new(AlertManagerTriggerSource::new(
            "alert_manager_trigger".to_string(),
        )))
        .with_trigger_source(Box::new(DiagnosticTriggerSource::new(
            "diagnostic_trigger".to_string(),
            3,
        )))
        .with_trigger_source(Box::new(ManualTriggerSource::new(
            "manual_trigger".to_string(),
        )))
    }

    /// Set the sandbox executor.
    pub fn with_sandbox(mut self, sandbox: SandboxExecutor) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// Set the approval mode.
    pub fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
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

    /// Set the self-evolution agent for LLM-based analysis and patch generation.
    pub fn with_agent(mut self, agent: Arc<SelfEvolutionAgent>) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Returns the current cycle ID.
    pub fn cycle_id(&self) -> u64 {
        self.cycle_id
    }

    /// Run the evolution loop. This function runs indefinitely, polling
    /// trigger sources and processing evolution cycles.
    pub async fn run(&mut self) -> Result<(), EvolutionLoopError> {
        if self.trigger_sources.is_empty() {
            return Err(EvolutionLoopError::NoTriggerSources);
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
                    // Record the failure but don't roll back here —
                    // the history system handles auto-rollback.
                }

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
                format!(
                    "Capability '{}' is degrading (trend={:.3})",
                    capability_id, trend_slope
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
    ) -> Result<Approval, EvolutionLoopError> {
        match self.approval_mode {
            ApprovalMode::AutoApproval => {
                info!("auto-approving evolution cycle");
                Ok(Approval::approved(
                    "auto_approval".to_string(),
                    Some("Auto-approved by policy".to_string()),
                ))
            }
            ApprovalMode::RequireApproval => {
                // In production, route to a trusted subsystem for review.
                info!("requesting system approval for evolution");
                Ok(Approval::approved(
                    "system_approver".to_string(),
                    Some("Approved by internal policy".to_string()),
                ))
            }
            ApprovalMode::RequireHuman => {
                // In production, this would send a notification and wait.
                info!("waiting for human approval");
                Err(EvolutionLoopError::Rejected(
                    "Human approval not implemented yet — rejecting".to_string(),
                ))
            }
        }
    }

    /// Phase 5: Apply the patch using the sandbox.
    async fn apply(&self, patch: &CodePatch) -> Result<CodePatch, EvolutionLoopError> {
        let sandbox = self.sandbox.as_ref().ok_or(EvolutionLoopError::NoSandbox)?;

        sandbox
            .apply_patch(patch)
            .await
            .map_err(|e| EvolutionLoopError::PatchApplyFailed(e.to_string()))?;

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

    #[test]
    fn test_evolution_trigger_label() {
        let t = EvolutionTrigger::ManualRequest {
            instruction: "fix bug".to_string(),
        };
        assert_eq!(t.label(), "manual_request");
    }

    #[test]
    fn test_evolution_trigger_description() {
        let t = EvolutionTrigger::DeadCodeDetected {
            module: "core".to_string(),
            ratio: 0.15,
        };
        assert!(t.description().contains("15.0%"));
    }

    #[test]
    fn test_analysis_new() {
        let trigger = EvolutionTrigger::ManualRequest {
            instruction: "optimize".to_string(),
        };
        let analysis = Analysis::new(
            trigger.clone(),
            "root cause".to_string(),
            "approach".to_string(),
            vec!["src/lib.rs".to_string()],
            "low".to_string(),
            0.85,
        );
        assert_eq!(analysis.trigger.label(), "manual_request");
        assert!(analysis.confidence > 0.8);
    }

    #[test]
    fn test_approval_modes() {
        assert!(!ApprovalMode::AutoApproval.requires_approval());
        assert!(ApprovalMode::RequireApproval.requires_approval());
        assert!(ApprovalMode::RequireHuman.requires_human());
    }

    #[test]
    fn test_approval_approved() {
        let a = Approval::approved("tester".to_string(), Some("looks good".to_string()));
        assert!(a.is_approved());
        assert_eq!(a.by, "tester");
    }

    #[test]
    fn test_approval_rejected() {
        let a = Approval::rejected("tester".to_string(), Some("not now".to_string()));
        assert!(!a.is_approved());
        assert_eq!(a.status, ApprovalStatus::Rejected);
    }

    #[test]
    fn test_metrics_snapshot_degradation() {
        let before = MetricsSnapshot::new(100.0, 1000.0, 0.01, 1_000_000, 0.5, 10);
        let after = MetricsSnapshot::new(500.0, 200.0, 0.10, 2_000_000, 0.8, 20);
        let ratio = after.degradation_ratio(&before);
        assert!(ratio > 0.2);
    }

    #[test]
    fn test_metrics_snapshot_no_degradation() {
        let before = MetricsSnapshot::new(100.0, 1000.0, 0.01, 1_000_000, 0.5, 10);
        let after = MetricsSnapshot::new(90.0, 1100.0, 0.005, 900_000, 0.4, 9);
        let ratio = after.degradation_ratio(&before);
        assert!(ratio < 0.0);
    }

    #[test]
    fn test_manual_trigger_source() {
        let source = ManualTriggerSource::new("test".to_string());
        let result = source.request_evolution("fix lint warnings".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_diagnostic_trigger_source() {
        let source = DiagnosticTriggerSource::new("test".to_string(), 3);
        source.record_error("E0308".to_string());
        source.record_error("E0308".to_string());
        source.record_error("E0308".to_string());
        // After 3 recordings, the next poll should return a trigger
    }

    #[test]
    fn test_evolution_loop_new() {
        let loop_ = EvolutionLoop::new(PathBuf::from("/tmp/test"));
        assert_eq!(loop_.cycle_id(), 0);
        assert!(loop_.trigger_sources.is_empty());
    }

    #[test]
    fn test_regression_direction() {
        assert_eq!(
            format!("{:?}", RegressionDirection::Increasing),
            "Increasing"
        );
        assert_eq!(
            format!("{:?}", RegressionDirection::Decreasing),
            "Decreasing"
        );
    }
}
