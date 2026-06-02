import type {
  ApiResponse,
  BreakerStatusResponse,
  ChatRequest,
  ChatMessage,
  CheckpointListResponse,
  ConfigBaselineResponse,
  CostStatusResponse,
  GovernanceStatusResponse,
  HarnessStatusResponse,
  HealthProbesResponse,
  HealthResponse,
  LearningSummaryResponse,
  MetricsResponse,
  SelectorStatusResponse,
  TaskPlanResponse,
  Usage,
} from "./types";

// ---------------------------------------------------------------------------
// Endpoint constants
// ---------------------------------------------------------------------------

/** JSON-RPC endpoint path (replaces deprecated `/v1/responses`). */
const JSON_RPC_ENDPOINT = "/rpc";

/** Chat SSE streaming endpoint path (replaces deprecated `/acp/chat`). */
const CHAT_STREAM_ENDPOINT = "/chat/stream";

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

export class GoOnError extends Error {
  constructor(
    public readonly code: number,
    message: string,
  ) {
    super(message);
    this.name = "GoOnError";
  }
}

// ---------------------------------------------------------------------------
// GoOnClient
// ---------------------------------------------------------------------------

/** JSON-RPC 2.0 client for the go-on ACP endpoint. */
export class GoOnClient {
  private baseUrl: string;
  private nextId: number;
  private timeoutMs: number;

  constructor(baseUrl: string, timeoutMs = 30_000) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    this.nextId = 1;
    this.timeoutMs = timeoutMs;
  }

  // ── Low-level JSON-RPC ─────────────────────────────────────────────

  private async jsonRpc<T>(
    method: string,
    params: Record<string, unknown>,
  ): Promise<T> {
    const id = this.nextId++;
    const body = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    const response = await fetch(`${this.baseUrl}${JSON_RPC_ENDPOINT}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(this.timeoutMs),
    });

    if (!response.ok) {
      throw new GoOnError(
        response.status,
        `HTTP ${response.status}: ${response.statusText}`,
      );
    }

    const json: unknown = await response.json();
    const payload = json as Record<string, unknown>;

    if (payload.error) {
      const err = payload.error as Record<string, unknown>;
      throw new GoOnError(
        (err.code as number) ?? -1,
        (err.message as string) ?? "unknown error",
      );
    }

    return (payload.result ?? payload) as T;
  }

  // ── Streaming chat (SSE) ───────────────────────────────────────────

  /**
   * Send a chat request and return an async generator over SSE stream chunks.
   * Each yielded value is a parsed JSON chunk from the SSE `data:` field.
   * The generator terminates when the stream ends or is aborted.
   */
  async *chatStream(
    request: ChatRequest,
    signal?: AbortSignal,
  ): AsyncGenerator<Record<string, unknown>, void, unknown> {
    const response = await fetch(`${this.baseUrl}${CHAT_STREAM_ENDPOINT}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ...request, stream: true }),
      signal,
    });

    if (!response.ok) {
      throw new GoOnError(
        response.status,
        `HTTP ${response.status}: ${response.statusText}`,
      );
    }

    const reader = response.body!.getReader();
    const decoder = new TextDecoder();
    let buffer = "";

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });

        // Process complete SSE frames (delimited by \n\n)
        while (buffer.includes("\n\n")) {
          const idx = buffer.indexOf("\n\n");
          const frame = buffer.slice(0, idx);
          buffer = buffer.slice(idx + 2);

          for (const line of frame.split("\n")) {
            if (line.startsWith("data:")) {
              const data = line.slice(5).trim();
              if (data === "[DONE]") return;
              yield JSON.parse(data) as Record<string, unknown>;
            }
          }
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  // ── Core Runtime ───────────────────────────────────────────────────

  /** GET /health — quick health check. */
  async health(): Promise<ApiResponse<HealthResponse>> {
    const response = await fetch(`${this.baseUrl}/health`, {
      signal: AbortSignal.timeout(5_000),
    });
    if (!response.ok) {
      throw new GoOnError(
        response.status,
        `HTTP ${response.status}: ${response.statusText}`,
      );
    }
    return (await response.json()) as ApiResponse<HealthResponse>;
  }

  /** runtime.health — full runtime health via JSON-RPC. */
  async runtimeHealth(): Promise<HealthResponse> {
    return this.jsonRpc<HealthResponse>("runtime.health", {});
  }

  /** runtime.stability — runtime stability snapshot. */
  async runtimeStability(): Promise<Record<string, unknown>> {
    return this.jsonRpc("runtime.stability", {});
  }

  /** initialize — initialize the runtime. */
  async initialize(setupLevel: string): Promise<Record<string, unknown>> {
    return this.jsonRpc("initialize", { setup_level: setupLevel });
  }

  /** shutdown — gracefully shut down the runtime. */
  async shutdown(): Promise<Record<string, unknown>> {
    return this.jsonRpc("shutdown", {});
  }

  // ── Governance ─────────────────────────────────────────────────────

  /** governance.status — full governance status. */
  async governanceStatus(): Promise<GovernanceStatusResponse> {
    return this.jsonRpc("governance.status", {});
  }

  /** governance.plan.get — get active governance plan. */
  async governancePlanGet(): Promise<Record<string, unknown>> {
    return this.jsonRpc("governance.plan.get", {});
  }

  /** governance.audit.recent — view recent audit entries. */
  async governanceAuditRecent(limit: number): Promise<Record<string, unknown>> {
    return this.jsonRpc("governance.audit.recent", { limit });
  }

  // ── Observability ──────────────────────────────────────────────────

  /** health.probes — module-level health probes. */
  async healthProbes(): Promise<HealthProbesResponse> {
    return this.jsonRpc("health.probes", {});
  }

  /** metrics.get — get current runtime metrics. */
  async metricsGet(): Promise<MetricsResponse> {
    return this.jsonRpc("metrics.get", {});
  }

  /** metrics.prometheus — get Prometheus-formatted metrics. */
  async metricsPrometheus(): Promise<string> {
    const result = await this.jsonRpc<string>("metrics.prometheus", {});
    return result;
  }

  /** trace.get — get trace entries. */
  async traceGet(limit: number): Promise<Record<string, unknown>> {
    return this.jsonRpc("trace.get", { limit });
  }

  // ── Reliability ────────────────────────────────────────────────────

  /** breaker.status — get circuit breaker status. */
  async breakerStatus(): Promise<BreakerStatusResponse> {
    return this.jsonRpc("breaker.status", {});
  }

  /** breaker.reset — reset a circuit breaker. */
  async breakerReset(name: string): Promise<Record<string, unknown>> {
    return this.jsonRpc("breaker.reset", { name });
  }

  /** maintenance.gc — run garbage collection. */
  async maintenanceGc(): Promise<Record<string, unknown>> {
    return this.jsonRpc("maintenance.gc", {});
  }

  // ── Checkpoint ─────────────────────────────────────────────────────

  /** checkpoint.create — create a runtime checkpoint. */
  async checkpointCreate(branch: string): Promise<Record<string, unknown>> {
    return this.jsonRpc("checkpoint.create", { branch });
  }

  /** checkpoint.list — list available checkpoints. */
  async checkpointList(): Promise<CheckpointListResponse> {
    return this.jsonRpc("checkpoint.list", {});
  }

  /** conversation.rollback — roll back to a checkpoint. */
  async conversationRollback(
    checkpointId: string,
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc("conversation.rollback", {
      checkpoint_id: checkpointId,
    });
  }

  // ── Workflow / Task ────────────────────────────────────────────────

  /** workflow.execute — execute the current workflow. */
  async workflowExecute(): Promise<Record<string, unknown>> {
    return this.jsonRpc("workflow.execute", {});
  }

  /** workflow.plan — generate a task plan. */
  async workflowPlan(task: string): Promise<TaskPlanResponse> {
    return this.jsonRpc("workflow.plan", { task });
  }

  /** task.execute — execute a specific task. */
  async taskExecute(task: string): Promise<Record<string, unknown>> {
    return this.jsonRpc("task.execute", { task });
  }

  // ── Learning / Intelligence ────────────────────────────────────────

  /** summary.get — get learning summary. */
  async summaryGet(): Promise<LearningSummaryResponse> {
    return this.jsonRpc("summary.get", {});
  }

  /** selector.status — get selector/router status. */
  async selectorStatus(): Promise<SelectorStatusResponse> {
    return this.jsonRpc("selector.status", {});
  }

  /** knowledge.search — search knowledge base. */
  async knowledgeSearch(query: string): Promise<Record<string, unknown>> {
    return this.jsonRpc("knowledge.search", { query });
  }

  /** rl.optimize — trigger reinforcement learning optimization. */
  async rlOptimize(): Promise<Record<string, unknown>> {
    return this.jsonRpc("rl.optimize", {});
  }

  // ── Optimization / Operations ──────────────────────────────────────

  /** cost.status — get cost/budget status. */
  async costStatus(): Promise<CostStatusResponse> {
    return this.jsonRpc("cost.status", {});
  }

  /** config.baseline — get configuration baseline. */
  async configBaseline(): Promise<ConfigBaselineResponse> {
    return this.jsonRpc("config.baseline", {});
  }

  /** harness.status — get harness integration testing status. */
  async harnessStatus(): Promise<HarnessStatusResponse> {
    return this.jsonRpc("harness.status", {});
  }
}
