import type {
  AcpSessionNewRequest,
  AcpSessionNewResponse,
  AcpSessionPromptRequest,
  AcpSessionCloseRequest,
  AcpSessionListResponse,
  AcpSessionResumeRequest,
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
  SessionModeState,
  TaskPlanResponse,
  ToolInfo,
  ToolsCallRequest,
  ToolsCallResult,
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
// Client options
// ---------------------------------------------------------------------------

export interface GoOnClientOptions {
  /** Base URL of the go-on ACP endpoint (trailing slash is stripped). */
  baseUrl: string;
  /** Request timeout in milliseconds (default: 30_000). */
  timeout?: number;
  /** Maximum number of retries for retryable failures (default: 3). */
  maxRetries?: number;
  /** Base delay between retries in milliseconds (default: 1000). */
  retryDelayMs?: number;
  /** Whether to use exponential backoff for retry delays (default: true). */
  useExponentialBackoff?: boolean;
}

// ---------------------------------------------------------------------------
// GoOnClient
// ---------------------------------------------------------------------------

/** JSON-RPC 2.0 client for the go-on ACP endpoint. */
export class GoOnClient {
  private baseUrl: string;
  private nextId: number;
  private timeout: number;
  private maxRetries: number;
  private retryDelayMs: number;
  private useExponentialBackoff: boolean;
  private abortController: AbortController;

  constructor(options: GoOnClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, "");
    this.nextId = 1;
    this.timeout = options.timeout ?? 30_000;
    this.maxRetries = options.maxRetries ?? 3;
    this.retryDelayMs = options.retryDelayMs ?? 1000;
    this.useExponentialBackoff = options.useExponentialBackoff ?? true;
    this.abortController = new AbortController();
  }

  // ── Low-level JSON-RPC ─────────────────────────────────────────────

  /**
   * Generic JSON-RPC call. Convenience escape hatch for methods that do not
   * have a dedicated typed wrapper yet (used e.g. by the VSCode addon for
   * ad-hoc methods such as `runtime.health` / `health.probes`).
   */
  async request<T = unknown>(
    method: string,
    params: Record<string, unknown> = {},
  ): Promise<T> {
    return this.jsonRpc<T>(method, params);
  }

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

    let lastError: GoOnError | null = null;

    for (let attempt = 0; attempt <= this.maxRetries; attempt++) {
      let response: Response;
      try {
        response = await fetch(`${this.baseUrl}${JSON_RPC_ENDPOINT}`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
          signal: AbortSignal.timeout(this.timeout),
        });
      } catch (err: unknown) {
        if (err instanceof DOMException && err.name === "AbortError") {
          throw new GoOnError(0, `Request timed out after ${this.timeout}ms`);
        }
        // Network errors are retryable
        lastError = new GoOnError(
          0,
          (err as Error).message ?? "Unknown fetch error",
        );
        if (attempt < this.maxRetries) {
          await this.delay(attempt);
        }
        continue;
      }

      // Retry on 429 (Too Many Requests) and 5xx (server errors)
      if (response.status === 429 || response.status >= 500) {
        lastError = new GoOnError(
          response.status,
          `HTTP ${response.status}: ${response.statusText}`,
        );
        if (attempt < this.maxRetries) {
          await this.delay(attempt);
        }
        continue;
      }

      // Non-retryable HTTP error (4xx except 429)
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

    throw lastError ?? new GoOnError(0, "Request failed after retries");
  }

  /**
   * Compute and await a retry delay.
   * Uses the unified backoff contract when `useExponentialBackoff` is enabled,
   * otherwise a fixed delay.
   */
  private async delay(attempt: number): Promise<void> {
    let ms: number;
    if (this.useExponentialBackoff) {
      // Unified contract (contracts/cross-client-sync.md):
      // delay = min(base * 2^attempt, 30s) * (0.7 + random() * 0.3)
      const capped = Math.min(30_000, this.retryDelayMs * 2 ** attempt);
      const jitter = 0.7 + Math.random() * 0.3;
      ms = Math.floor(capped * jitter);
    } else {
      ms = this.retryDelayMs;
    }
    await new Promise((r) => setTimeout(r, ms));
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
    let wasAborted = false;

    // Listen for abort signal so we can distinguish "completed" vs "aborted"
    const onAbort = () => {
      wasAborted = true;
    };
    signal?.addEventListener("abort", onAbort);

    try {
      while (true) {
        let result: ReadableStreamReadResult<Uint8Array>;
        try {
          result = await reader.read();
        } catch {
          wasAborted = true;
          yield {
            _type: "abort",
            message: "Chat stream was aborted",
          } as Record<string, unknown>;
          return;
        }

        const { done, value } = result;
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
              if (data === "[DONE]") {
                wasAborted = false;
                return;
              }
              yield JSON.parse(data) as Record<string, unknown>;
            }
          }
        }
      }
    } finally {
      signal?.removeEventListener("abort", onAbort);
      reader.releaseLock();
      // If the stream ended due to abort, yield a structured notification
      if (wasAborted) {
        yield { _type: "abort", message: "Chat stream was aborted" } as Record<
          string,
          unknown
        >;
      }
    }
  }

  // ── Core Runtime ───────────────────────────────────────────────────

  /** GET /health — quick health check. */
  /** GET /health — quick health check (ServerStatus payload, no envelope). */
  async health(): Promise<HealthResponse> {
    const response = await fetch(`${this.baseUrl}/health`, {
      signal: AbortSignal.timeout(5_000),
    });
    if (!response.ok) {
      throw new GoOnError(
        response.status,
        `HTTP ${response.status}: ${response.statusText}`,
      );
    }
    return (await response.json()) as HealthResponse;
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

  /**
   * governance.audit.verify — verify the tamper-evident audit hash chain.
   * Optional: fromMs/toMs export a time-window report; publicKeyHex enables
   * Ed25519 signature verification of signed chains.
   */
  async governanceAuditVerify(params: {
    fromMs?: number;
    toMs?: number;
    publicKeyHex?: string;
  } = {}): Promise<Record<string, unknown>> {
    return this.jsonRpc("governance.audit.verify", {
      from_ms: params.fromMs,
      to_ms: params.toMs,
      public_key_hex: params.publicKeyHex,
    });
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

  /** conversation.checkpoint.create — create a checkpoint for a conversation. */
  async checkpointCreate(
    conversationId: string,
    messages: Array<{ role: string; content: string }>,
    branch: string = "main",
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc("conversation.checkpoint.create", {
      conversation_id: conversationId,
      branch,
      messages,
    });
  }

  /** checkpoint.list — list available checkpoints for a conversation. */
  async checkpointList(conversationId: string): Promise<CheckpointListResponse> {
    return this.jsonRpc("checkpoint.list", {
      conversation_id: conversationId,
    });
  }

  /** conversation.rollback — roll back to a checkpoint. */
  async conversationRollback(
    conversationId: string,
    checkpointId: string,
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc("conversation.rollback", {
      conversation_id: conversationId,
      checkpoint_id: checkpointId,
    });
  }

  // ── Workflow / Task ────────────────────────────────────────────────

  /** workflow.execute — execute the current workflow. */
  async workflowExecute(): Promise<Record<string, unknown>> {
    return this.jsonRpc("workflow.execute", {});
  }

  /** task.plan — generate a task plan. */
  async taskPlan(task: string): Promise<TaskPlanResponse> {
    return this.jsonRpc("task.plan", { task });
  }

  /** task.execute — execute a specific task. */
  async taskExecute(task: string): Promise<Record<string, unknown>> {
    return this.jsonRpc("task.execute", { task });
  }

  // ── Learning / Intelligence ────────────────────────────────────────

  /** learning.summary — get learning summary. */
  async learningSummary(): Promise<LearningSummaryResponse> {
    return this.jsonRpc("learning.summary", {});
  }

  /** selector.status — get selector/router status. */
  async selectorStatus(): Promise<SelectorStatusResponse> {
    return this.jsonRpc("selector.status", {});
  }

  /** knowledge.distill — run knowledge distillation over the last `limit` events. */
  async knowledgeDistill(limit?: number): Promise<Record<string, unknown>> {
    return this.jsonRpc(
      "knowledge.distill",
      limit === undefined ? {} : { limit },
    );
  }

  /** rl.alignment.offline_eval — trigger RL alignment offline evaluation. */
  async rlAlignmentOfflineEval(): Promise<Record<string, unknown>> {
    return this.jsonRpc("rl.alignment.offline_eval", {});
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

  /** config.reload — reload configuration at runtime. */
  async configReload(): Promise<Record<string, unknown>> {
    return this.jsonRpc("config.reload", {});
  }

  /** harness.status — get harness integration testing status. */
  async harnessStatus(): Promise<HarnessStatusResponse> {
    return this.jsonRpc("harness.status", {});
  }

  // ── ACP Session Protocol ───────────────────────────────────────────

  /** session/new — create a new ACP session. */
  async sessionNew(params: AcpSessionNewRequest): Promise<AcpSessionNewResponse> {
    return this.jsonRpc("session/new", params as Record<string, unknown>);
  }

  /** session/prompt — send a prompt in an ACP session (returns stopReason). */
  async sessionPrompt(
    params: AcpSessionPromptRequest,
  ): Promise<{ stopReason: string }> {
    return this.jsonRpc("session/prompt", params as unknown as Record<string, unknown>);
  }

  /** session/close — close an ACP session. */
  async sessionClose(params: AcpSessionCloseRequest): Promise<void> {
    await this.jsonRpc("session/close", params as unknown as Record<string, unknown>);
  }

  /** session/list — list active ACP sessions. */
  async sessionList(): Promise<AcpSessionListResponse> {
    return this.jsonRpc("session/list", {});
  }

  /** session/resume — resume an existing ACP session. */
  async sessionResume(
    params: AcpSessionResumeRequest,
  ): Promise<SessionModeState> {
    return this.jsonRpc("session/resume", params as unknown as Record<string, unknown>);
  }

  /** session/set_mode — change the mode of an ACP session. */
  async sessionSetMode(
    sessionId: string,
    modeId: string,
  ): Promise<void> {
    await this.jsonRpc("session/set_mode", { sessionId, modeId });
  }

  /** session/set_config_option — set a configuration option for an ACP session. */
  async sessionSetConfigOption(
    sessionId: string,
    configId: string,
    value: string,
  ): Promise<void> {
    await this.jsonRpc("session/set_config_option", {
      sessionId,
      configId,
      value,
    });
  }

  /** session/load — load an existing ACP session (modes + config options). */
  async sessionLoad(sessionId: string): Promise<Record<string, unknown>> {
    return this.jsonRpc("session/load", { sessionId });
  }

  /** session/delete — delete an ACP session (and its permission state). */
  async sessionDelete(sessionId: string): Promise<{
    deleted: boolean;
    sessionId: string;
  }> {
    return this.jsonRpc("session/delete", { sessionId });
  }

  /** session/config/get — read the per-session config options. */
  async sessionConfigGet(sessionId: string): Promise<{
    configOptions: Record<string, unknown>;
    sessionId: string;
  }> {
    return this.jsonRpc("session/config/get", { sessionId });
  }

  /** session/config/set — set a per-session config option. */
  async sessionConfigSet(
    sessionId: string,
    configId: string,
    value: string,
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc("session/config/set", { sessionId, configId, value });
  }

  /** session/request_permission — respond to a pending permission request. */
  async sessionRequestPermission(
    sessionId: string,
    optionId: string,
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc("session/request_permission", { sessionId, optionId });
  }

  // ── Tools ────────────────────────────────────────────────────────────

  /** tools/list — list all available tools with their input schemas. */
  async toolsList(): Promise<ToolInfo[]> {
    const result = await this.jsonRpc<{ tools?: ToolInfo[] }>("tools/list", {});
    return result.tools ?? [];
  }

  /** tools/call — execute a tool by name with the given arguments. */
  async toolsCall(params: ToolsCallRequest): Promise<ToolsCallResult> {
    return this.jsonRpc<ToolsCallResult>("tools/call", {
      name: params.name,
      arguments: params.arguments,
      sessionId: params.sessionId,
    });
  }

  /** tool.approve — approve a tool so later calls bypass the review gate. */
  async toolApprove(toolName: string): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("tool.approve", {
      tool_name: toolName,
    });
  }

  // ── Skills ─────────────────────────────────────────────────────────

  /** skill.list — list all registered skills. */
  async skillList(): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.list", {});
  }

  /** skill.list_imported — list imported skills. */
  async skillListImported(): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.list_imported", {});
  }

  /** skill.create — create a prompt-based skill. */
  async skillCreate(params: {
    name: string;
    description: string;
    prompt_template: string;
    input_schema?: Record<string, string>;
  }): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.create", {
      name: params.name,
      description: params.description,
      prompt_template: params.prompt_template,
      input_schema: params.input_schema ?? {},
    });
  }

  /** skill.update — update a skill's description / prompt template. */
  async skillUpdate(
    name: string,
    patch: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.update", {
      name,
      ...patch,
    });
  }

  /** skill.enable — enable a skill. */
  async skillEnable(name: string): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.enable", { name });
  }

  /** skill.disable — disable a skill. */
  async skillDisable(name: string): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.disable", { name });
  }

  /** skill.remove — remove a skill. */
  async skillRemove(name: string): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.remove", { name });
  }

  /** skill.version.list — list versions of a skill. */
  async skillVersionList(name: string): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.version.list", { name });
  }

  /** skill.version.rollback — roll back a skill to a previous version. */
  async skillVersionRollback(
    name: string,
    version: string,
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("skill.version.rollback", {
      name,
      version,
    });
  }

  // ── Prompts ────────────────────────────────────────────────────────

  /** prompts.list — list all prompt templates for a language. */
  async promptsList(lang = "en"): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("prompts.list", { lang });
  }

  /** prompts.get — get a single prompt template by id. */
  async promptsGet(id: string, lang = "en"): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("prompts.get", { id, lang });
  }

  /** prompts.search — search prompt templates by keyword. */
  async promptsSearch(query: string, lang = "en"): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("prompts.search", { query, lang });
  }

  /** prompts.create — create a custom prompt template. */
  async promptsCreate(
    template: Record<string, unknown>,
    lang = "en",
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("prompts.create", {
      lang,
      ...template,
    });
  }

  /** prompts.update — update a custom prompt template. */
  async promptsUpdate(
    template: Record<string, unknown>,
    lang = "en",
  ): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("prompts.update", {
      lang,
      ...template,
    });
  }

  /** prompts.delete — delete a custom prompt template by id. */
  async promptsDelete(id: string, lang = "en"): Promise<Record<string, unknown>> {
    return this.jsonRpc<Record<string, unknown>>("prompts.delete", { id, lang });
  }

  // ── OpenAI-compatible endpoints ─────────────────────────────────────

  /**
   * POST /v1/chat/completions — OpenAI-compatible chat completions.
   * The body follows the OpenAI wire format and is forwarded verbatim.
   */
  async chatCompletions(
    request: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.httpJson("POST", "/v1/chat/completions", request);
  }

  /**
   * POST /v1/responses — OpenAI Responses API create.
   * The body follows the Responses API wire format and is forwarded verbatim.
   */
  async responsesCreate(
    request: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return this.httpJson("POST", "/v1/responses", request);
  }

  /** GET /v1/responses/{id} — retrieve a single Responses API response. */
  async responsesGet(responseId: string): Promise<Record<string, unknown>> {
    return this.httpJson("GET", `/v1/responses/${encodeURIComponent(responseId)}`);
  }

  /** GET /v1/models — list models exposed by the OpenAI-compat surface. */
  async modelsList(): Promise<Record<string, unknown>> {
    return this.httpJson("GET", "/v1/models");
  }

  /**
   * Low-level JSON HTTP helper for the OpenAI-compatible surface
   * (plain JSON in/out — no JSON-RPC envelope, no result unwrapping).
   */
  private async httpJson(
    method: "GET" | "POST",
    path: string,
    body?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const init: RequestInit = {
      method,
      headers: { "Content-Type": "application/json" },
      signal: AbortSignal.timeout(this.timeout),
    };
    if (body !== undefined) {
      init.body = JSON.stringify(body);
    }
    const response = await fetch(`${this.baseUrl}${path}`, init);
    if (!response.ok) {
      throw new GoOnError(
        response.status,
        `HTTP ${response.status}: ${response.statusText}`,
      );
    }
    return (await response.json()) as Record<string, unknown>;
  }
}
