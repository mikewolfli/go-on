import { invokeRuntimeRpc } from "./bridge";

export interface GovernanceStatusResult {
  governance?: {
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
  };
}

export interface ReleaseReadinessResult {
  readiness?: {
    version?: string;
    status?: string;
    overall_pass?: boolean;
    blocked_gate_count?: number;
    gates?: Array<{
      name?: string;
      passed?: boolean;
    }>;
  };
}

export interface HealthProbesResult {
  probes?: {
    liveness?: { ok?: boolean; status?: string; uptime_seconds?: number };
    readiness?: { ok?: boolean; status?: string; generated_at?: number };
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
      details?: {
        entries?: number;
        memory_entries?: number;
        summary_entries?: number;
      };
    }>;
    circuit_breakers?: Array<{ state?: string; failure_count?: number }>;
    rate_limiter?: {
      buckets?: Array<{ used_percent?: number }>;
    };
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
    };
    drift?: {
      alert?: boolean;
      absolute_diff?: number;
      threshold?: number;
    };
    decision?: {
      recommended_mode?: string;
      fallback_triggered?: boolean;
    };
    recommendations?: string[];
  };
}

export interface BreakerStatusResult {
  degraded_count?: number;
  degraded_services?: Array<{
    recommended_action?: string;
  }>;
}

export interface MetricsResult {
  total_requests?: number;
  successful_requests?: number;
  failed_requests?: number;
  avg_request_duration_ms?: number;
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

export type SkillImportSource = SkillImportSourceGithub | SkillImportSourceUrl | SkillImportSourceLocal;

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

function parseRpcJson(raw: string): unknown {
  try {
    return JSON.parse(raw || "{}");
  } catch {
    return {};
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
  if (payload && typeof payload === "object" && "result" in (payload as Record<string, unknown>)) {
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

export async function getGovernanceAuditRecent(limit = 20): Promise<GovernanceAuditRecentResult> {
  return callRpcJson<GovernanceAuditRecentResult>("governance.audit.recent", { limit });
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

export async function getRuntimeSelfModel(params: Record<string, unknown> = {}): Promise<RuntimeSelfModelResult> {
  return callRpcJson<RuntimeSelfModelResult>("runtime.self_model", params);
}

export async function getBreakerStatus(): Promise<BreakerStatusResult> {
  return callRpcJson<BreakerStatusResult>("breaker.status", {});
}

export async function getMetrics(): Promise<MetricsResult> {
  return callRpcJson<MetricsResult>("metrics.get", {});
}

export async function importSkill(source: SkillImportSource): Promise<SkillImportResult> {
  return callRpcJson<SkillImportResult>("skill.import", { source });
}

export async function listImportedSkills(): Promise<SkillListImportedResult> {
  return callRpcJson<SkillListImportedResult>("skill.list_imported", {});
}

export async function enableImportedSkill(name: string): Promise<SkillImportResult> {
  return callRpcJson<SkillImportResult>("skill.enable", { name });
}

export async function disableImportedSkill(name: string): Promise<SkillImportResult> {
  return callRpcJson<SkillImportResult>("skill.disable", { name });
}

export async function removeImportedSkill(name: string): Promise<SkillRemoveResult> {
  return callRpcJson<SkillRemoveResult>("skill.remove", { name });
}