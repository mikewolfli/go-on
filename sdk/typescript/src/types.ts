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

// ── BLUE56-E05: Missing key types ────────────────────────────────────────

/** Record of a tool call made by an agent. */
export interface ToolCall {
  /** Name of the tool that was called. */
  tool_name: string;
  /** Arguments passed to the tool. */
  arguments: Record<string, unknown>;
  /** The agent that made the call. */
  agent_name: string;
  /** Optional result of the tool execution. */
  result?: Record<string, unknown>;
  /** Duration of the tool call in milliseconds. */
  duration_ms: number;
}

/** Multimodal input types for rich chat requests. */
export type MultimodalInput =
  | { type: "text"; text: string }
  | { type: "image"; image_url: string; detail?: "auto" | "low" | "high" }
  | { type: "document"; data: string; mime_type: string; filename?: string }
  | { type: "audio"; data: string; format: string };

/** A single chunk in an SSE streaming response. */
export interface StreamChunk {
  /** The token text content. */
  token: string;
  /** Whether this is the final chunk. */
  done: boolean;
  /** Optional reasoning content. */
  reasoning?: string;
  /** Optional tool calls included in this chunk. */
  tool_calls?: ToolCall[];
  /** Chunk index in the stream. */
  index: number;
  /** Total characters sent so far. */
  total_chars: number;
}

/** Metadata about an available agent. */
export interface AgentInfo {
  /** Unique agent name/ID. */
  name: string;
  /** Agent type (e.g. "copilot", "custom"). */
  agent_type: string;
  /** Human-readable description. */
  description: string;
  /** Available model names this agent can use. */
  models?: string[];
  /** Capability tags (e.g. "coding", "review"). */
  capabilities?: string[];
  /** Whether this agent is currently healthy. */
  healthy?: boolean;
}
