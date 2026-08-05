//! Main GoOnClient class — async JSON-RPC client for go-on.
//!
//! Targets `POST {base_url}/rpc` for JSON-RPC calls
//! and `/chat/stream` for SSE streaming chat.
//!
//! Covers: runtime, governance, observability, reliability,
//! checkpoint, workflow, learning, optimization, streaming chat.

import * as https from "https";
import * as http from "http";
import * as url from "url";
import { v4 as uuidv4 } from "uuid";

import { GoOnClientError, GoOnJsonRpcError, GoOnHttpError } from "./errors";
import { createSseParser } from "./sse";
import type { SseFrame } from "./sse";
import type {
  AcpSessionNewRequest,
  AcpSessionNewResponse,
  AcpSessionPromptRequest,
  AcpSessionCloseRequest,
  AcpSessionListResponse,
  AcpSessionResumeRequest,
  BreakerStatusResponse,
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
} from "./types";

/** Options for the GoOnClient constructor. */
export interface GoOnClientOptions {
  /** Base URL of the go-on server (e.g. `http://127.0.0.1:8090`). */
  baseUrl: string;
  /** Timeout in seconds for HTTP requests (default: 30.0). */
  timeout?: number;
  /** Max retries for transient failures (default: 3). */
  maxRetries?: number;
  /** Base retry delay in ms (default: 1000). Uses exponential backoff. */
  retryDelayMs?: number;
}

// ── Client ──────────────────────────────────────────────────────────

/**
 * Async JSON-RPC client for go-on ACP endpoints.
 *
 * ```ts
 * const client = new GoOnClient({ baseUrl: "http://127.0.0.1:8090" });
 * const health = await client.health();
 * for await (const chunk of client.chatStream([{ role: "user", content: "Hello" }])) {
 *   process.stdout.write(chunk);
 * }
 * await client.close();
 * ```
 */
export class GoOnClient {
  readonly baseUrl: string;
  readonly timeout: number;
  readonly maxRetries: number;
  readonly retryDelayMs: number;
  private _abortController: AbortController | null = null;

  constructor(options: GoOnClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, "");
    this.timeout = options.timeout ?? 30.0;
    this.maxRetries = options.maxRetries ?? 3;
    this.retryDelayMs = options.retryDelayMs ?? 1000;
  }

  /** Close the client and abort any in-flight requests. */
  close(): void {
    this._abortController?.abort();
    this._abortController = null;
  }

  // ── Internal helpers ──────────────────────────────────────────────

  /**
   * Compute retry delay with the unified exponential backoff contract
   * (contracts/cross-client-sync.md):
   * `delay = min(base * 2^attempt, 30s) * (0.7 + random() * 0.3)`
   * The ±30% jitter keeps delays above 70% of the base, matching the GUI
   * and VS Code implementations exactly.
   */
  private _retryDelayForAttempt(attempt: number): number {
    const cappedMs = Math.min(30_000, this.retryDelayMs * Math.pow(2, attempt));
    const jitter = 0.7 + Math.random() * 0.3;
    return Math.floor(cappedMs * jitter);
  }

  /** Build URL from base + path. */
  private _url(path: string): string {
    return `${this.baseUrl}${path}`;
  }

  /**
   * Perform a JSON-RPC call via POST to `/rpc`.
   *
   * Retries on transient HTTP errors (5xx, 429) with exponential backoff.
   * Throws `GoOnJsonRpcError` on JSON-RPC-level errors,
   * `GoOnHttpError` on non-2xx HTTP responses without valid JSON-RPC body,
   * and `GoOnClientError` on network errors.
   */
  private async _jsonRpc(
    method: string,
    params?: Record<string, unknown>,
  ): Promise<unknown> {
    const payload = JSON.stringify({
      jsonrpc: "2.0",
      id: uuidv4(),
      method,
      params: params ?? {},
    });

    let lastError: Error | null = null;

    for (let attempt = 0; attempt <= this.maxRetries; attempt++) {
      if (attempt > 0) {
        const delay = this._retryDelayForAttempt(attempt - 1);
        await new Promise((resolve) => setTimeout(resolve, delay));
      }

      try {
        const response = await this._httpPost("/rpc", payload, {
          "Content-Type": "application/json",
        });

        const body = JSON.parse(response.body) as {
          result?: unknown;
          error?: { code: number; message: string };
        };

        if (body.error) {
          throw new GoOnJsonRpcError(body.error.code, body.error.message);
        }

        return body.result;
      } catch (err) {
        // Don't retry JSON-RPC errors (client errors) or auth errors
        if (
          err instanceof GoOnJsonRpcError ||
          (err instanceof GoOnHttpError && err.statusCode === 401) ||
          (err instanceof GoOnHttpError && err.statusCode === 403)
        ) {
          throw err;
        }

        lastError = err instanceof Error ? err : new Error(String(err));

        // Don't retry on last attempt
        if (attempt === this.maxRetries) {
          throw lastError;
        }
      }
    }

    throw lastError ?? new GoOnClientError("Unexpected retry exhaustion");
  }

  /**
   * Low-level HTTP POST using Node.js built-in http/https.
   */
  private _httpPost(
    path: string,
    body: string,
    headers: Record<string, string>,
  ): Promise<{ statusCode: number; statusMessage: string; body: string }> {
    return new Promise((resolve, reject) => {
      const parsedUrl = new url.URL(this._url(path));
      const isHttps = parsedUrl.protocol === "https:";
      const lib = isHttps ? https : http;

      const options: http.RequestOptions = {
        hostname: parsedUrl.hostname,
        port: parsedUrl.port ? parseInt(parsedUrl.port, 10) : undefined,
        path: parsedUrl.pathname + parsedUrl.search,
        method: "POST",
        headers: {
          ...headers,
          "Content-Length": Buffer.byteLength(body).toString(),
        },
        timeout: this.timeout * 1000,
      };

      const req = lib.request(options, (res) => {
        const chunks: Buffer[] = [];
        res.on("data", (chunk: Buffer) => chunks.push(chunk));
        res.on("end", () => {
          const responseBody = Buffer.concat(chunks).toString("utf-8");
          if (res.statusCode && res.statusCode >= 200 && res.statusCode < 300) {
            resolve({
              statusCode: res.statusCode,
              statusMessage: res.statusMessage || "",
              body: responseBody,
            });
          } else {
            reject(
              new GoOnHttpError(
                res.statusCode || 0,
                res.statusMessage || "Unknown",
              ),
            );
          }
        });
      });

      req.on("error", (err) => {
        reject(new GoOnClientError(`Network error: ${err.message}`));
      });

      req.on("timeout", () => {
        req.destroy();
        reject(new GoOnClientError(`Request timed out after ${this.timeout}s`));
      });

      req.write(body);
      req.end();
    });
  }

  /**
   * POST a JSON body and resolve with the response as soon as the status
   * line and headers arrive, leaving the body to be streamed incrementally.
   * Rejects with `GoOnClientError` on network errors.
   */
  private _requestStream(
    path: string,
    body: string,
  ): Promise<http.IncomingMessage> {
    return new Promise((resolve, reject) => {
      const parsedUrl = new url.URL(this._url(path));
      const isHttps = parsedUrl.protocol === "https:";
      const lib = isHttps ? https : http;

      const req = lib.request(
        {
          hostname: parsedUrl.hostname,
          port: parsedUrl.port ? parseInt(parsedUrl.port, 10) : undefined,
          path: parsedUrl.pathname + parsedUrl.search,
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            Accept: "text/event-stream",
            "Content-Length": Buffer.byteLength(body).toString(),
          },
          timeout: 0, // SSE streams are long-lived; socket idle time is expected
        },
        (res) => {
          resolve(res);
        },
      );

      req.on("error", (err) => {
        reject(new GoOnClientError(`Network error: ${err.message}`));
      });

      req.on("timeout", () => {
        req.destroy();
        reject(new GoOnClientError("Request timed out"));
      });

      req.write(body);
      req.end();
    });
  }

  /**
   * Chat streaming via SSE at `/chat/stream`.
   * Yields content chunks as they arrive from the server.
   *
   * This is a true streaming generator: each yielded value is the text
   * content of an SSE `chunk` event as it arrives over the wire (preferring
   * the `token` field, then legacy `content`/`text`). Frames without a text
   * field (e.g. `telemetry`, `done`) are yielded as their raw JSON payload so
   * nothing is silently dropped. `error` events throw a `GoOnClientError`.
   * The generator terminates on the `[DONE]` sentinel, on the terminal
   * `done`/`result` event, or when the server closes the connection.
   */
  async *chatStream(
    messages: Array<{ role: string; content: string }>,
    options?: {
      model?: string;
      temperature?: number;
      maxTokens?: number;
    },
  ): AsyncGenerator<string> {
    const payload = JSON.stringify({
      messages,
      model: options?.model,
      temperature: options?.temperature,
      max_tokens: options?.maxTokens,
      stream: true,
    });

    const res = await this._requestStream("/chat/stream", payload);

    if (res.statusCode !== 200) {
      res.resume(); // drain so the connection can be closed/reused cleanly
      throw new GoOnHttpError(res.statusCode || 0, "chat/stream failed");
    }

    const parser = createSseParser();
    try {
      for await (const chunk of res) {
        const frames = parser.push(chunk.toString("utf-8"));
        for (const frame of frames) {
          const text = this._sseFrameText(frame);
          if (text !== undefined) yield text;
        }
        if (parser.done) return;
      }

      // The server may omit the trailing blank line after the last frame.
      const tail = parser.flush();
      for (const frame of tail) {
        const text = this._sseFrameText(frame);
        if (text !== undefined) yield text;
      }
    } catch (err) {
      if (err instanceof GoOnClientError || err instanceof GoOnHttpError) {
        throw err;
      }
      throw new GoOnClientError(
        `SSE stream error: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      // Release the socket if the consumer stopped iterating early.
      res.destroy();
    }
  }

  /**
   * Convert a parsed SSE frame into a yielded string, or `undefined` to skip.
   *
   * - `error` events throw a `GoOnClientError` carrying the error message.
   * - Frames with a non-empty `token`/`content`/`text` field yield that text.
   * - Frames whose `token` field is present but empty (reasoning-only chunks)
   *   yield nothing.
   * - Any other frame (no text field at all) yields its raw JSON payload so
   *   that no frame is silently dropped.
   */
  private _sseFrameText(frame: SseFrame): string | undefined {
    const { data } = frame;

    if (frame.eventType === "error") {
      const message =
        typeof data.message === "string"
          ? data.message
          : JSON.stringify(data);
      throw new GoOnClientError(`Chat stream error: ${message}`);
    }

    const token = typeof data.token === "string" ? data.token : undefined;
    const content = typeof data.content === "string" ? data.content : undefined;
    const text = typeof data.text === "string" ? data.text : undefined;
    const piece = token || content || text;
    if (piece) return piece;
    if (token !== undefined) return undefined; // token field present but empty
    return JSON.stringify(data);
  }

  // ── Runtime API ───────────────────────────────────────────────────

  /** Get backend health status. */
  async health(): Promise<HealthResponse> {
    return (await this._jsonRpc("runtime.health")) as HealthResponse;
  }

  /** Get runtime health with detailed module status. */
  async runtimeHealth(): Promise<HealthResponse> {
    return (await this._jsonRpc("health.probes")) as HealthResponse;
  }

  /** Get runtime stability metrics. */
  async runtimeStability(): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("runtime.stability")) as Record<
      string,
      unknown
    >;
  }

  /** Initialize the runtime (ACP `initialize` handshake). */
  async initialize(
    profile: string = "full",
  ): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("initialize", {
      profile,
    })) as Record<string, unknown>;
  }

  /** Shutdown the runtime gracefully (ACP `shutdown`). */
  async shutdown(): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("shutdown")) as Record<string, unknown>;
  }

  // ── Governance API ────────────────────────────────────────────────

  /** Get governance status (HarnessBus profile + policy stats). */
  async governanceStatus(): Promise<GovernanceStatusResponse> {
    return (await this._jsonRpc(
      "governance.status",
    )) as GovernanceStatusResponse;
  }

  /** Get a governance plan by ID. */
  async governancePlanGet(planId: string): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("governance.plan.get", {
      plan_id: planId,
    })) as Record<string, unknown>;
  }

  /** Get recent audit entries. */
  async governanceAuditRecent(
    limit: number = 10,
  ): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("governance.audit.recent", {
      limit,
    })) as Record<string, unknown>;
  }

  /**
   * Verify the tamper-evident audit hash chain.
   * Optional params: `fromMs`/`toMs` export a time-window report,
   * `publicKeyHex` enables Ed25519 signature verification.
   */
  async governanceAuditVerify(params: {
    fromMs?: number;
    toMs?: number;
    publicKeyHex?: string;
  } = {}): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("governance.audit.verify", {
      from_ms: params.fromMs,
      to_ms: params.toMs,
      public_key_hex: params.publicKeyHex,
    })) as Record<string, unknown>;
  }

  // ── Observability API ─────────────────────────────────────────────

  /** Get health probes for all modules. */
  async healthProbes(): Promise<HealthProbesResponse> {
    return (await this._jsonRpc("health.probes")) as HealthProbesResponse;
  }

  /** Get runtime metrics as structured data. */
  async metricsGet(): Promise<MetricsResponse> {
    return (await this._jsonRpc("metrics.get")) as MetricsResponse;
  }

  /** Get Prometheus-formatted metrics. */
  async metricsPrometheus(): Promise<string> {
    return (await this._jsonRpc("metrics.prometheus")) as string;
  }

  /** Get an OpenTelemetry trace by ID. */
  async traceGet(traceId: string): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("trace.get", {
      trace_id: traceId,
    })) as Record<string, unknown>;
  }

  // ── Reliability API ───────────────────────────────────────────────

  /** Get circuit breaker status for all groups. */
  async breakerStatus(): Promise<BreakerStatusResponse> {
    return (await this._jsonRpc("breaker.status")) as BreakerStatusResponse;
  }

  /** Reset a circuit breaker by name. */
  async breakerReset(name: string): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("breaker.reset", { name })) as Record<
      string,
      unknown
    >;
  }

  /** Trigger garbage collection for the maintenance engine. */
  async maintenanceGc(): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("maintenance.gc")) as Record<string, unknown>;
  }

  // ── Checkpoint & Recovery API ─────────────────────────────────────

  /** Create a checkpoint for a conversation. */
  async checkpointCreate(
    conversationId: string,
    messages: Array<{ role: string; content: string }>,
    branch: string = "main",
  ): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("conversation.checkpoint.create", {
      conversation_id: conversationId,
      branch,
      messages,
    })) as Record<string, unknown>;
  }

  /** List available checkpoints. */
  async checkpointList(): Promise<CheckpointListResponse> {
    return (await this._jsonRpc("checkpoint.list")) as CheckpointListResponse;
  }

  /** Rollback a conversation to a checkpoint. */
  async conversationRollback(
    checkpointId: string,
  ): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("conversation.rollback", {
      checkpoint_id: checkpointId,
    })) as Record<string, unknown>;
  }

  // ── Workflow / Task API ───────────────────────────────────────────

  /** Execute a workflow by name with optional params. */
  async workflowExecute(
    workflow: string,
    params?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("workflow.execute", {
      workflow,
      params: params ?? {},
    })) as Record<string, unknown>;
  }

  /** Plan a task from a natural language description. */
  async taskPlan(description: string): Promise<TaskPlanResponse> {
    return (await this._jsonRpc("task.plan", {
      description,
    })) as TaskPlanResponse;
  }

  /** Execute a planned task. */
  async taskExecute(planId: string): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("task.execute", {
      plan_id: planId,
    })) as Record<string, unknown>;
  }

  // ── Learning & Intelligence API ───────────────────────────────────

  /** Get learning summary (FederatedRL + reinforcement stats). */
  async learningSummary(): Promise<LearningSummaryResponse> {
    return (await this._jsonRpc("learning.summary")) as LearningSummaryResponse;
  }

  /** Get agent selector status. */
  async selectorStatus(): Promise<SelectorStatusResponse> {
    return (await this._jsonRpc("selector.status")) as SelectorStatusResponse;
  }

  /** Trigger knowledge distillation. */
  async knowledgeDistill(): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("knowledge.distill")) as Record<
      string,
      unknown
    >;
  }

  /** Run offline RL alignment evaluation. */
  async rlAlignmentOfflineEval(): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("rl.alignment.offline_eval")) as Record<
      string,
      unknown
    >;
  }

  // ── Optimization & Operations API ─────────────────────────────────

  /** Get cost status across models. */
  async costStatus(): Promise<CostStatusResponse> {
    return (await this._jsonRpc("cost.status")) as CostStatusResponse;
  }

  /** Get configuration baseline. */
  async configBaseline(): Promise<ConfigBaselineResponse> {
    return (await this._jsonRpc("config.baseline")) as ConfigBaselineResponse;
  }

  /** Reload configuration at runtime. */
  async configReload(): Promise<Record<string, unknown>> {
    return (await this._jsonRpc("config.reload")) as Record<string, unknown>;
  }

  /** Get harness (governance) status. */
  async harnessStatus(): Promise<HarnessStatusResponse> {
    return (await this._jsonRpc("harness.status")) as HarnessStatusResponse;
  }

  // ── ACP Session Protocol ───────────────────────────────────────────

  /** Create a new ACP session. */
  async sessionNew(params: AcpSessionNewRequest): Promise<AcpSessionNewResponse> {
    return (await this._jsonRpc(
      "session/new",
      params as unknown as Record<string, unknown>,
    )) as AcpSessionNewResponse;
  }

  /** Send a prompt in an ACP session (returns stopReason). */
  async sessionPrompt(
    params: AcpSessionPromptRequest,
  ): Promise<{ stopReason: string }> {
    return (await this._jsonRpc(
      "session/prompt",
      params as unknown as Record<string, unknown>,
    )) as { stopReason: string };
  }

  /** Close an ACP session. */
  async sessionClose(params: AcpSessionCloseRequest): Promise<void> {
    await this._jsonRpc("session/close", params as unknown as Record<string, unknown>);
  }

  /** List active ACP sessions. */
  async sessionList(): Promise<AcpSessionListResponse> {
    return (await this._jsonRpc("session/list")) as AcpSessionListResponse;
  }

  /** Resume an existing ACP session. */
  async sessionResume(
    params: AcpSessionResumeRequest,
  ): Promise<SessionModeState> {
    return (await this._jsonRpc(
      "session/resume",
      params as unknown as Record<string, unknown>,
    )) as SessionModeState;
  }

  /** Change the mode of an ACP session. */
  async sessionSetMode(
    sessionId: string,
    modeId: string,
  ): Promise<void> {
    await this._jsonRpc("session/set_mode", { sessionId, modeId });
  }

  /** Set a configuration option for an ACP session. */
  async sessionSetConfigOption(
    sessionId: string,
    optionId: string,
    value: string,
  ): Promise<void> {
    await this._jsonRpc("session/set_config_option", {
      sessionId,
      optionId,
      value,
    });
  }
}
