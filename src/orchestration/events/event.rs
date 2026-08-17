//! M4.1: agent-lifecycle event types — the observable event domain.
//!
//! [`AgentEvent`] is the named, cloneable data exchanged on the `EventBus`
//! (defined in `mod.rs`). Every variant is emitted somewhere in the pipeline
//! — the emission site is documented on each variant. No variant is dead.

/// An observable agent-lifecycle event.
///
/// Every variant is emitted somewhere in the pipeline — the emission site is
/// documented on each variant. No variant is dead.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A step boundary in the pipeline is about to be entered.
    ///
    /// Emitted from `acp::r#impl::chat::process_chat_request` with
    /// `step = "turn"`: the ACP chat turn is the pipeline's single clean
    /// step boundary today, so the whole turn is reported as one step.
    AgentPreStep { step: String },
    /// An agent request has been accepted and is starting.
    ///
    /// Emitted at the start of `process_chat_request` with the ACP trace's
    /// `request_id`.
    AgentRequest { request_id: String },
    /// A tool is about to execute (after argument validation and the
    /// governance gate, before the hook chain).
    ///
    /// A `Consume` verdict intercepts the call: the tool is reported as
    /// blocked and never reaches the hooks or the executor. Emitted from
    /// `orchestration::tool::executor::execute_single_tool`.
    ToolsPreExecute {
        tool_name: String,
        input: serde_json::Value,
    },
    /// A tool finished executing. `ok` is the tool's real outcome
    /// (`output.success`).
    ///
    /// Emitted from `execute_single_tool` only when execution actually ran
    /// (mirroring the scope of the post-execute `ToolHook`s); calls blocked
    /// by validation, governance, hooks, or a pre-execute `Consume` never
    /// emit it.
    ToolsPostExecute { tool_name: String, ok: bool },
    /// An agent turn is stopping: the response has been finalized and no
    /// further tool activity will occur for this request.
    ///
    /// Informational — the emitter ignores the verdict. Emitted at the end
    /// of `process_chat_request`.
    AgentTurnStopping { request_id: String },
}

/// A listener's decision for an [`AgentEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventVerdict {
    /// Keep dispatching to the remaining listeners.
    Continue,
    /// Stop the waterfall: later listeners are skipped and (for pre-execute
    /// events) the action is marked as intercepted.
    Consume,
}

/// Observer for agent-lifecycle events.
pub trait EventListener: Send + Sync {
    /// Handle one event, returning the verdict for the waterfall.
    fn on_event(&self, event: &AgentEvent) -> EventVerdict;
}
