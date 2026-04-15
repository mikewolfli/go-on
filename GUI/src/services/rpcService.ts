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

function parseRpcJson(raw: string): unknown {
  try {
    return JSON.parse(raw || "{}");
  } catch {
    return {};
  }
}

function unwrapResult<T>(payload: unknown): T {
  if (payload && typeof payload === "object" && "result" in (payload as Record<string, unknown>)) {
    const result = (payload as Record<string, unknown>).result;
    if (result !== undefined) {
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

export async function getHealthProbes(): Promise<HealthProbesResult> {
  return callRpcJson<HealthProbesResult>("health.probes", {});
}

export async function getBreakerStatus(): Promise<BreakerStatusResult> {
  return callRpcJson<BreakerStatusResult>("breaker.status", {});
}

export async function getMetrics(): Promise<MetricsResult> {
  return callRpcJson<MetricsResult>("metrics.get", {});
}