//! Type definitions for the go-on Node.js SDK.

/** A chat message in a conversation. */
export interface ChatMessage {
  role: "user" | "assistant" | "system" | "tool";
  content: string;
}

/** Parameters for a chat request. */
export interface ChatRequest {
  messages: ChatMessage[];
  model?: string;
  temperature?: number;
  max_tokens?: number;
  stream?: boolean;
}

/** Response from the /rpc health endpoint. */
export interface HealthResponse {
  status: string;
  version: string;
  uptime_seconds: number;
  modules?: Record<string, unknown>;
}

/** Response from the governance.status RPC. */
export interface GovernanceStatusResponse {
  ok: boolean;
  governance: Record<string, unknown>;
}

/** Response from health.probes RPC. */
export interface HealthProbesResponse {
  modules: Record<string, unknown>;
}

/** Response from metrics.get RPC. */
export interface MetricsResponse {
  metrics: Record<string, unknown>;
}

/** Response from breaker.status RPC. */
export interface BreakerStatusResponse {
  breakers: Record<string, unknown>;
}

/** Response from checkpoint.list RPC. */
export interface CheckpointListResponse {
  checkpoints: Array<Record<string, unknown>>;
}

/** Response from task.plan RPC. */
export interface TaskPlanResponse {
  plan: Record<string, unknown>;
}

/** Response from learning.summary RPC. */
export interface LearningSummaryResponse {
  summary: Record<string, unknown>;
}

/** Response from selector.status RPC. */
export interface SelectorStatusResponse {
  selector: Record<string, unknown>;
}

/** Response from cost.status RPC. */
export interface CostStatusResponse {
  cost: Record<string, unknown>;
}

/** Response from config.baseline RPC. */
export interface ConfigBaselineResponse {
  baseline: Record<string, unknown>;
}

/** Response from harness.status RPC. */
export interface HarnessStatusResponse {
  harness: Record<string, unknown>;
}
