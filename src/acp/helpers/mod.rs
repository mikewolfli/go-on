//! Helper modules for ACP server
//!
//! This module contains utility functions and data structures used throughout
//! the ACP server implementation. Files are organized into subdirectories by
//! domain, but all modules are re-exported at this level via `#[path]` to
//! maintain backward compatibility with existing import paths.
//!
//! # Directory layout
//!
//! ```text
//! helpers/
//! ├── agent/          # Agent selection, routing, options, preferences
//! ├── autonomy/       # Autonomy loop, executor, metrics
//! ├── governance/     # Policies, review gates, voting, tool governance
//! ├── planning/       # Planning, orchestration alignment, context
//! ├── response/       # Response assembly and finalization
//! ├── diagnosis/      # Repair and autonomy gate diagnosis
//! ├── requirement/    # Requirement contracts and continuation
//! └── (root)         # General-purpose helpers
//! ```

// ── Agent selection & routing ──────────────────────────────────────────────
#[path = "agent/agent_options.rs"]
pub mod agent_options;
#[path = "agent/agent_preference.rs"]
pub mod agent_preference;
#[path = "agent/agent_router.rs"]
pub mod agent_router;
#[path = "agent/agent_selector.rs"]
pub mod agent_selector;
#[path = "agent/capability_selector.rs"]
pub mod capability_selector;
#[path = "agent/model_router.rs"]
pub mod model_router;

// ── Autonomy loop & execution ──────────────────────────────────────────────
#[path = "autonomy/autonomy.rs"]
pub mod autonomy;
#[path = "autonomy/autonomy_executor.rs"]
pub mod autonomy_executor;
#[path = "autonomy/autonomy_loop.rs"]
pub mod autonomy_loop;
#[path = "autonomy/autonomy_loop_adapter.rs"]
pub mod autonomy_loop_adapter;
#[path = "autonomy/autonomy_metrics.rs"]
pub mod autonomy_metrics;
#[path = "autonomy/execution_intelligence.rs"]
pub mod execution_intelligence;
#[path = "autonomy/full_auto_executor.rs"]
pub mod full_auto_executor;
#[path = "autonomy/intelligence_bridge.rs"]
pub mod intelligence_bridge;

// ── Governance & policy ────────────────────────────────────────────────────
#[path = "governance/policy.rs"]
pub mod policy;
#[path = "governance/pre_route_policy.rs"]
pub mod pre_route_policy;
#[path = "governance/review_gate.rs"]
pub mod review_gate;
#[path = "governance/tool_governance.rs"]
pub mod tool_governance;
#[path = "governance/tool_governance_defaults.rs"]
pub mod tool_governance_defaults;
#[path = "governance/vote_executor.rs"]
pub mod vote_executor;
#[path = "governance/vote_orchestration.rs"]
pub mod vote_orchestration;

// ── Planning & orchestration ───────────────────────────────────────────────
#[path = "planning/context.rs"]
pub mod context;
#[path = "planning/council_deliberation.rs"]
pub mod council_deliberation;
#[path = "planning/orchestration_alignment.rs"]
pub mod orchestration_alignment;
#[path = "planning/phase_resolver.rs"]
pub mod phase_resolver;
#[path = "planning/planner_bridge.rs"]
pub mod planner_bridge;

// ── Response assembly ──────────────────────────────────────────────────────
#[path = "response/response_assembler.rs"]
pub mod response_assembler;
#[path = "response/response_finalizer.rs"]
pub mod response_finalizer;

// ── Diagnosis & repair ─────────────────────────────────────────────────────
#[path = "diagnosis/auton_gate_diagnosis.rs"]
pub mod auton_gate_diagnosis;
#[path = "diagnosis/repair_diagnosis.rs"]
pub mod repair_diagnosis;

// ── Requirement contracts ──────────────────────────────────────────────────
#[path = "requirement/requirement.rs"]
pub mod requirement;
#[path = "requirement/requirement_continuation.rs"]
pub mod requirement_continuation;

// ── General-purpose helpers (root) ─────────────────────────────────────────
pub mod cache_strategy;
pub mod conversation;
pub mod fallback_executor;
pub mod idempotency_resume;
pub mod metrics;
pub mod misc;

// Re-export for convenience
