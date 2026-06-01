/** Core message in a chat conversation. */
export interface ChatMessage {
  role: "user" | "assistant" | "system";
  content: string;
}

/** Request payload for the chat endpoint. */
export interface ChatRequest {
  messages: ChatMessage[];
  model?: string;
  temperature?: number;
  max_tokens?: number;
  stream?: boolean;
}

/** Token usage metadata returned by the API. */
export interface Usage {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
}

/** Standard API response envelope wrapping typed output. */
export interface ApiResponse<T> {
  id: string;
  object: string;
  created_at: number;
  model: string;
  status: string;
  output: T;
  usage?: Usage;
  error?: string;
}

/** Health check response. */
export interface HealthResponse {
  status: string;
  version: string;
  uptime_seconds: number;
  modules?: Record<string, unknown>;
}

/** Governance status. */
export interface GovernanceStatusResponse {
  ok: boolean;
  governance: Record<string, unknown>;
}

/** Circuit breaker status. */
export interface BreakerStatusResponse {
  breakers: Record<string, unknown>;
}

/** Runtime metrics snapshot. */
export interface MetricsResponse {
  metrics: Record<string, unknown>;
}

/** Health probe results per module. */
export interface HealthProbesResponse {
  modules: Record<string, unknown>;
}

/** Cost / budget status. */
export interface CostStatusResponse {
  cost: Record<string, unknown>;
}

/** Configuration baseline info. */
export interface ConfigBaselineResponse {
  baseline: Record<string, unknown>;
}

/** Harness / integration testing status. */
export interface HarnessStatusResponse {
  harness: Record<string, unknown>;
}

/** Learning / intelligence summary. */
export interface LearningSummaryResponse {
  summary: Record<string, unknown>;
}

/** Selector / router status. */
export interface SelectorStatusResponse {
  selector: Record<string, unknown>;
}

/** Checkpoint list entry. */
export interface CheckpointListResponse {
  checkpoints: unknown[];
}

/** Task plan description. */
export interface TaskPlanResponse {
  plan: Record<string, unknown>;
}
