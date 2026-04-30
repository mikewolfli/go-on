import { invokeRuntimeRpc } from "./bridge";

export interface GovernanceStatusResult {
  governance?: {
    schema_version?: string;
    artifact_contract?: {
      schema_version?: string;
      compatibility?: string;
      source?: string;
      companion?: {
        release_readiness_schema_version?: string;
      };
    };
    dual_track_consistency?: {
      ready?: boolean;
      issues?: string[];
      governance_schema_version?: string;
      readiness_schema_version?: string;
    };
    status?: string;
    rules?: {
      version?: string;
      files?: Array<{ path?: string; size_bytes?: number }>;
    };
    config?: {
      production_strict?: boolean;
      entry_auth_enabled?: boolean;
      entry_auth_key_configured?: boolean;
      entry_rate_limit_rpm?: number;
      entry_rate_limit_burst?: number;
      warning_count?: number;
      strict_violation_count?: number;
    };
    dynamic_rules?: {
      red_line_count?: number;
      stage_requirement_count?: number;
      quality_compass_count?: number;
    };
    violations?: {
      pua_recent_failed?: number;
      breaker_open_count?: number;
    };
    runtime?: {
      is_healthy?: boolean;
    };
    multi_user_server?: {
      mode?: string;
      inference?: {
        source?: string;
        deployment_target?: string;
        requested_server_mode?: string;
      };
      tenant_context?: {
        tenant_id_required?: boolean;
        cross_tenant_access_denied_by_default?: boolean;
        default_tenant_scope?: string;
      };
      components?: {
        authn_authz?: { status?: string };
        data_execution_isolation?: { status?: string };
        resource_quota?: { status?: string };
        audit_forensics?: { status?: string };
        lifecycle_ops?: { status?: string };
      };
      lifecycle?: {
        ready?: boolean;
        backup_restore_ready?: boolean;
        freeze_unfreeze_ready?: boolean;
        deprovision_cleanup_ready?: boolean;
        blocking_issues?: string[];
        runbook_version?: string;
      };
      release_gate?: {
        ready?: boolean;
        blocking_issues?: string[];
        bundle_version?: string;
        environment?: string;
      };
    };
  };
}

export interface GovernanceAuditEvent {
  timestamp?: number;
  action?: string;
  actor?: string;
  result?: string;
  detail?: {
    escalation_level?: string;
  };
}

export interface GovernanceAuditRecentResult {
  audit?: {
    events?: GovernanceAuditEvent[];
  };
}

export interface ProviderStatusResult {
  provider_status?: {
    status?: string;
    message?: string;
    summary?: {
      ready?: number;
      degraded?: number;
      configured?: number;
      registry?: number;
      coverage_percent?: number;
    };
    configured_agents?: Array<{
      agent?: string;
      ready?: boolean;
      endpoint_status?: string;
      missing_envs?: string[];
    }>;
    registry_catalog?: Array<{
      agent?: string;
      default_model?: string;
      available_models?: number;
    }>;
    timestamp?: number;
  };
}

export interface ReleaseReadinessResult {
  readiness?: {
    version?: string;
    schema_version?: string;
    artifact_contract?: {
      schema_version?: string;
      compatibility?: string;
      source?: string;
      companion?: {
        governance_schema_version?: string;
      };
    };
    dual_track_consistency?: {
      ready?: boolean;
      issues?: string[];
      schema_consistent?: boolean;
      summary_detail_mode_consistent?: boolean;
      summary_detail_gate_consistent?: boolean;
      summary_detail_lifecycle_consistent?: boolean;
      summary_detail_inference_source_consistent?: boolean;
    };
    status?: string;
    overall_pass?: boolean;
    blocked_gate_count?: number;
    blocked_gate_names?: string[];
    gates?: Array<{
      name?: string;
      passed?: boolean;
    }>;
    summary?: {
      multi_user_mode?: string;
      multi_user_gate_ready?: boolean;
      multi_user_lifecycle_ready?: boolean;
      multi_user_inference_source?: string;
      dual_track_consistency_ready?: boolean;
    };
    multi_user_server?: {
      mode?: string;
      inference?: {
        source?: string;
        deployment_target?: string;
        requested_server_mode?: string;
      };
      release_gate_ready?: boolean;
      entry_auth_enabled?: boolean;
      entry_auth_key_configured?: boolean;
      production_strict_enabled?: boolean;
      lifecycle?: {
        ready?: boolean;
        backup_restore_ready?: boolean;
        freeze_unfreeze_ready?: boolean;
        deprovision_cleanup_ready?: boolean;
        blocking_issues?: string[];
        runbook_version?: string;
      };
      dual_track_consistency?: {
        ready?: boolean;
        issues?: string[];
      };
    };
  };
}

export interface HealthProbesResult {
  probes?: {
    liveness?: {
      ok?: boolean;
      status?: string;
      shutting_down?: boolean;
      uptime_seconds?: number;
    };
    readiness?: {
      ok?: boolean;
      status?: string;
      overall_status?: string;
      generated_at?: number;
    };
    summary?: {
      healthy?: number;
      warn?: number;
      error?: number;
      skipped?: number;
    };
    locks?: {
      status?: string;
      components_tracked?: number;
      poisoned_total?: number;
      recovered_total?: number;
      slow_wait_total?: number;
      max_wait_ms?: number;
    };
    timeouts?: {
      status?: string;
      agent_request_total?: number;
      review_gate_total?: number;
      runtime_probe_total?: number;
    };
    dependencies?: Array<{
      name?: string;
      status?: string;
      message?: string;
      details?: Record<string, unknown>;
    }>;
    circuit_breakers?: Array<{
      name?: string;
      state?: string;
      failure_count?: number;
      success_count?: number;
      last_state_change?: string | null;
      total_failures?: number;
      total_successes?: number;
    }>;
    rate_limiter?: {
      tracked?: number;
      buckets?: Array<{
        phase?: string;
        tokens?: number;
        capacity?: number;
        used_percent?: number;
      }>;
    };
    token_cache?: {
      l1?: { hits?: number; misses?: number };
      l2?: { hits?: number; misses?: number };
      l3?: { hits?: number; misses?: number };
      overall?: {
        hit_rate?: number;
        total_tokens_saved?: number;
        total_entries?: number;
      };
    };
    timestamp?: number;
  };
}

export interface RuntimeSelfModelResult {
  self_model?: {
    health?: HealthProbesResult["probes"];
    stability?: {
      score?: number;
      level?: string;
      safe_restart_ready?: boolean;
      summary?: {
        health_errors?: number;
        health_warnings?: number;
        config_warnings?: number;
        strict_violations?: number;
      };
      checks?: Array<{
        name?: string;
        status?: string;
        description?: string;
      }>;
      recommendation?: string;
      timestamp?: number;
    };
    drift?: {
      alert?: boolean;
      absolute_diff?: number;
      threshold?: number;
    };
    decision?: {
      recommended_mode?: string;
      fallback_triggered?: boolean;
      readiness_status?: string;
      stability_level?: string;
      safe_restart_ready?: boolean;
    };
    constraints?: {
      shutdown_requested?: boolean;
      health_errors?: number;
      health_warnings?: number;
      config_warnings?: number;
      strict_violations?: number;
    };
    meta_cognition?: {
      self_consistency_score?: number;
      goal_stability?: string;
      capability_boundary?: {
        known_limits?: string[];
        confidence_envelope?: string;
      };
      metacognitive_loop?: {
        active?: boolean;
        last_reflection?: string;
        reflection_trigger?: string;
      };
      world_model?: {
        runtime_state_known?: boolean;
        environment_stable?: boolean;
        adaptation_needed?: boolean;
      };
      schema_version?: string;
    };
    warnings?: unknown[];
    recommendations?: string[];
    source_methods?: string[];
    timestamp?: number;
    learning_profile?: Record<string, unknown>;
    knowledge_refinement?: Record<string, unknown>;
  };
}

export interface BreakerStatusResult {
  ok?: boolean;
  open_count?: number;
  degraded_count?: number;
  degraded_services?: Array<{
    name?: string;
    recommended_action?: string;
  }>;
  breakers?: Array<{
    name?: string;
    state?: string;
    failure_count?: number;
    success_count?: number;
    last_state_change?: string | null;
    total_failures?: number;
    total_successes?: number;
  }>;
}

export interface MetricsResult {
  total_requests?: number;
  successful_requests?: number;
  failed_requests?: number;
  avg_request_duration_ms?: number;
}

export interface TaskPlanResult {
  ok?: boolean;
  run_mode?: "manual" | "assisted" | "autonomous" | string;
  memory_graph?: {
    task?: string;
    nodes?: Array<{ id?: string; type?: string; label?: string }>;
    edges?: Array<{ from?: string; to?: string; rel?: string }>;
    summary?: {
      related_events?: number;
      related_failures?: number;
      sources?: string[];
    };
  };
  memory_recall?: {
    hit_count?: number;
    sources?: string[];
    evidence?: Array<Record<string, unknown>>;
    recall_applied_before_planning?: boolean;
  };
}

export interface TaskExecuteResult {
  ok?: boolean;
  run_mode?: "manual" | "assisted" | "autonomous" | string;
  repair_readiness?: Record<string, unknown>;
  repair_history?: Record<string, unknown>;
}

export interface SkillImportSourceGithub {
  kind: "github";
  repo: string;
  ref: string;
  path?: string;
  sha256?: string;
}

export interface SkillImportSourceUrl {
  kind: "url";
  url: string;
  sha256?: string;
}

export interface SkillImportSourceLocal {
  kind: "local";
  path: string;
  sha256?: string;
}

export type SkillImportSource =
  | SkillImportSourceGithub
  | SkillImportSourceUrl
  | SkillImportSourceLocal;

export interface ImportedSkillRecord {
  name?: string;
  version?: string;
  description?: string;
  source?: string;
  source_ref?: string;
  sha256?: string;
  manifest_path?: string;
  enabled?: boolean;
  imported_at?: number;
}

export interface SkillImportResult {
  ok?: boolean;
  skill?: ImportedSkillRecord;
}

export interface SkillListImportedResult {
  ok?: boolean;
  skills?: ImportedSkillRecord[];
}

export interface SkillRemoveResult {
  ok?: boolean;
  removed?: boolean;
  name?: string;
}

export interface SkillCreateResult {
  ok?: boolean;
  name?: string;
}

function parseRpcJson(raw: string): unknown {
  try {
    return JSON.parse(raw || "{}");
  } catch (error) {
    const preview = (raw || "").slice(0, 200);
    const reason =
      error instanceof Error ? error.message : "unknown parse error";
    throw new Error(`Invalid RPC JSON response: ${reason}. preview=${preview}`);
  }
}

function rpcErrorMessage(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") {
    return null;
  }

  const root = payload as Record<string, unknown>;
  const error = root.error;
  if (!error || typeof error !== "object") {
    return null;
  }

  const errorObj = error as Record<string, unknown>;
  const message = errorObj.message;
  const code = errorObj.code;
  if (typeof message === "string" && message.trim().length > 0) {
    return typeof code === "number" ? `[${code}] ${message}` : message;
  }

  return "RPC request failed";
}

function unwrapResult<T>(payload: unknown): T {
  const errorMessage = rpcErrorMessage(payload);
  if (errorMessage) {
    throw new Error(errorMessage);
  }
  if (
    payload &&
    typeof payload === "object" &&
    "result" in (payload as Record<string, unknown>)
  ) {
    const result = (payload as Record<string, unknown>).result;
    if (result !== undefined) {
      const nestedErrorMessage = rpcErrorMessage(result);
      if (nestedErrorMessage) {
        throw new Error(nestedErrorMessage);
      }
      return result as T;
    }
  }
  return (payload ?? {}) as T;
}

async function callRpcJson<T>(method: string, params: unknown): Promise<T> {
  const payload = JSON.stringify(params ?? {});
  const raw = await invokeRuntimeRpc(method, payload);
  return unwrapResult<T>(parseRpcJson(raw));
}

export async function getGovernanceStatus(): Promise<GovernanceStatusResult> {
  return callRpcJson<GovernanceStatusResult>("governance.status", {});
}

export async function getGovernanceAuditRecent(
  limit = 20,
): Promise<GovernanceAuditRecentResult> {
  return callRpcJson<GovernanceAuditRecentResult>("governance.audit.recent", {
    limit,
  });
}

export async function getProviderStatus(): Promise<ProviderStatusResult> {
  return callRpcJson<ProviderStatusResult>("provider.status", {});
}

export async function getReleaseReadiness(): Promise<ReleaseReadinessResult> {
  return callRpcJson<ReleaseReadinessResult>("release.readiness", {});
}

export async function getHealthProbes(): Promise<HealthProbesResult> {
  return callRpcJson<HealthProbesResult>("health.probes", {});
}

export async function getRuntimeSelfModel(
  params: Record<string, unknown> = {},
): Promise<RuntimeSelfModelResult> {
  return callRpcJson<RuntimeSelfModelResult>("runtime.self_model", params);
}

export async function getBreakerStatus(): Promise<BreakerStatusResult> {
  return callRpcJson<BreakerStatusResult>("breaker.status", {});
}

export async function getMetrics(): Promise<MetricsResult> {
  return callRpcJson<MetricsResult>("metrics.get", {});
}

export async function callTaskPlan(
  task: string,
  params: Record<string, unknown> = {},
): Promise<TaskPlanResult> {
  return callRpcJson<TaskPlanResult>("task.plan", { task, ...params });
}

export async function callTaskExecute(
  task: string,
  params: Record<string, unknown> = {},
): Promise<TaskExecuteResult> {
  return callRpcJson<TaskExecuteResult>("task.execute", { task, ...params });
}

export async function importSkill(
  source: SkillImportSource,
): Promise<SkillImportResult> {
  return callRpcJson<SkillImportResult>("skill.import", { source });
}

export async function listImportedSkills(): Promise<SkillListImportedResult> {
  return callRpcJson<SkillListImportedResult>("skill.list_imported", {});
}

export async function enableImportedSkill(
  name: string,
): Promise<SkillImportResult> {
  return callRpcJson<SkillImportResult>("skill.enable", { name });
}

export async function disableImportedSkill(
  name: string,
): Promise<SkillImportResult> {
  return callRpcJson<SkillImportResult>("skill.disable", { name });
}

export async function removeImportedSkill(
  name: string,
): Promise<SkillRemoveResult> {
  return callRpcJson<SkillRemoveResult>("skill.remove", { name });
}

export async function createSkill(source: {
  name: string;
  description: string;
  prompt_template: string;
  input_schema: Record<string, string>;
}): Promise<SkillCreateResult> {
  return callRpcJson<SkillCreateResult>("skill.create", source);
}
