//! Cognitive-loop phases for `process_chat_request` (BLUE62 ARCH-1).
//!
//! Breaks the monolithic request pipeline into four phases, each a separate
//! single-responsibility module (split out of the former `chat_phases.rs`,
//! M0.4):
//!   1. [`observe`] — input validation, multimodal detection, prompt injection
//!      check, context gathering, memory recall, capability sensing;
//!   2. [`think`]   — model resolution, agent selection, routing, planning,
//!      capability analysis, risk assessment, metacognitive evaluation;
//!   3. [`act`]     — LLM calls, tool execution, autonomy loop, fallback, vote,
//!      cache operations, scheduler;
//!   4. [`reflect`] — response assembly, error handling, knowledge persistence,
//!      metacognitive updates, threshold learning, capability bus feedback,
//!      BrainLoop reflection.
//!
//! The shared phase result types live in [`types`].

pub(crate) mod act;
pub(crate) mod observe;
pub(crate) mod reflect;
pub(crate) mod think;
pub(crate) mod types;

pub(crate) use act::act_phase;
pub(crate) use observe::observe_phase;
pub(crate) use reflect::reflect_phase;
pub(crate) use think::think_phase;
pub(crate) use types::{ActOutput, ObserveOutput, ThinkOutput};
