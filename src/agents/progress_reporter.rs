//! Progress reporter for streaming status hints.
//!
//! Provides phase-based progress tokens for SSE streams, allowing
//! consumers to track agent progress through the Think-Act-Observe cycle
//! in real time without parsing the actual content stream.
//!
//! Supports two transport backends:
//! - `StreamingSender` (string channel, used by BrainLoop / CLI chat)
//! - `StreamFrame` sender (SSE frame channel, used by ACP autonomy loop)

use crate::acp::r#impl::chat::streaming::StreamFrame;
use crate::agent::StreamingSender;
use tokio::sync::mpsc;
use tracing::warn;

/// Phase-based progress tokens emitted into SSE streams.
pub const TOKEN_PHASE_PLANNING: &str = "__phase__:planning";
pub const TOKEN_PHASE_EXECUTING: &str = "__phase__:executing";
pub const TOKEN_PHASE_REFLECTING: &str = "__phase__:reflecting";
pub const TOKEN_PHASE_COMPLETE: &str = "__phase__:complete";

/// Prefix for numeric progress tokens: `__progress__:3/10` means step 3 of 10.
pub const TOKEN_PROGRESS_PREFIX: &str = "__progress__:";

/// Dual-transport progress sender.
///
/// Supports both string-based streaming (used by BrainLoop / CLI chat)
/// and SSE-frame-based streaming (used by ACP autonomy loop).
#[allow(dead_code)]
pub(crate) enum ProgressSender {
    /// String channel — tokens are raw strings consumed by BrainLoop.
    Streaming(StreamingSender),
    /// SSE frame channel — tokens are wrapped in status StreamFrames.
    StreamFrame(mpsc::UnboundedSender<StreamFrame>),
}

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
    sender: Option<ProgressSender>,
}

impl ProgressReporter {
    /// Create a new progress reporter with a string-based sender.
    ///
    /// `sender` may be `None` to disable all output (no-op reporter).
    /// `total_steps` is the expected number of steps; set to 0 if unknown.
    pub fn new(sender: Option<StreamingSender>, total_steps: u32) -> Self {
        Self {
            current_phase: String::new(),
            total_steps,
            current_step: 0,
            sender: sender.map(ProgressSender::Streaming),
        }
    }

    /// Create a new progress reporter with an SSE-frame-based sender.
    ///
    /// When this variant is used, progress tokens are wrapped as
    /// `StreamFrame { event: "status", payload: { "message": token } }`
    /// so they can be forwarded directly through the ACP SSE pipeline.
    #[allow(dead_code)]
    pub(crate) fn with_stream_frame(
        sender: Option<mpsc::UnboundedSender<StreamFrame>>,
        total_steps: u32,
    ) -> Self {
        Self {
            current_phase: String::new(),
            total_steps,
            current_step: 0,
            sender: sender.map(ProgressSender::StreamFrame),
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

    /// Return the current phase string (for external inspection).
    pub fn current_phase(&self) -> &str {
        &self.current_phase
    }

    /// Return the current step number (1-based).
    pub fn current_step(&self) -> u32 {
        self.current_step
    }

    /// Return the total steps.
    pub fn total_steps(&self) -> u32 {
        self.total_steps
    }

    // ── private ──────────────────────────────────────────────────────────

    fn emit(&self, token: &str) {
        match &self.sender {
            Some(ProgressSender::Streaming(sender)) => {
                if let Err(e) = sender.send(token.to_string()) {
                    warn!("ProgressReporter: failed to send token '{}': {}", token, e);
                }
            }
            Some(ProgressSender::StreamFrame(sender)) => {
                if let Err(e) = sender.send(StreamFrame {
                    event: "status",
                    payload: serde_json::json!({"message": token}),
                    status: None,
                }) {
                    warn!(
                        "ProgressReporter: failed to send StreamFrame '{}': {}",
                        token, e
                    );
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn phase_tokens_are_emitted_on_change() {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
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
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
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
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
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
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
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

    #[test]
    fn stream_frame_sender_emits_status_events() {
        let (tx, mut rx) = mpsc::unbounded_channel::<StreamFrame>();
        let mut reporter = ProgressReporter::with_stream_frame(Some(tx), 5);

        reporter.report_phase(TOKEN_PHASE_PLANNING);
        reporter.report_progress(1, 5);

        // Drain receiver
        let mut frames = Vec::new();
        while let Ok(f) = rx.try_recv() {
            frames.push(f);
        }
        // Should have 2 frames: one for phase, one for progress
        assert_eq!(frames.len(), 2);
        // First frame should be a status event
        assert_eq!(frames[0].event, "status");
        assert_eq!(frames[0].payload["message"], TOKEN_PHASE_PLANNING);
    }

    #[test]
    fn stream_frame_noop_when_none() {
        let mut reporter = ProgressReporter::with_stream_frame(None, 5);
        reporter.report_phase(TOKEN_PHASE_PLANNING);
        reporter.report_progress(1, 5);
        reporter.report_complete();
        assert!(!reporter.is_active());
    }
}
