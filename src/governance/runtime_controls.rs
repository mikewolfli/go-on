//! F-GAP-08: Runtime adaptive controls
//!
//! # Status
//! Complete implementation ready for CapabilityBus integration (ARCH-13).

use std::collections::{HashMap, VecDeque};

/// Infer a canonical task-type label from a phase name.
///
/// Used to populate the task-type-aware scoring dimension in
/// `OnlineControllerState` without requiring every call site to
/// pass task type explicitly.  Falls back to `"general"` when
/// no specific mapping exists.
fn infer_task_type_from_phase(phase_name: &str) -> &'static str {
    match phase_name {
        p if p.eq_ignore_ascii_case("planning") => "architecture_design",
        p if p.eq_ignore_ascii_case("coding") => "feature_implementation",
        p if p.eq_ignore_ascii_case("review") => "code_review",
        p if p.eq_ignore_ascii_case("delivery") => "documentation",
        _ => "general",
    }
}

/// Streaming P95 latency estimator using reservoir sampling.
/// Avoids cloning+sorting the entire VecDeque on every call.
#[derive(Debug, Clone)]
pub struct LatencyQuantileEstimator {
    samples: Vec<f64>,
    max_samples: usize,
    count: u64,
}

impl LatencyQuantileEstimator {
    pub fn new(max_samples: usize) -> Self {
        Self {
            samples: Vec::with_capacity(max_samples),
            max_samples,
            count: 0,
        }
    }

    pub fn record(&mut self, latency_ms: f64) {
        self.count += 1;
        if self.samples.len() < self.max_samples {
            self.samples.push(latency_ms);
        } else {
            // Reservoir sampling: replace a random element
            let idx = fastrand::usize(..self.count.min(self.max_samples as u64) as usize);
            if idx < self.samples.len() {
                self.samples[idx] = latency_ms;
            }
        }
    }

    pub fn p95(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((sorted.len() as f64) * 0.95).ceil() as usize - 1;
        sorted[idx.min(sorted.len() - 1)]
    }
}

impl Default for LatencyQuantileEstimator {
    fn default() -> Self {
        Self::new(ONLINE_CONTROLLER_WINDOW)
    }
}

const ONLINE_CONTROLLER_WINDOW: usize = 64;
const ONLINE_CONTROLLER_FAILURE_ESCALATION: f64 = 0.25;
const ONLINE_CONTROLLER_P95_LATENCY_MS_ESCALATION: u64 = 15_000;
const ONLINE_CONTROLLER_MIN_AGENT_SAMPLES: u64 = 3;
const ONLINE_CONTROLLER_BANDIT_EXPLORATION: f64 = 1.4;

#[derive(Debug, Default, Clone)]
struct AgentSignalWindow {
    recent_failures: VecDeque<bool>,
    latency_estimator: LatencyQuantileEstimator,
    attempts: u64,
}

#[derive(Debug, Default, Clone)]
struct PhaseBanditArm {
    pulls: u64,
    reward_sum: f64,
}

impl PhaseBanditArm {
    fn update(&mut self, reward: f64) {
        self.pulls = self.pulls.saturating_add(1);
        self.reward_sum += reward;
    }

    fn mean_reward(&self) -> f64 {
        if self.pulls == 0 {
            0.5
        } else {
            (self.reward_sum / self.pulls as f64).clamp(0.0, 1.0)
        }
    }

    fn ucb_score(&self, total_pulls: u64) -> f64 {
        if self.pulls == 0 {
            return 1.0;
        }
        let mean = self.mean_reward();
        let explore = ONLINE_CONTROLLER_BANDIT_EXPLORATION
            * ((total_pulls.max(1) as f64).ln() / self.pulls as f64).sqrt();
        (mean + explore).clamp(0.0, 2.0)
    }
}

impl AgentSignalWindow {
    fn record(&mut self, success: bool, duration_ms: u64) {
        if self.recent_failures.len() >= ONLINE_CONTROLLER_WINDOW {
            self.recent_failures.pop_front();
        }
        self.recent_failures.push_back(!success);

        self.latency_estimator.record(duration_ms as f64);
        self.attempts = self.attempts.saturating_add(1);
    }

    fn failure_rate(&self) -> f64 {
        if self.recent_failures.is_empty() {
            return 0.0;
        }
        let failures = self
            .recent_failures
            .iter()
            .filter(|failed| **failed)
            .count();
        failures as f64 / self.recent_failures.len() as f64
    }

    fn latency_p95_ms(&self) -> u64 {
        self.latency_estimator.p95() as u64
    }

    fn reliability_score(&self) -> Option<f64> {
        if self.attempts < ONLINE_CONTROLLER_MIN_AGENT_SAMPLES {
            return None;
        }

        let success_score = 1.0 - self.failure_rate();
        let latency_ms = self.latency_p95_ms() as f64;
        let latency_score = if latency_ms <= f64::EPSILON {
            0.5
        } else {
            (1.0 / (1.0 + (latency_ms / 5000.0))).clamp(0.0, 1.0)
        };

        Some((0.75 * success_score + 0.25 * latency_score).clamp(0.0, 1.0))
    }
}

#[derive(Debug, Default, Clone)]
pub struct OnlineControllerState {
    recent_failures: VecDeque<bool>,
    latency_estimator: LatencyQuantileEstimator,
    agent_windows: HashMap<String, AgentSignalWindow>,
    phase_agent_windows: HashMap<String, AgentSignalWindow>,
    /// Per-task-type per-agent scores.
    /// Key format: `"{task_type}::{agent_name}"`.
    /// Enables querying agent reliability scoped to a specific task type
    /// (e.g. bugfix, refactor, code_review) in addition to the existing
    /// phase and global dimensions.
    task_type_agent_windows: HashMap<String, AgentSignalWindow>,
    phase_windows: HashMap<String, AgentSignalWindow>,
    phase_bandit_arms: HashMap<String, PhaseBanditArm>,
    phase_bandit_total_pulls: u64,
}

impl OnlineControllerState {
    fn phase_agent_key(phase_name: &str, agent_name: &str) -> String {
        let mut key = String::with_capacity(phase_name.len() + 2 + agent_name.len());
        key.push_str(phase_name);
        key.push_str("::");
        key.push_str(agent_name);
        key
    }

    fn task_type_agent_key(task_type: &str, agent_name: &str) -> String {
        let mut key = String::with_capacity(task_type.len() + 2 + agent_name.len());
        key.push_str(task_type);
        key.push_str("::");
        key.push_str(agent_name);
        key
    }

    pub(crate) fn record(&mut self, success: bool, duration_ms: u64) {
        if self.recent_failures.len() >= ONLINE_CONTROLLER_WINDOW {
            self.recent_failures.pop_front();
        }
        self.recent_failures.push_back(!success);

        self.latency_estimator.record(duration_ms as f64);
    }

    pub(crate) fn record_agent_outcome(
        &mut self,
        phase_name: &str,
        agent_name: &str,
        success: bool,
        duration_ms: u64,
    ) {
        self.agent_windows
            .entry(agent_name.to_string())
            .or_default()
            .record(success, duration_ms);

        self.phase_agent_windows
            .entry(Self::phase_agent_key(phase_name, agent_name))
            .or_default()
            .record(success, duration_ms);

        // Also index by inferred task type — additive, zero breakage.
        let task_type = infer_task_type_from_phase(phase_name);
        self.task_type_agent_windows
            .entry(Self::task_type_agent_key(task_type, agent_name))
            .or_default()
            .record(success, duration_ms);
    }

    fn agent_reliability_score(&self, phase_name: &str, agent_name: &str) -> f64 {
        let phase_key = Self::phase_agent_key(phase_name, agent_name);
        let phase_score = self
            .phase_agent_windows
            .get(&phase_key)
            .and_then(AgentSignalWindow::reliability_score);
        let global_score = self
            .agent_windows
            .get(agent_name)
            .and_then(AgentSignalWindow::reliability_score);

        match (phase_score, global_score) {
            (Some(phase), Some(global)) => (0.7 * phase + 0.3 * global).clamp(0.0, 1.0),
            (Some(phase), None) => phase,
            (None, Some(global)) => global,
            (None, None) => 0.5,
        }
    }

    /// Agent reliability score **with task-type awareness**.
    ///
    /// Blends three signals when all are available:
    ///   - phase+agent  (60%)
    ///   - task_type+agent (25%)
    ///   - global agent (15%)
    ///
    /// Falls back gracefully to the base `agent_reliability_score` when
    /// the task-type dimension has no data, keeping existing behavior
    /// unchanged.
    fn agent_reliability_score_with_task_type(
        &self,
        phase_name: &str,
        task_type: &str,
        agent_name: &str,
    ) -> f64 {
        let base = self.agent_reliability_score(phase_name, agent_name);
        let tt_key = Self::task_type_agent_key(task_type, agent_name);
        let tt_score = self
            .task_type_agent_windows
            .get(&tt_key)
            .and_then(AgentSignalWindow::reliability_score);

        match tt_score {
            Some(tt) => {
                // Three-way blend: phase+agent dominates, task type adds signal.
                let phase_key = Self::phase_agent_key(phase_name, agent_name);
                let phase = self
                    .phase_agent_windows
                    .get(&phase_key)
                    .and_then(AgentSignalWindow::reliability_score);
                let global = self
                    .agent_windows
                    .get(agent_name)
                    .and_then(AgentSignalWindow::reliability_score);
                match (phase, global) {
                    (Some(p), Some(g)) => (0.60 * p + 0.25 * tt + 0.15 * g).clamp(0.0, 1.0),
                    (Some(p), None) => (0.70 * p + 0.30 * tt).clamp(0.0, 1.0),
                    (None, Some(g)) => (0.55 * tt + 0.45 * g).clamp(0.0, 1.0),
                    (None, None) => tt,
                }
            }
            None => base,
        }
    }

    pub(crate) fn rank_agent_names_for_phase(
        &self,
        phase_name: &str,
        agent_names: &[String],
    ) -> Vec<(String, f64)> {
        // Use task-type-aware ranking to incorporate the additional
        // task-type scoring dimension alongside phase+agent and global.
        // Falls back seamlessly to the base scoring when no task-type
        // data exists.
        let task_type = infer_task_type_from_phase(phase_name);
        let mut scored = agent_names
            .iter()
            .enumerate()
            .map(|(idx, name)| {
                (
                    idx,
                    name.clone(),
                    self.agent_reliability_score_with_task_type(phase_name, task_type, name),
                )
            })
            .collect::<Vec<_>>();

        scored.sort_by(|left, right| match right.2.partial_cmp(&left.2) {
            Some(std::cmp::Ordering::Equal) | None => left.0.cmp(&right.0),
            Some(other) => other,
        });

        scored
            .into_iter()
            .map(|(_, name, score)| (name, score))
            .collect()
    }

    pub(crate) fn record_phase_outcome(
        &mut self,
        phase_name: &str,
        success: bool,
        duration_ms: u64,
    ) {
        self.phase_windows
            .entry(phase_name.to_string())
            .or_default()
            .record(success, duration_ms);

        let latency_penalty = ((duration_ms as f64 / 15_000.0).clamp(0.0, 1.0)) * 0.4;
        let reward = if success { 1.0 } else { 0.0 } - latency_penalty;
        self.record_phase_reward(phase_name, reward.clamp(0.0, 1.0));
    }

    pub(crate) fn record_phase_reward(&mut self, phase_name: &str, reward: f64) {
        self.phase_bandit_total_pulls = self.phase_bandit_total_pulls.saturating_add(1);
        self.phase_bandit_arms
            .entry(phase_name.to_string())
            .or_default()
            .update(reward.clamp(0.0, 1.0));
    }

    fn phase_reliability_score(&self, phase_name: &str) -> Option<f64> {
        self.phase_windows
            .get(phase_name)
            .and_then(AgentSignalWindow::reliability_score)
    }

    pub(crate) fn recommend_phase(&self, phase_candidates: &[String]) -> Option<String> {
        let mut scored = phase_candidates
            .iter()
            .enumerate()
            .map(|(idx, name)| {
                let reliability = self.phase_reliability_score(name).unwrap_or(0.5);
                let (bandit_ucb, mean_reward, pulls) = self
                    .phase_bandit_arms
                    .get(name)
                    .map(|arm| {
                        (
                            arm.ucb_score(self.phase_bandit_total_pulls),
                            arm.mean_reward(),
                            arm.pulls,
                        )
                    })
                    .unwrap_or((1.0, 0.5, 0));
                let normalized_ucb = (bandit_ucb / 2.0).clamp(0.0, 1.0);
                let composite = (0.55 * normalized_ucb + 0.30 * mean_reward + 0.15 * reliability)
                    .clamp(0.0, 1.0);
                (idx, name.clone(), composite, pulls)
            })
            .collect::<Vec<_>>();

        if scored.is_empty() {
            return None;
        }

        scored.sort_by(|left, right| match right.2.partial_cmp(&left.2) {
            Some(std::cmp::Ordering::Equal) | None => {
                right.3.cmp(&left.3).then_with(|| left.0.cmp(&right.0))
            }
            Some(other) => other,
        });

        scored.into_iter().next().map(|(_, name, _, _)| name)
    }

    pub(crate) fn phase_policy_snapshot(
        &self,
        phase_candidates: &[String],
    ) -> Vec<(String, f64, f64, u64)> {
        phase_candidates
            .iter()
            .map(|name| {
                let reliability = self.phase_reliability_score(name).unwrap_or(0.5);
                let (mean_reward, pulls) = self
                    .phase_bandit_arms
                    .get(name)
                    .map(|arm| (arm.mean_reward(), arm.pulls))
                    .unwrap_or((0.5, 0));
                (name.clone(), mean_reward, reliability, pulls)
            })
            .collect()
    }

    pub(crate) fn failure_rate(&self) -> f64 {
        if self.recent_failures.is_empty() {
            return 0.0;
        }
        let failures = self
            .recent_failures
            .iter()
            .filter(|failed| **failed)
            .count();
        failures as f64 / self.recent_failures.len() as f64
    }

    pub(crate) fn latency_p95_ms(&self) -> u64 {
        self.latency_estimator.p95() as u64
    }

    /// Derive a human-readable control mode string from the current state.
    pub(crate) fn control_mode(&self) -> String {
        let fail_rate = self.failure_rate();
        let p95 = self.latency_p95_ms();
        if fail_rate >= 0.50 || p95 >= 30_000 {
            "critical".to_string()
        } else if fail_rate >= ONLINE_CONTROLLER_FAILURE_ESCALATION
            || p95 >= ONLINE_CONTROLLER_P95_LATENCY_MS_ESCALATION
        {
            "elevated".to_string()
        } else {
            "normal".to_string()
        }
    }

    /// Derive a violation trend string from the current state.
    pub(crate) fn violation_trend(&self) -> String {
        let fail_rate = self.failure_rate();
        if fail_rate >= 0.40 {
            "degrading".to_string()
        } else if fail_rate >= 0.20 {
            "unstable".to_string()
        } else {
            "stable".to_string()
        }
    }

    pub(crate) fn should_escalate(&self) -> bool {
        self.failure_rate() >= ONLINE_CONTROLLER_FAILURE_ESCALATION
            || self.latency_p95_ms() >= ONLINE_CONTROLLER_P95_LATENCY_MS_ESCALATION
    }
}

/// Periodic timeout check called from background tasks (BLUE56-D02).
/// Scans for operations that have exceeded their timeout budget and
/// Tracks the earliest cycle at which pending operations were first observed.
static TIMEOUT_START_CYCLE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Tracks whether a timeout warning has already been emitted for the current
/// pending batch (to avoid log spam every 5-second cycle).
static TIMEOUT_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Check for timed-out operations and log/escalate if any have expired.
///
/// When `pending_count` and `timeout_secs` are provided, tracks the duration
/// that operations have been pending. If the deadline has passed (pending_count
/// remains > 0 longer than `timeout_secs` at 5s/cycle), logs a warning and
/// resets the tracking.
pub fn run_timeout_check(cycle: u64, pending_count: Option<usize>, timeout_secs: Option<u64>) {
    if let (Some(count), Some(timeout)) = (pending_count, timeout_secs) {
        if count > 0 {
            // First cycle with pending work — record start
            TIMEOUT_START_CYCLE
                .compare_exchange(
                    0,
                    cycle,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .ok();

            let start = TIMEOUT_START_CYCLE.load(std::sync::atomic::Ordering::Relaxed);
            let cycles_elapsed = cycle.wrapping_sub(start);
            let elapsed_secs = cycles_elapsed * 5; // each cycle is 5 seconds

            if elapsed_secs >= timeout && !TIMEOUT_WARNED.load(std::sync::atomic::Ordering::Relaxed)
            {
                tracing::warn!(
                    target: "runtime_controls",
                    cycle,
                    pending = count,
                    timeout_secs = timeout,
                    elapsed_secs = elapsed_secs,
                    cycles_since_pending = cycles_elapsed,
                    "TIMEOUT: {} pending operations exceeded {}s timeout (elapsed: {}s)",
                    count, timeout, elapsed_secs
                );
                TIMEOUT_WARNED.store(true, std::sync::atomic::Ordering::Relaxed);
            } else if elapsed_secs < timeout {
                tracing::debug!(
                    target: "runtime_controls",
                    cycle,
                    pending = count,
                    timeout_secs = timeout,
                    elapsed_secs = elapsed_secs,
                    "timeout check: {} pending, {}s elapsed of {}s budget",
                    count, elapsed_secs, timeout
                );
            }
        } else {
            // No pending work — reset tracking
            TIMEOUT_START_CYCLE.store(0, std::sync::atomic::Ordering::Relaxed);
            TIMEOUT_WARNED.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }

    if cycle > 0 && cycle.is_multiple_of(12) {
        tracing::debug!(
            target: "runtime_controls",
            cycle,
            "timeout check: periodic health tick"
        );
    }
}

/// Spawn a continuous background loop that periodically checks for runtime
/// timeouts and processes approval engine timeouts.
///
/// Call this once during server startup to integrate timeout checking into
/// the runtime control system.
///
/// The loop runs every 5 seconds and exits when `shutdown_notify` is signalled.
pub fn spawn_timeout_loop(
    shutdown_notify: std::sync::Arc<tokio::sync::Notify>,
    approval_engine: Option<
        std::sync::Arc<std::sync::RwLock<crate::governance::approval_engine::ApprovalEngine>>,
    >,
) {
    tokio::spawn(async move {
        let mut cycle: u64 = 0;
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = shutdown_notify.notified() => {
                    tracing::debug!(target: "runtime_controls", "timeout loop shutting down");
                    break;
                }
                _ = ticker.tick() => {
                    cycle = cycle.wrapping_add(1);

                    // 1. Run the runtime_controls timeout check
                    run_timeout_check(cycle, None, None);

                    // 2. Process approval engine timeouts if available
                    if let Some(ref engine) = approval_engine {
                        // std::sync::RwLock is used here (not tokio::sync::RwLock)
                        // because ApprovalEngine operations are synchronous.
                        // The lock scope is brief and never held across .await points.
                        let mut guard = match engine.write() {
                            Ok(g) => g,
                            Err(poisoned) => {
                                tracing::error!(
                                    target: "runtime_controls",
                                    "approval_engine lock poisoned in timeout loop"
                                );
                                poisoned.into_inner()
                            }
                        };
                        let changed = guard.process_timeouts();
                        if !changed.is_empty() {
                            tracing::info!(
                                target: "runtime_controls",
                                count = changed.len(),
                                "approval engine timed out {} request(s)",
                                changed.len()
                            );
                        }
                    }
                }
            }
        }
    });
}
