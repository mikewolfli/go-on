use std::collections::{HashMap, VecDeque};

const ONLINE_CONTROLLER_WINDOW: usize = 64;
const ONLINE_CONTROLLER_FAILURE_ESCALATION: f64 = 0.25;
const ONLINE_CONTROLLER_P95_LATENCY_MS_ESCALATION: u64 = 15_000;
const ONLINE_CONTROLLER_MIN_AGENT_SAMPLES: u64 = 3;

#[derive(Debug, Default, Clone)]
struct AgentSignalWindow {
    recent_failures: VecDeque<bool>,
    recent_latency_ms: VecDeque<u64>,
    attempts: u64,
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
pub(crate) struct OnlineControllerState {
    recent_failures: VecDeque<bool>,
    recent_latency_ms: VecDeque<u64>,
    agent_windows: HashMap<String, AgentSignalWindow>,
    phase_agent_windows: HashMap<String, AgentSignalWindow>,
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
