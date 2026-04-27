//! Runtime controls — online adaptive control, sliding window, bandit-based phase selection.
//!
//! # Status
//! Complete implementation ready for CapabilityBus integration (ARCH-13).
//! Currently zero-call — all items are intentionally public for future wiring.

use std::collections::{HashMap, VecDeque};

const ONLINE_CONTROLLER_WINDOW: usize = 64;
const ONLINE_CONTROLLER_FAILURE_ESCALATION: f64 = 0.25;
const ONLINE_CONTROLLER_P95_LATENCY_MS_ESCALATION: u64 = 15_000;
const ONLINE_CONTROLLER_MIN_AGENT_SAMPLES: u64 = 3;
const ONLINE_CONTROLLER_BANDIT_EXPLORATION: f64 = 1.4;

#[derive(Debug, Default, Clone)]
struct AgentSignalWindow {
    recent_failures: VecDeque<bool>,
    recent_latency_ms: VecDeque<u64>,
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

        if self.recent_latency_ms.len() >= ONLINE_CONTROLLER_WINDOW {
            self.recent_latency_ms.pop_front();
        }
        self.recent_latency_ms.push_back(duration_ms);
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
        if self.recent_latency_ms.is_empty() {
            return 0;
        }
        let mut samples = self.recent_latency_ms.iter().copied().collect::<Vec<_>>();
        samples.sort_unstable();
        percentile(&samples, 95.0)
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
    recent_latency_ms: VecDeque<u64>,
    agent_windows: HashMap<String, AgentSignalWindow>,
    phase_agent_windows: HashMap<String, AgentSignalWindow>,
    phase_windows: HashMap<String, AgentSignalWindow>,
    phase_bandit_arms: HashMap<String, PhaseBanditArm>,
    phase_bandit_total_pulls: u64,
}

impl OnlineControllerState {
    fn phase_agent_key(phase_name: &str, agent_name: &str) -> String {
        format!("{}::{}", phase_name, agent_name)
    }

    pub(crate) fn record(&mut self, success: bool, duration_ms: u64) {
        if self.recent_failures.len() >= ONLINE_CONTROLLER_WINDOW {
            self.recent_failures.pop_front();
        }
        self.recent_failures.push_back(!success);

        if self.recent_latency_ms.len() >= ONLINE_CONTROLLER_WINDOW {
            self.recent_latency_ms.pop_front();
        }
        self.recent_latency_ms.push_back(duration_ms);
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

    pub(crate) fn rank_agent_names_for_phase(
        &self,
        phase_name: &str,
        agent_names: &[String],
    ) -> Vec<(String, f64)> {
        let mut scored = agent_names
            .iter()
            .enumerate()
            .map(|(idx, name)| {
                (
                    idx,
                    name.clone(),
                    self.agent_reliability_score(phase_name, name),
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
        if self.recent_latency_ms.is_empty() {
            return 0;
        }
        let mut samples = self.recent_latency_ms.iter().copied().collect::<Vec<_>>();
        samples.sort_unstable();
        percentile(&samples, 95.0)
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

fn percentile(samples: &[u64], percentile: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let clamped = percentile.clamp(0.0, 100.0);
    let rank = ((clamped / 100.0) * ((samples.len() - 1) as f64)).round() as usize;
    samples[rank]
}
