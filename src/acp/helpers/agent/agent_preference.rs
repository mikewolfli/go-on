//! Agent preference resolution for chat requests
//!
//! This module encapsulates the **Agent Switch State & Preferred Agent Resolution**
//! logic that was previously inline in `process_chat_request`. It resolves
//! the configured primary agent, preferred agent from the request, manages
//! the agent switch state global, and computes conversation/branch/plan IDs.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex as StdMutex, OnceLock};

use anyhow::Result;

use crate::acp::r#impl::chat::ChatParams;
use crate::acp::server::AcpServer;
use crate::agent::Agent;
use crate::flow::ResolvedPhase;
use crate::flow::ResolvedRouting;
use crate::i18n::runtime::tf;
use crate::orchestration::task_router::{TaskCharacteristics, TaskType};
use crate::pua::PuaEnforcementPlan;
use crate::reinforcement::{RequirementContractArtifact, TaskPlanArtifact};

/// Default capacity for the agent switch state maps (per-phase entries).
/// Prevents unbounded memory growth when many distinct phases are used.
const AGENT_SWITCH_STATE_CAPACITY: usize = 10_000;

/// Insert a key-value pair into the map, evicting the oldest entry if at capacity.
/// This prevents unbounded memory growth when many distinct phase names appear.
fn map_insert_with_capacity<K: std::hash::Hash + Eq + Clone, V>(
    map: &mut std::collections::HashMap<K, V>,
    key: K,
    value: V,
    max_capacity: usize,
) {
    if map.len() >= max_capacity && !map.contains_key(&key) {
        // Evict the first (oldest) entry
        if let Some(oldest_key) = map.keys().next().cloned() {
            map.remove(&oldest_key);
        }
    }
    map.insert(key, value);
}

/// Result of resolving agent preferences for a chat request.
///
/// Captures all outputs produced by `resolve_agent_preferences()`:
/// configured primary agent, preferred agent from request, conversation ID,
/// branch ID, requirement contract, and task plan artifact.
pub struct AgentPreferenceResult {
    /// Primary agent from phase config's first entry or first runtime-resolved agent.
    pub configured_primary_agent: Option<String>,
    /// Explicit `preferred_agent` value from `params.options.extra`, if present.
    pub preferred_agent_from_request: Option<String>,
    /// Resolved conversation ID (with optional tenant namespace when user auth is enabled).
    pub conversation_id: String,
    /// Resolved branch ID (defaults to `"main"` when absent).
    pub branch_id: String,
    /// Requirement contract (from `params.requirement_contract` or a newly-created default).
    pub requirement_contract: RequirementContractArtifact,
    /// Task plan artifact (from `params.plan` or a newly-created default).
    pub plan: TaskPlanArtifact,
}

// ── Agent Switch State (global, process-wide) ────────────────────────────

#[derive(Default)]
struct AgentSwitchState {
    forced_agent_by_phase: HashMap<String, String>,
    primary_agent_by_phase: HashMap<String, String>,
}

static AGENT_SWITCH_STATE: OnceLock<StdMutex<AgentSwitchState>> = OnceLock::new();

fn agent_switch_state() -> &'static StdMutex<AgentSwitchState> {
    AGENT_SWITCH_STATE.get_or_init(|| StdMutex::new(AgentSwitchState::default()))
}

// ── Agent reordering helper ──────────────────────────────────────────────

/// Move the named agent to the front of the agent list.
///
/// Returns `true` if the agent was found and reordered, `false` if the name
/// was not present in the list.
fn reorder_agents_with_priority(
    agents: &mut Vec<(String, Arc<dyn Agent>)>,
    preferred: &str,
) -> bool {
    if let Some(index) = agents.iter().position(|(name, _)| name == preferred) {
        if index > 0 {
            let selected = agents.remove(index);
            agents.insert(0, selected);
        }
        return true;
    }
    false
}

// ── Default requirement contract helper ──────────────────────────────────

/// Create a default requirement contract with minimal fields.
fn default_requirement_contract(task: &str, source: &str) -> RequirementContractArtifact {
    RequirementContractArtifact {
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
        task: task.to_string(),
        source: source.to_string(),
        goal: String::new(),
        scope: String::new(),
        non_goals: Vec::new(),
        acceptance_criteria: Vec::new(),
        constraints: Vec::new(),
        open_questions: Vec::new(),
        ambiguity_score: 0,
        user_confirmed: false,
    }
}

/// Resolve agent preferences, switch state, conversation/branch IDs, and plan artifacts.
///
/// This function encapsulates the **Agent Switch State & Preferred Agent Resolution**
/// logic, including:
///
/// 1. Resolving `configured_primary_agent` from phase config or first runtime agent
/// 2. Reading `preferred_agent_from_request` from `params.options.extra`
/// 3. Managing `agent_switch_state()` global — forced/primary agent by phase
/// 4. Priority logic:
///    - Explicit `preferred_agent` → persist as forced
///    - Stored forced fallback → probe primary first
/// 5. Phase-level rate limiter check (RPM/burst)
/// 6. Resolving `conversation_id` (with optional tenant namespace)
/// 7. Resolving `branch_id`, `requirement_contract`, and `_plan` (TaskPlanArtifact)
///
/// # Side-effects
/// - Modifies `resolved.agents` ordering via reorder_agents_with_priority
/// - Updates the global `AGENT_SWITCH_STATE` (forced/primary agent by phase)
/// - May short-circuit with `anyhow::bail!` if the phase rate limiter rejects the request
pub fn resolve_agent_preferences(
    server: &AcpServer,
    params: &ChatParams,
    phase: &ResolvedPhase,
    resolved: &mut ResolvedRouting,
    tenant_id: &str,
) -> Result<AgentPreferenceResult> {
    let phase_name = &phase.phase_name;

    // ── 1. Resolve configured_primary_agent ──────────────────────────────
    // Path A: explicit config list → use first configured name.
    // Path B: auto-map (empty config list) → fall back to first runtime-resolved agent name.
    let configured_primary_agent = phase
        .agent_names
        .first()
        .cloned()
        .or_else(|| resolved.agents.first().map(|(name, _)| name.clone()));

    // ── 2. Read preferred_agent_from_request ─────────────────────────────
    let preferred_agent_from_request = params
        .options
        .as_ref()
        .and_then(|opts| opts.extra.get("preferred_agent"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    // ── 3. Update primary agent by phase in global state ─────────────────
    if let Some(primary) = configured_primary_agent.as_ref() {
        if let Ok(mut state) = agent_switch_state().lock() {
            map_insert_with_capacity(
                &mut state.primary_agent_by_phase,
                phase_name.clone(),
                primary.clone(),
                AGENT_SWITCH_STATE_CAPACITY,
            );
        }
    }

    // ── 4. Priority logic ────────────────────────────────────────────────
    // 1) If request explicitly chooses preferred_agent, honor immediately
    //    and persist the choice as the forced agent for this phase.
    // 2) Otherwise, if the phase has a stored forced fallback, probe the
    //    primary agent first and then the forced agent (auto-recover strategy).
    if let Some(preferred) = preferred_agent_from_request.as_deref() {
        if reorder_agents_with_priority(&mut resolved.agents, preferred) {
            if let Ok(mut state) = agent_switch_state().lock() {
                map_insert_with_capacity(
                    &mut state.forced_agent_by_phase,
                    phase_name.clone(),
                    preferred.to_string(),
                    AGENT_SWITCH_STATE_CAPACITY,
                );
            }
        }
    } else if let Ok(state) = agent_switch_state().lock() {
        if let Some(forced) = state.forced_agent_by_phase.get(phase_name) {
            let primary = state.primary_agent_by_phase.get(phase_name);
            if let Some(primary_name) = primary {
                // Auto-recover strategy: always probe primary first, then fallback agent.
                let _ = reorder_agents_with_priority(&mut resolved.agents, forced);
                let _ = reorder_agents_with_priority(&mut resolved.agents, primary_name);
            }
        }
    }

    // ── 5. Phase-level rate limiter check (RPM/burst) ────────────────────
    if let Some(options) = phase.options.as_ref() {
        let rpm_limit = options
            .extra
            .get("rate_limit_rpm")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::MAX);
        let burst = options
            .extra
            .get("rate_limit_burst")
            .and_then(|v| v.as_u64());
        if rpm_limit != u64::MAX {
            let allowed = server
                .phase_rate_limiter
                .lock()
                .map(|guard| guard.allow(phase_name, rpm_limit, burst))
                .unwrap_or_else(|e| {
                    tracing::warn!("rate limiter lock failed: {e}");
                    true
                });
            if !allowed {
                let burst_str = burst
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "none".to_string());
                anyhow::bail!(tf(
                    "error.chat.rate_limited",
                    &[
                        ("phase", phase_name),
                        ("rpm", &rpm_limit.to_string()),
                        ("burst", &burst_str),
                    ]
                ));
            }
        }
    }

    // ── 6. Resolve conversation_id (with optional tenant namespace) ─────
    let raw_conversation_id = params.conversation_id.clone().unwrap_or_else(|| {
        format!(
            "conv_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    });
    let conversation_id = if server.runtime_config.user_auth_enabled {
        format!("{}:{}", tenant_id, raw_conversation_id)
    } else {
        raw_conversation_id
    };

    // ── 7. Resolve branch_id, requirement_contract, and plan ─────────────
    let branch_id = params
        .branch_id
        .clone()
        .unwrap_or_else(|| "main".to_string());

    let requirement_contract = if let Some(contract) = &params.requirement_contract {
        contract.clone()
    } else {
        let task_description = crate::acp::r#impl::chat::extract_task_description(&params.messages);
        default_requirement_contract(&task_description, "chat")
    };

    let plan = if let Some(existing_plan) = &params.plan {
        existing_plan.clone()
    } else {
        TaskPlanArtifact {
            generated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            task: String::new(),
            characteristics: TaskCharacteristics {
                description: String::new(),
                task_type: TaskType::BugFix,
                complexity: 1,
                required_capabilities: Vec::new(),
                involves_multiple_modules: false,
                is_time_critical: false,
                needs_verification: false,
                has_safety_concerns: false,
            },
            routing: crate::orchestration::task_router::RoutingDecision {
                roles: Vec::new(),
                requirements: Vec::new(),
                predicted_success_rate: 1.0,
                estimated_duration_seconds: 1000,
                can_parallelize: Vec::new(),
                risk_factors: Vec::new(),
                recommended_safeguards: Vec::new(),
                pua_enforcement: PuaEnforcementPlan {
                    escalation_level: String::new(),
                    mandatory_roles: Vec::new(),
                    red_lines: Vec::new(),
                    quality_compass: Vec::new(),
                    mandatory_safeguards: Vec::new(),
                    mandatory_evidence: Vec::new(),
                    stage_requirements: Vec::new(),
                },
            },
            decomposition: None,
            planned_subtasks: Vec::new(),
            sub_agent_recommended: false,
            activation_reasons: Vec::new(),
            action_checks_required: Vec::new(),
        }
    };

    Ok(AgentPreferenceResult {
        configured_primary_agent,
        preferred_agent_from_request,
        conversation_id,
        branch_id,
        requirement_contract,
        plan,
    })
}
