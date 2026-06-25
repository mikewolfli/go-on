//! Progress reporter for streaming status hints.
//!
//! Provides phase-based progress tokens for SSE streams, allowing
//! consumers to track agent progress through the Think-Act-Observe cycle
//! in real time without parsing the actual content stream.

use crate::agent::StreamingSender;
use tracing::warn;

/// Phase-based progress tokens emitted into SSE streams.
pub const TOKEN_PHASE_PLANNING: &str = "__phase__:planning";
pub const TOKEN_PHASE_EXECUTING: &str = "__phase__:executing";
pub const TOKEN_PHASE_REFLECTING: &str = "__phase__:reflecting";
pub const TOKEN_PHASE_COMPLETE: &str = "__phase__:complete";

/// Prefix for numeric progress tokens: `__progress__:3/10` means step 3 of 10.
pub const TOKEN_PROGRESS_PREFIX: &str = "__progress__:";

/// Reports phase transitions and step progress over a streaming sender.
///
/// # Example
///
/// ```text
/// let mut reporter = ProgressReporter::new(sender, 10);
/// reporter.report_phase(TOKEN_PHASE_PLANNING);
/// for i in 1..=10 {
///     reporter.report_progress(i, 10);
/// }
/// reporter.report_complete();
/// ```
pub struct ProgressReporter {
    /// The phase we are currently in (one of the TOKEN_PHASE_* constants).
    current_phase: String,
    /// Total number of steps expected for the current phase.
    total_steps: u32,
    /// Current step counter (1-based).
    current_step: u32,
    /// The sender to emit tokens through. `None` if streaming is disabled.
    sender: Option<StreamingSender>,
}

impl ProgressReporter {
    /// Create a new progress reporter.
    ///
    /// `sender` may be `None` to disable all output (no-op reporter).
    /// `total_steps` is the expected number of steps; set to 0 if unknown.
    pub fn new(sender: Option<StreamingSender>, total_steps: u32) -> Self {
        Self {
            current_phase: String::new(),
            total_steps,
            current_step: 0,
            sender,
        }
    }

    /// Emit a phase transition token.
    ///
    /// Only emits if the phase has changed since the last call. The step
    /// counter is reset when entering a new phase.
    pub fn report_phase(&mut self, phase: &str) {
        if phase == self.current_phase {
            return;
        }
        self.current_phase = phase.to_string();
        self.current_step = 0;
        self.emit(phase);
    }

    /// Emit a numeric progress token: `__progress__:step/total`.
    ///
    /// Step is 1-based. Only emits when step increases and `total` is > 0.
    pub fn report_progress(&mut self, step: u32, total: u32) {
        if step <= self.current_step || total == 0 {
            return;
        }
        self.current_step = step;
        self.total_steps = total;
        let token = format!("{}{}/{}", TOKEN_PROGRESS_PREFIX, step, total);
        self.emit(&token);
    }

    /// Emit the completion token.
    pub fn report_complete(&mut self) {
        self.emit(TOKEN_PHASE_COMPLETE);
    }

    /// Returns true if the reporter has a live sender.
    pub fn is_active(&self) -> bool {
        self.sender.is_some()
    }

    /// Update the total step count (e.g., when a plan is refined).
    pub fn set_total_steps(&mut self, total: u32) {
        self.total_steps = total;
    }

    // ── private ──────────────────────────────────────────────────────────

    fn emit(&self, token: &str) {
        if let Some(ref sender) = self.sender {
            if let Err(e) = sender.send(token.to_string()) {
                warn!("ProgressReporter: failed to send token '{}': {}", token, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn phase_tokens_are_emitted_on_change() {
        let (tx, mut rx) = mpsc::channel::<String>(16);
        let sender = StreamingSender::new(tx);
        let mut reporter = ProgressReporter::new(Some(sender), 10);

        reporter.report_phase(TOKEN_PHASE_PLANNING);
        // Same phase again — should be a no-op.
        reporter.report_phase(TOKEN_PHASE_PLANNING);

        let mut tokens = Vec::new();
        while let Ok(t) = rx.try_recv() {
            tokens.push(t);
        }
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], TOKEN_PHASE_PLANNING);
    }

    #[test]
    fn progress_tokens_are_emitted_in_order() {
        let (tx, mut rx) = mpsc::channel::<String>(16);
        let sender = StreamingSender::new(tx);
        let mut reporter = ProgressReporter::new(Some(sender), 10);

        reporter.report_progress(3, 10);
        // Skipping backward — no-op.
        reporter.report_progress(2, 10);

        let mut tokens = Vec::new();
        while let Ok(t) = rx.try_recv() {
            tokens.push(t);
        }
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], "__progress__:3/10");
    }

    #[test]
    fn progress_resets_on_phase_change() {
        let (tx, mut rx) = mpsc::channel::<String>(16);
        let sender = StreamingSender::new(tx);
        let mut reporter = ProgressReporter::new(Some(sender), 10);

        reporter.report_progress(5, 10);
        reporter.report_phase(TOKEN_PHASE_EXECUTING);
        // After phase change, step 1 should emit (it is > current_step which is 0).
        reporter.report_progress(1, 10);

        let mut tokens = Vec::new();
        while let Ok(t) = rx.try_recv() {
            tokens.push(t);
        }
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], "__progress__:5/10");
        assert_eq!(tokens[1], TOKEN_PHASE_EXECUTING);
        assert_eq!(tokens[2], "__progress__:1/10");
    }

    #[test]
    fn complete_token_is_emitted() {
        let (tx, mut rx) = mpsc::channel::<String>(16);
        let sender = StreamingSender::new(tx);
        let mut reporter = ProgressReporter::new(Some(sender), 0);

        reporter.report_complete();

        let mut tokens = Vec::new();
        while let Ok(t) = rx.try_recv() {
            tokens.push(t);
        }
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], TOKEN_PHASE_COMPLETE);
    }

    #[test]
    fn noop_reporter_silently_discards_tokens() {
        let mut reporter: ProgressReporter = ProgressReporter::new(None, 0);
        reporter.report_phase(TOKEN_PHASE_PLANNING);
        reporter.report_progress(5, 10);
        reporter.report_complete();
        // No panics, nothing to assert beyond the call succeeding.
        assert!(!reporter.is_active());
    }
}
