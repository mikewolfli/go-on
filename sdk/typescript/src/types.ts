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

/** Health check response (ServerStatus / runtime.health payloads). */
export interface HealthResponse {
  lifecycle?: Record<string, unknown>;
  version?: string;
  stats?: Record<string, unknown>;
  maintenance?: Record<string, unknown>;
  timestamp?: number;
  metrics?: Record<string, unknown>;
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

// ── ACP Session Protocol Types ────────────────────────────────────────────

/** Request to create a new ACP session. */
export interface AcpSessionNewRequest {
  cwd?: string;
  /** Backend reads this key as snake_case `work_dirs`. */
  work_dirs?: string[];
  /** Backend reads this key as camelCase `additionalDirectories`. */
  additionalDirectories?: string[];
  mode?: string;
}

/** Response from creating a new ACP session. */
export interface AcpSessionNewResponse {
  sessionId: string;
  modes?: SessionModeState;
  configOptions?: SessionConfigOption[];
}

/** Response from loading an existing ACP session (session/load). */
export interface SessionLoadResponse {
  sessionId?: string;
  modes?: SessionModeState;
  configOptions?: SessionConfigOption[];
  memoryContext?: string[];
  memoryRestoredCount?: number;
}

/** Request to send a prompt in an ACP session. */
export interface AcpSessionPromptRequest {
  sessionId: string;
  prompt: PromptContentBlock[];
  mode?: string;
  cwd?: string;
  /** Backend reads this key as camelCase `additionalDirectories`. */
  additionalDirectories?: string[];
}

/** Request to close an ACP session. */
export interface AcpSessionCloseRequest {
  sessionId: string;
}

/** Response from listing ACP sessions. */
export interface AcpSessionListResponse {
  /** The backend returns a minimal summary per session: `[{ "id": sid }]`. */
  sessions: AcpSessionInfo[];
  /**
   * Wire key is camelCase `nextCursor` (backend `ListSessionsResponse` uses
   * `#[serde(rename_all = "camelCase")]`); currently always absent because
   * the backend handler sends `next_cursor: None`.
   */
  nextCursor?: string | null;
}

/** Info about an ACP session, as returned by the backend `session/list`. */
export interface AcpSessionInfo {
  id: string;
}

/** Request to resume an ACP session. */
export interface AcpSessionResumeRequest {
  sessionId: string;
  cwd?: string;
}

/** State describing the current session mode and available modes. */
export interface SessionModeState {
  currentModeId: string;
  availableModes: SessionModeDescription[];
}

/** Description of an available session mode. */
export interface SessionModeDescription {
  id: string;
  name: string;
  description?: string;
}

/** Configuration option for a session. */
export interface SessionConfigOption {
  id: string;
  name: string;
  description?: string;
  category?: string;
  kind: SessionConfigKind;
}

/** Kind of a session configuration option (discriminated union). */
export type SessionConfigKind =
  | { type: "select"; currentValue: string; options: SessionConfigSelectOptions };

/** Select options for a select-type config option. */
export type SessionConfigSelectOptions =
  | { type: "grouped"; groups: SessionConfigGroup[] };

/** A group within select options. */
export interface SessionConfigGroup {
  group: string;
  name: string;
  options: SessionConfigOption[];
}

/** Descriptor for a tool exposed via tools/list. */
export interface ToolInfo {
  name: string;
  description: string;
  /** Backend emits the schema under the snake_case key `input_schema`. */
  input_schema?: Record<string, unknown>;
  /** MCP-spec alias; some tool entries may emit `inputSchema` instead. */
  inputSchema?: Record<string, unknown>;
}

/** Request payload for tools/call. */
export interface ToolsCallRequest {
  name: string;
  arguments: Record<string, unknown>;
  /** Optional ACP session ID for progress streaming. */
  sessionId?: string;
}

/** Result of a tools/call execution. */
export interface ToolsCallResult {
  content: Array<{ type: string; text: string }>;
  structured?: unknown;
  isError?: boolean;
}

/** A content block in a prompt (text, resource, image, audio, etc.). */
export interface PromptContentBlock {
  type: "text" | "resource" | "resource_link" | "image" | "audio";
  text?: string;
  uri?: string;
  name?: string;
  resource?: { uri: string; text?: string; mimeType?: string };
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
