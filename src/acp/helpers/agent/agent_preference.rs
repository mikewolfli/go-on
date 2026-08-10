//! Agent preference resolution for chat requests
//!
//! This module encapsulates the **Agent Switch State & Preferred Agent Resolution**
//! logic that was previously inline in `process_chat_request`. It resolves
//! the configured primary agent, preferred agent from the request, manages
//! the agent switch state global, and computes conversation/branch/plan IDs.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use anyhow::Result;

use crate::acp::r#impl::chat::ChatParams;
use crate::acp::server::AcpServer;
use crate::flow::ResolvedPhase;
use crate::flow::ResolvedRouting;
use crate::i18n::runtime::tf;

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
/// configured primary agent, conversation ID, branch ID, requirement
/// contract, and task plan artifact.
pub struct AgentPreferenceResult {
    /// Primary agent from phase config's first entry or first runtime-resolved agent.
    pub configured_primary_agent: Option<String>,
    /// Resolved conversation ID (with optional tenant namespace when user auth is enabled).
    pub conversation_id: String,
    /// Resolved branch ID (defaults to `"main"` when absent).
    pub branch_id: String,
}

// ── Agent Switch State (global, process-wide) ────────────────────────────

#[derive(Default)]
pub(crate) struct AgentSwitchState {
    pub(crate) forced_agent_by_phase: HashMap<String, String>,
    pub(crate) primary_agent_by_phase: HashMap<String, String>,
}

static AGENT_SWITCH_STATE: OnceLock<RwLock<AgentSwitchState>> = OnceLock::new();

pub(crate) fn agent_switch_state() -> &'static RwLock<AgentSwitchState> {
    AGENT_SWITCH_STATE.get_or_init(|| RwLock::new(AgentSwitchState::default()))
}

/// Only available in non-Postgres profiles because the caller is gated.
#[cfg(all(test, not(feature = "backend-postgres")))]
pub(crate) fn reset_agent_switch_state_for_test() {
    if let Some(state) = AGENT_SWITCH_STATE.get() {
        if let Ok(mut guard) = state.write() {
            guard.forced_agent_by_phase.clear();
            guard.primary_agent_by_phase.clear();
        }
    }
}

// ── Agent reordering helper ──────────────────────────────────────────────
// NOTE: `reorder_agents_with_priority` lives once in
// crate::acp::r#impl::chat (moved here after dedup); this module calls it.

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
/// 7. Resolving `branch_id`
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
        let mut state = agent_switch_state().write().unwrap_or_else(|poisoned| {
            tracing::warn!("agent_switch_state lock poisoned — primary_agent_by_phase");
            poisoned.into_inner()
        });
        map_insert_with_capacity(
            &mut state.primary_agent_by_phase,
            phase_name.clone(),
            primary.clone(),
            AGENT_SWITCH_STATE_CAPACITY,
        );
    }

    // ── 4. Priority logic ────────────────────────────────────────────────
    // 1) If request explicitly chooses preferred_agent, honor immediately
    //    and persist the choice as the forced agent for this phase.
    // 2) Otherwise, if the phase has a stored forced fallback, probe the
    //    primary agent first and then the forced agent (auto-recover strategy).
    if let Some(preferred) = preferred_agent_from_request.as_deref() {
        if crate::acp::r#impl::chat::reorder_agents_with_priority(&mut resolved.agents, preferred) {
            let mut state = agent_switch_state().write().unwrap_or_else(|poisoned| {
                tracing::warn!("agent_switch_state lock poisoned — forced_agent_by_phase");
                poisoned.into_inner()
            });
            map_insert_with_capacity(
                &mut state.forced_agent_by_phase,
                phase_name.clone(),
                preferred.to_string(),
                AGENT_SWITCH_STATE_CAPACITY,
            );
        }
    } else {
        let state = agent_switch_state().read().unwrap_or_else(|poisoned| {
            tracing::warn!("agent_switch_state lock poisoned — forced agent lookup");
            poisoned.into_inner()
        });
        if let Some(forced) = state.forced_agent_by_phase.get(phase_name) {
            let primary = state.primary_agent_by_phase.get(phase_name);
            if let Some(primary_name) = primary {
                // Auto-recover strategy: always probe primary first, then fallback agent.
                let _ = crate::acp::r#impl::chat::reorder_agents_with_priority(
                    &mut resolved.agents,
                    forced,
                );
                let _ = crate::acp::r#impl::chat::reorder_agents_with_priority(
                    &mut resolved.agents,
                    primary_name,
                );
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
                .resilience
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

    // ── 7. Resolve branch_id ───────────────────────────────────────
    let branch_id = params
        .branch_id
        .clone()
        .unwrap_or_else(|| "main".to_string());

    Ok(AgentPreferenceResult {
        configured_primary_agent,
        conversation_id,
        branch_id,
    })
}
