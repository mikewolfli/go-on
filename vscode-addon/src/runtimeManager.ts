import { ChildProcess, spawn } from "child_process";
import * as http from "http";
import * as os from "os";
import * as vscode from "vscode";
import { GoOnClient } from "go-on-sdk-typescript";
import { Logger } from "./logger";

const log = Logger.forModule("runtimeManager");
import { i18n, MessageKeys } from "./i18n";
import {
  FramedMessage,
  isAllowedProtocolMode,
  normalizeProtocolMode,
  protocolContract,
} from "./protocolContract";
import { StreamRequestOptions } from "./managerTypes";
import {
  FramedReader,
  FramedWriter,
  ReadableStreamLike,
} from "./runtime/framedProtocol";
import { parseSseChunk } from "./runtime/sseStream";
import {
  JsonRpcRequest,
  JsonRpcResponse,
  PendingRequest,
  asRecord,
  secretNameForEnvVar,
} from "./runtime/jsonRpc";
import { HeartbeatManager } from "./runtime/heartbeat";
import { ReconnectManager } from "./runtime/reconnect";

// ── Framed stdio protocol ────────────────────────────────────────────
// FramedReader, FramedWriter, and supporting types moved to
// runtime/framedProtocol.ts -- imported above

// ── GoOnManager ──────────────────────────────────────────────────────

export class GoOnManager {
  private process: ChildProcess | null = null;
  private requestId = 0;
  private pendingRequests = new Map<number, PendingRequest>();
  private statusItems: vscode.TreeItem[] = [];
  private runtimeEnvOverrides: Record<string, string> = {};
  private providerReadyCache?: { checkedAt: number; ready: boolean };
  private lastWizardPromptAt = 0;
  private _outputChannel?: vscode.OutputChannel;
  private stdoutBuffer = "";
  private lineBuffer = "";
  /**
   * Removed fixed limit (was 3). Now uses unlimited exponential backoff
   * to support long-running multi-agent workflows (10+ min).
   * Backoff: min(2000 * 2^attempt, 300000)ms with 30% jitter.
   * @see https://github.com/go-on/go-on/issues/connection-resilience
   */
  private _shutdownInProgress = false;
  /**
   * Guards start()/stop() from concurrent calls.
   * When a start() or stop() is in progress, subsequent calls await the
   * in-flight operation instead of being silently dropped.
   */
  private _operationPromise: Promise<void> | null = null;
  private _closeListener: (() => void) | null = null;
  private reconnect: ReconnectManager;
  private heartbeat: HeartbeatManager;
  private _startupConfig?: {
    configPath: string;
    executablePath: string;
    cwd: string;
    protocolMode: string;
    useFramedProtocol?: boolean;
  };

  // ── Framed protocol support ──
  private useFramedProtocol = false;
  private framedReader: FramedReader | null = null;
  private framedWriter: FramedWriter | null = null;
  private writerWriteFn: ((_data: Uint8Array) => boolean) | null = null;

  // ── Heartbeat managed by HeartbeatManager ──
  // Fields moved to runtime/heartbeat.ts

  // ── Message deduplication ──
  private readonly DEDUP_CAPACITY = 100;
  private readonly recentMessageIds: Set<string> = new Set();
  private readonly messageIdQueue: string[] = [];

  /** Connect a VS Code OutputChannel so Go-On process output is visible to users. */
  setOutputChannel(channel: vscode.OutputChannel): void {
    this._outputChannel = channel;
  }

  private classifyRpcErrorKind(message: string, data?: unknown): string {
    const details = asRecord(data);
    const explicit =
      typeof details.kind === "string" ? details.kind : undefined;
    if (explicit && explicit.trim().length > 0) {
      return explicit;
    }

    const lower = String(message || "").toLowerCase();
    if (lower.includes("pua")) return "PuaViolation";
    if (lower.includes("budget")) return "BudgetExceeded";
    if (
      lower.includes("hardening policy denied") ||
      lower.includes("sandbox")
    ) {
      return "SandboxBlocked";
    }
    return "GeneralError";
  }

  private formatRpcError(error: {
    code: number;
    message: string;
    data?: unknown;
  }): string {
    const kind = this.classifyRpcErrorKind(error.message, error.data);
    const errorData = asRecord(error.data);
    const detail = typeof errorData.detail === "string" ? errorData.detail : "";
    const context = detail.includes(
      protocolContract.errors.requestErrorContextPrefix,
    )
      ? protocolContract.errors.requestErrorContextPrefix
      : "none";
    return `rpc_error:${error.code}:${kind}:${error.message} (context=${context})`;
  }

  constructor() {
    this.reconnect = new ReconnectManager(
      () => this._doReconnect(),
      this._outputChannel,
    );
    this.heartbeat = new HeartbeatManager(
      () => this._sendFramedPing(),
      () => this._sendLegacyPing(),
      () => this._onHeartbeatTimeout(),
      this._outputChannel,
    );
    this.updateStatus();
  }

  async start(
    configPath: string,
    executablePath: string,
    cwd: string,
    protocolMode: string,
    useFramedProtocol = false,
  ): Promise<void> {
    if (this._shutdownInProgress) {
      throw new Error("Go-On is shutting down. Please wait and try again.");
    }
    // B51-20: If process already exists, resolve immediately.
    if (this.process) {
      return Promise.resolve();
    }
    // B51-20: If _operationPromise was a failed start, allow new attempt.
    // The .finally() on the start promise clears _operationPromise on completion.
    // If it's still set, another start() is in-flight — await it.
    if (this._operationPromise) {
      return this._operationPromise;
    }

    // Store config for potential reconnection
    this._startupConfig = {
      configPath,
      executablePath,
      cwd,
      protocolMode,
      useFramedProtocol,
    };
    this.useFramedProtocol = useFramedProtocol;

    const startPromise = new Promise<void>((resolve, reject) => {
      // Assign immediately so concurrent callers await the same operation
      this._operationPromise = startPromise;
      let settled = false;
      let resolved = false;
      let stderrBuffer = "";

      const args = ["--config", configPath, "--verbose"];
      const normalizedProtocolMode = normalizeProtocolMode(
        protocolMode || "from_config",
      );
      if (!isAllowedProtocolMode(normalizedProtocolMode)) {
        reject(
          new Error(
            `Invalid protocol mode '${protocolMode}'. Allowed values: from_config, ${protocolContract.protocol.supportedModes.join(", ")}`,
          ),
        );
        return;
      }

      if (normalizedProtocolMode !== "from_config") {
        args.push("--protocol-mode", normalizedProtocolMode);
      }

      // B51-21: Dynamically detect and prune sensitive env vars
      // Keys should only flow through keyring://
      const sensitiveSuffixes = [
        "_API_KEY",
        "_SECRET",
        "_TOKEN",
        "_SECRET_KEY",
      ];
      const safeEnv: Record<string, string | undefined> = {};
      for (const key of Object.keys(process.env)) {
        if (
          sensitiveSuffixes.some((suffix) => key.toUpperCase().endsWith(suffix))
        ) {
          continue; // skip sensitive vars — they should only flow through keyring://
        }
        safeEnv[key] = process.env[key];
      }

      this.process = spawn(executablePath, args, {
        cwd,
        env: {
          ...safeEnv,
          ...this.runtimeEnvOverrides,
        },
        stdio: ["pipe", "pipe", "pipe"],
      });

      let startupTimeout: NodeJS.Timeout | undefined = setTimeout(() => {
        this.process?.kill();
        reject(new Error("Go-On startup timeout"));
      }, 30000);

      if (useFramedProtocol && this._trySetupFramedProtocol()) {
        // Framed protocol was set up successfully (with heartbeat started)
      } else {
        // Framed protocol not available / not requested — use line-based
        this.useFramedProtocol = false;
        // Start legacy mode heartbeat (JSON-RPC ping/pong)
        this.heartbeat.startLegacy();

        // ── Traditional line-based protocol on stdout ──
        this.process.stdout?.on("data", (data: Buffer) => {
          const output = data.toString();
          this._outputChannel?.appendLine(output.trimEnd());

          // Buffered line-frame protocol: accumulate data and split by newlines
          this.stdoutBuffer += output;
          // Cap buffer at 1MB to prevent memory leak
          if (this.stdoutBuffer.length > 1024 * 1024) {
            // Cut at line boundary to avoid truncating mid-line
            const cutPoint = this.stdoutBuffer.length - 1024 * 1024;
            const lastNewline = this.stdoutBuffer.lastIndexOf("\n", cutPoint);
            if (lastNewline >= 0) {
              // Cut at the last complete line boundary before the cut point
              this.stdoutBuffer = this.stdoutBuffer.slice(lastNewline + 1);
            } else {
              // No newline found — cut at boundary and note the dropped bytes
              this.stdoutBuffer = this.stdoutBuffer.slice(cutPoint);
            }
            // dropped bytes tracking reserved for future metrics
          }
          const lines = this.stdoutBuffer.split("\n");
          // Keep the last (potentially incomplete) fragment in the buffer
          this.stdoutBuffer = lines.pop() || "";

          for (const line of lines) {
            const trimmed = line.trim();
            if (!trimmed) {
              continue;
            }

            // Prepend any previously buffered incomplete line fragment
            const combined = this.lineBuffer
              ? this.lineBuffer + trimmed
              : trimmed;
            this.lineBuffer = "";

            try {
              const response: JsonRpcResponse = JSON.parse(combined);
              // Legacy heartbeat: any valid JSON-RPC response means backend is alive
              this.heartbeat.resetLegacyTimeout();
              const pending = this.pendingRequests.get(response.id);
              if (pending) {
                this.pendingRequests.delete(response.id);
                if (response.error) {
                  pending.reject(
                    new Error(this.formatRpcError(response.error)),
                  );
                } else {
                  pending.resolve(response.result);
                }
              }
            } catch (err) {
              log.warn("JSON parse failed, buffering:", err);
              this.lineBuffer = combined;
              // Cap lineBuffer at 1MB to prevent memory leak
              if (this.lineBuffer.length > 1024 * 1024) {
                this.lineBuffer = "";
              }
            }
          }
        });
      }

      // Short delay to let the process initialize before probing
      setTimeout(() => {
        if (this.process?.stdin) {
          // Send a JSON-RPC health probe (works for stdio-based modes:
          // acp_stdio, mcp_stdio, and adaptive when resolved to stdio).
          // The server responds with a JSON-RPC result on stdout.
          const healthRequest: JsonRpcRequest = {
            jsonrpc: "2.0",
            id: ++this.requestId,
            method: "runtime.health",
          };
          this.pendingRequests.set(healthRequest.id, {
            resolve: (_v: unknown) => {
              if (startupTimeout) {
                clearTimeout(startupTimeout);
                startupTimeout = undefined;
              }
              if (!resolved) {
                resolved = true;
                resolve();
              }
            },
            reject: (err: unknown) => {
              // Health probe failed via stdin. The process may be running
              // in HTTP mode (acp_http, mcp_http, or adaptive→http) where
              // stdin is not consumed. We fall through — the HTTP probe
              // below will handle this case.
              this._outputChannel?.appendLine(
                `[health] stdin health probe rejected: ${err instanceof Error ? err.message : String(err)}`,
              );
            },
          });
          this.process.stdin.write(JSON.stringify(healthRequest) + "\n");
        }

        // Also try an HTTP health check (works for HTTP-based modes:
        // acp_http, mcp_http, and adaptive when resolved to http).
        const baseUrl = protocolContract.runtime.baseUrl;
        const healthUrl = `${baseUrl}${protocolContract.runtime.healthPath}`;
        const healthReq = http.get(healthUrl, (res) => {
          if (res.statusCode === 200 && startupTimeout && !resolved) {
            clearTimeout(startupTimeout);
            startupTimeout = undefined;
            resolved = true;
            resolve();
          }
          res.resume();
        });
        healthReq.on("error", (err: Error) => {
          if (!settled) {
            this._outputChannel?.appendLine(
              `[health] HTTP health probe failed: ${err.message}`,
            );
            // HTTP health check failed — the server may be running in
            // stdio mode instead. The health monitoring loop will pick
            // up any connectivity issues later.
          }
        });
        healthReq.setTimeout(3000, () => {
          healthReq.destroy();
        });

        // Protocol version discovery: probe the /protocol/version endpoint
        // to confirm the backend API level and unify with the GUI's discovery pattern.
        // This is fire-and-forget — we log the result but don't block startup.
        const protoUrl = `${baseUrl}/protocol/version`;
        const protoReq = http.get(protoUrl, (res) => {
          let body = "";
          res.on("data", (chunk: Buffer) => {
            body += chunk.toString();
          });
          res.on("end", () => {
            if (res.statusCode === 200) {
              this._outputChannel?.appendLine(
                `[protocol] version discovery succeeded: ${body.slice(0, 200)}`,
              );
            } else {
              this._outputChannel?.appendLine(
                `[protocol] version discovery returned HTTP ${res.statusCode}`,
              );
            }
          });
        });
        protoReq.on("error", (err: Error) => {
          this._outputChannel?.appendLine(
            `[protocol] version discovery failed: ${err.message}`,
          );
        });
        protoReq.setTimeout(3000, () => {
          protoReq.destroy();
        });
      }, 500);

      this.process.stderr?.on("data", (data: Buffer) => {
        const text = data.toString();
        stderrBuffer += text;
        if (stderrBuffer.length > 4000) {
          stderrBuffer = stderrBuffer.slice(-4000);
        }
        // Sanitize stderr: strip potential secrets (API keys, tokens) from output
        // channel display while preserving the raw buffer for error diagnostics.
        const sanitized = text
          .replace(/(sk-[a-zA-Z0-9]{20,})/g, "sk-***REDACTED***")
          .replace(/([A-Za-z0-9+/=]{40,})/g, "***REDACTED***");
        this._outputChannel?.appendLine("[stderr] " + sanitized.trimEnd());
      });

      this.process.on("close", (code: number) => {
        if (settled) return;
        settled = true;
        this._outputChannel?.appendLine(`[exit] code ${code}`);
        this._shutdownInProgress = false;
        if (!this._startupConfig) return;
        const failedBeforeStartup = !resolved;
        this.process = null;

        // Clean up framed protocol and heartbeat
        this._clearFramedProtocol();
        this.heartbeat.stopAll();

        // Reject all pending requests — process exited unexpectedly
        for (const [, pending] of this.pendingRequests) {
          pending.reject(new Error("Go-On process exited unexpectedly"));
        }
        this.pendingRequests.clear();

        this.updateStatus();

        if (startupTimeout) {
          clearTimeout(startupTimeout);
          startupTimeout = undefined;
        }

        if (failedBeforeStartup) {
          settled = true;
          const details = stderrBuffer.trim();
          reject(
            new Error(
              `Go-On process exited prematurely with code ${code}. ${details || "No stderr output."}`,
            ),
          );
        } else {
          // process exited after startup — attempt reconnect
          this._handleProcessExit();
        }
      });

      this.process.on("error", (error: Error) => {
        if (settled) return;
        settled = true;
        clearTimeout(startupTimeout);
        this._outputChannel?.appendLine(`[error] ${error.message}`);
        this.process = null;
        reject(error);
      });
    });

    return startPromise
      .finally(() => {
        this._operationPromise = null;
      })
      .then(() => {
        // B51-03a: Register persistent close listener for normal operation.
        // The inner close listener inside the start() promise only monitors
        // the process while the promise is pending. Once startup succeeds,
        // this listener ensures crashes during normal operation are caught.
        if (this.process) {
          this.process.on("close", (code: number) => {
            if (this._shutdownInProgress) return;
            if (!this._startupConfig) return;
            this._outputChannel?.appendLine(
              `[exit] code ${code} (post-startup)`,
            );
            this._handleProcessExit();
          });
        }
      });
  }

  private async forceKillProcess(proc: ChildProcess): Promise<void> {
    if (os.platform() === "win32") {
      try {
        await new Promise<void>((resolve, reject) => {
          const kill = spawn(
            "taskkill",
            ["/F", "/T", "/PID", String(proc.pid)],
            {
              stdio: "ignore",
            },
          );
          kill.on("exit", (code) => {
            if (code === 0) resolve();
            else reject(new Error(`taskkill exited with code ${code}`));
          });
          kill.on("error", reject);
          kill.unref();
        });
      } catch (err) {
        log.warn("forceKill fallback:", err);
        proc.kill();
      }
    } else {
      proc.kill("SIGKILL");
    }
  }

  async stop(): Promise<void> {
    if (this._operationPromise) {
      // V10: Wait for the in-flight operation to complete, then proceed with stop.
      // Previously this returned early, abandoning the stop request.
      try {
        await this._operationPromise;
      } catch {
        // If the in-flight operation failed, we still need to proceed with stop.
      }
    }
    this._shutdownInProgress = true;
    this.reconnect.reset();

    // Clean up framed protocol
    this._clearFramedProtocol();
    this.heartbeat.stopAll();

    // Save startupConfig before clearing, so we can restore it
    // if a concurrent start() call spawned a new process during our await
    const savedConfig = this._startupConfig;
    this._startupConfig = undefined;

    // Cancel any pending reconnection
    this.reconnect.cancel();

    // Save process reference to local variable before clearing
    const proc = this.process;
    this.process = null;

    if (proc) {
      // Graceful shutdown: send SIGTERM first
      proc.kill("SIGTERM");

      // If process doesn't exit within 5 seconds, force kill
      // Use local variable so timer still works after this.process is cleared
      const forceKillTimer = setTimeout(() => {
        if (proc) {
          this._outputChannel?.appendLine(
            "[shutdown] SIGTERM timeout, sending SIGKILL",
          );
          void this.forceKillProcess(proc);
        }
      }, 5000);

      // Remove previous close listener if any (shouldn't happen, but be safe)
      this._closeListener?.();

      // Register close handler FIRST (avoids TOCTOU race: process could exit
      // between checking exitCode and attaching the listener)
      const closeHandler = () => {
        clearTimeout(forceKillTimer);
        this._shutdownInProgress = false;
      };
      proc.on("close", closeHandler);
      // Store cleanup so it can be removed if stop() is called again
      this._closeListener = () => {
        proc.off("close", closeHandler);
        this._closeListener = null;
      };

      // Then check if process already exited (close event already fired)
      if (proc.exitCode !== null && proc.exitCode !== undefined) {
        clearTimeout(forceKillTimer);
        this._shutdownInProgress = false;
      }
    }

    // Bug 3: Always restore _startupConfig if no new process was created.
    // If stop() cleared the config but start() hasn't created a new process,
    // restore it so potential reconnect or re-initialization can use it.
    if (!this.process) {
      this._startupConfig = savedConfig;
    }

    if (!proc) {
      this.updateStatus();
      return;
    }

    this.updateStatus();
  }

  private _handleProcessExit(): void {
    if (this._shutdownInProgress || !this._startupConfig) return;
    void this._doReconnect();
  }

  /** Core reconnect logic (called by ReconnectManager). */
  private async _doReconnect(): Promise<void> {
    if (this._shutdownInProgress || !this._startupConfig) return;

    // Use exponential backoff with jitter
    const delay = this.reconnect.backoffMs(this.reconnect.attempts);
    this._outputChannel?.appendLine(
      `[reconnect] Attempt ${this.reconnect.attempts + 1} in ${delay}ms (unlimited retries)...`,
    );

    // Wait before reconnecting with a local timer so stop() can't clear it.
    await new Promise<void>((resolve) => {
      setTimeout(resolve, delay);
    });

    // Check again after delay — stop() may have been called
    if (this._shutdownInProgress || !this._startupConfig) return;

    try {
      await this.start(
        this._startupConfig.configPath,
        this._startupConfig.executablePath,
        this._startupConfig.cwd,
        this._startupConfig.protocolMode,
        this._startupConfig.useFramedProtocol,
      );
      this._outputChannel?.appendLine(
        `[reconnect] Reconnect attempt ${this.reconnect.attempts} succeeded.`,
      );
      this.reconnect.reset();
    } catch (error) {
      this._outputChannel?.appendLine(
        `[reconnect] Attempt ${this.reconnect.attempts} failed: ${error}`,
      );
      // Only schedule retry if not shutting down
      if (!this._shutdownInProgress) {
        this.reconnect.schedule();
      }
    }
  }

  isRunning(): boolean {
    return this.process !== null;
  }

  /**
   * Trigger reconnection from StatusMonitor (or other observers).
   * Does nothing if process is running, shutting down, or no startup config.
   */
  triggerReconnectFromObserver(): void {
    if (this.process || this._shutdownInProgress || !this._startupConfig)
      return;
    this._outputChannel?.appendLine(
      "[reconnect] Observer triggered reconnection",
    );
    void this._doReconnect();
  }

  /** Expose reconnect state for diagnostics. */
  getReconnectState(): { attempts: number; maxAttempts: number } {
    return {
      attempts: this.reconnect.attempts,
      maxAttempts: Infinity, // unlimited retries with exponential backoff
    };
  }

  setRuntimeEnvOverrides(overrides: Record<string, string>): void {
    this.runtimeEnvOverrides = {
      ...this.runtimeEnvOverrides,
      ...overrides,
    };
  }

  /**
   * Process a pasted/dropped image into the multimodal content format
   * accepted by the chat completion API.
   *
   * Returns a content block suitable for inclusion in a messages payload.
   * The dataUrl is expected to be a base64 data URL (e.g. "data:image/png;base64,...").
   */
  static processImageAttachment(
    dataUrl: string,
    _mimeType: string,
    _name: string,
  ): {
    type: "image_url";
    image_url: { url: string; detail: string };
  } {
    return {
      type: "image_url",
      image_url: {
        url: dataUrl,
        detail: "auto",
      },
    };
  }

  /**
   * Process a pasted/dropped file (non-image) into the multimodal content format
   * accepted by the chat completion API.
   */
  static processFileAttachment(
    dataUrl: string,
    mimeType: string,
    filename: string,
  ): {
    type: "file";
    file_data: { data: string; filename: string; mime_type: string };
  } {
    return {
      type: "file",
      file_data: {
        data: dataUrl,
        filename: filename || "attachment",
        mime_type: mimeType || "application/octet-stream",
      },
    };
  }

  async sendRequest(
    method: string,
    params?: unknown,
    options?: { skipProviderGuard?: boolean },
  ): Promise<unknown> {
    if (!this.process) {
      throw new Error("Go-On is not running");
    }

    if (!options?.skipProviderGuard && this.requiresAiProvider(method)) {
      const ready = await this.isAnyAiProviderReady();
      if (!ready) {
        await this.notifyAndOpenSetupWizard();
        throw new Error(
          `${protocolContract.errors.providerNotReady} ${protocolContract.errors.setupWizardOpened}`,
        );
      }
    }

    const id = ++this.requestId;
    const request: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method,
      params,
    };

    return new Promise((resolve, reject) => {
      // Bug 11: Check process availability BEFORE setting pending request.
      // Previously the pending request was set first, then checked — if the
      // check failed the pending entry was orphaned until timeout.
      if (!this.process || !this.process.stdin) {
        reject(new Error("Go-On process not available or stdin not connected"));
        return;
      }

      const timeoutId = setTimeout(() => {
        if (this.pendingRequests.has(id)) {
          this.pendingRequests.delete(id);
          reject(new Error("Request timeout"));
        }
      }, 30000);

      this.pendingRequests.set(id, {
        resolve: (v: unknown) => {
          clearTimeout(timeoutId);
          resolve(v);
        },
        reject: (e: unknown) => {
          clearTimeout(timeoutId);
          reject(e);
        },
      });

      try {
        if (this.useFramedProtocol && this.framedWriter) {
          // Use framed protocol: write with length prefix and message_id
          const queued = !this.framedWriter.writeMessage(request);
          if (queued) {
            this._outputChannel?.appendLine(
              "[framed] RPC message queued (backpressure)",
            );
          }
        } else {
          // Traditional newline-delimited JSON-RPC
          const requestStr = JSON.stringify(request) + "\n";
          const canWrite = this.process.stdin.write(requestStr);
          if (!canWrite) {
            this._outputChannel?.appendLine("[warn] RPC stdin backpressure");
            // Fallback to HTTP via the shared TypeScript SDK client (reuses the
            // JSON-RPC envelope handling / timeout / retry logic instead of a
            // hand-written fetch).
            (async () => {
              try {
                const client = new GoOnClient({
                  baseUrl: protocolContract.runtime.baseUrl,
                });
                const params =
                  typeof request.params === "object" && request.params !== null
                    ? (request.params as Record<string, unknown>)
                    : {};
                const result = await client.request(
                  request.method,
                  params,
                );
                if (this.pendingRequests.has(id)) {
                  const pending = this.pendingRequests.get(id);
                  this.pendingRequests.delete(id);
                  pending?.resolve(result);
                }
                return;
              } catch (httpErr) {
                this._outputChannel?.appendLine(
                  `[warn] HTTP fallback also failed: ${httpErr}`,
                );
                // HTTP fallback also failed, continue to reject via timeout
              }
            })();
          }
        }
      } catch (writeErr) {
        this.pendingRequests.delete(id);
        reject(new Error(`Failed to write to process stdin: ${writeErr}`));
        return;
      }
    });
  }

  async sendStreamingRequest(
    method: string,
    params?: unknown,
    options?: StreamRequestOptions,
  ): Promise<string> {
    if (!this.process) {
      throw new Error("Go-On is not running");
    }

    if (!options?.skipProviderGuard && this.requiresAiProvider(method)) {
      const ready = await this.isAnyAiProviderReady();
      if (!ready) {
        await this.notifyAndOpenSetupWizard();
        throw new Error(
          `${protocolContract.errors.providerNotReady} ${protocolContract.errors.setupWizardOpened}`,
        );
      }
    }

    const baseUrl = protocolContract.runtime.baseUrl;
    const chatPath = protocolContract.openai.chatCompletionsPath;
    const url = `${baseUrl}${chatPath}`;

    const { callbacks, signal } = options || {};

    // Build the request body — wrap messages if needed, enable streaming
    const bodyObj: Record<string, unknown> =
      params &&
      typeof params === "object" &&
      "messages" in (params as Record<string, unknown>)
        ? { ...(params as Record<string, unknown>), stream: true }
        : { messages: params, stream: true };

    // Try streaming via HTTP SSE
    try {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const g = globalThis as any;
      if (typeof g.fetch !== "function") {
        throw new Error("fetch API not available");
      }

      // Combine the caller's abort signal (if any) with a default timeout so
      // a hung backend cannot stall the stream forever (same contract as the
      // TypeScript/Rust/Python SDKs).
      const controller = new AbortController();
      let timedOut = false;
      // Keep a stable listener reference so removeEventListener below actually
      // removes it (a fresh arrow function each time would leak listeners).
      const onAbort = () => controller.abort();
      if (signal?.aborted) {
        controller.abort();
      } else {
        signal?.addEventListener("abort", onAbort, { once: true });
      }
      const timeoutMs = options?.timeout ?? 30_000;
      const timeoutId = setTimeout(() => {
        timedOut = true;
        controller.abort();
      }, timeoutMs);

      let response: Response;
      try {
        response = await g.fetch(url, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(bodyObj),
          signal: controller.signal,
        });
      } catch (e) {
        clearTimeout(timeoutId);
        signal?.removeEventListener("abort", onAbort);
        if (timedOut) {
          throw new Error(`Streaming request timed out after ${timeoutMs}ms`);
        }
        throw e;
      }
      clearTimeout(timeoutId);
      signal?.removeEventListener("abort", onAbort);

      if (!response.ok) {
        // Non-200 response — fall back to non-streaming JSON-RPC
        this._outputChannel?.appendLine(
          `[stream] HTTP ${response.status} — falling back to non-streaming`,
        );
        const result = await this.sendRequest(method, params, {
          skipProviderGuard: options?.skipProviderGuard,
        });
        const text =
          typeof result === "string" ? result : JSON.stringify(result);
        callbacks?.onDone();
        return text;
      }

      const contentType = response.headers.get("content-type") || "";
      const body = response.body;

      if (!contentType.includes("text/event-stream") || !body) {
        // Response is not SSE — read as JSON
        const json = await response.json();
        const text = typeof json === "string" ? json : JSON.stringify(json);
        callbacks?.onDone();
        return text;
      }

      // Consume SSE stream using ReadableStream and parseSseChunk
      // eslint-disable-next-line @typescript-eslint/no-unsafe-call,@typescript-eslint/no-unsafe-assignment
      const reader = body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      let fullContent = "";

      // eslint-disable-next-line no-constant-condition
      while (true) {
        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value as Uint8Array, { stream: true });

        // Use parseSseChunk to extract structured SSE frames
        const frames = parseSseChunk(buffer);
        // Keep only the trailing partial frame (text after the last \n\n frame
        // separator). Frames split across chunk boundaries must not be dropped:
        // parseSseChunk splits on \n\n, so any data without a trailing separator
        // is an incomplete frame that the next read completes.
        const lastBoundary = buffer.lastIndexOf("\n\n");
        buffer = lastBoundary >= 0 ? buffer.slice(lastBoundary + 2) : "";

        for (const frame of frames) {
          const eventData = frame.data;
          const eventType = frame.eventType || "";

          switch (eventType) {
            case "chunk": {
              const token =
                (eventData.token as string) ||
                (eventData.content as string) ||
                "";
              if (token) {
                fullContent += token;
                callbacks?.onToken(token);
              }
              break;
            }
            case "done":
              callbacks?.onDone();
              return fullContent;
            case "error": {
              const err = new Error(
                (eventData.message as string) || "Stream error",
              );
              callbacks?.onError(err);
              throw err;
            }
            case "sub_agent": {
              log.info("[SSE] sub_agent:", eventData.agent, eventData.status);
              break;
            }
            case "command": {
              log.info(
                "[SSE] command:",
                eventData.command,
                "exit=",
                eventData.exit_code,
              );
              break;
            }
            default: {
              // Unknown event type — handle delta-style content (OpenAI compat)
              // For OpenAI-style data: lines without event:, try content field
              const content =
                (eventData.content as string) ||
                (eventData.token as string) ||
                "";
              if (content) {
                fullContent += content;
                callbacks?.onToken(content);
              }
              break;
            }
          }
        }
      }

      // Stream ended without explicit done event — treat as complete
      callbacks?.onDone();
      return fullContent;
    } catch (err: unknown) {
      if ((err as Error)?.name === "AbortError") {
        const abortErr = new Error("Request aborted");
        callbacks?.onError(abortErr);
        throw abortErr;
      }

      this._outputChannel?.appendLine(
        `[stream] HTTP streaming failed: ${err instanceof Error ? err.message : String(err)} — falling back to non-streaming`,
      );

      // Fallback to non-streaming JSON-RPC
      const result = await this.sendRequest(method, params, {
        skipProviderGuard: options?.skipProviderGuard,
      });
      const text = typeof result === "string" ? result : JSON.stringify(result);
      callbacks?.onDone();
      return text;
    }
  }

  async sendCancelRequest(): Promise<void> {
    if (!this.process || !this.process.stdin) {
      return; // Silently ignore if not running
    }

    const id = ++this.requestId;
    const cancelRequest: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method: "$/cancel_request",
      params: {},
    };

    try {
      if (this.useFramedProtocol && this.framedWriter) {
        this.framedWriter.writeMessage(cancelRequest);
      } else {
        const requestStr = JSON.stringify(cancelRequest) + "\n";
        this.process.stdin.write(requestStr);
      }
      this._outputChannel?.appendLine("[cancel] Sent cancel request");
    } catch (err) {
      this._outputChannel?.appendLine(
        `[cancel] Failed to send: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }

  // ── Framed protocol helpers ──

  private _clearFramedProtocol(): void {
    if (this.framedReader) {
      this.framedReader.abort();
      this.framedReader = null;
    }
    this.framedWriter = null;
    this.writerWriteFn = null;
  }

  // ── Heartbeat (delegated to HeartbeatManager) ──

  /** Send a framed heartbeat ping via the FramedWriter. */
  private _sendFramedPing(): void {
    if (!this.useFramedProtocol || !this.framedWriter) return;
    try {
      this.framedWriter.writeMessage({ type: "heartbeat.ping" });
      this._outputChannel?.appendLine("[heartbeat] ping sent");
    } catch (err) {
      this._outputChannel?.appendLine(
        `[heartbeat] Failed to send ping: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }

  /** Send a legacy JSON-RPC ping via stdin. */
  private _sendLegacyPing(): void {
    if (!this.process?.stdin || this._shutdownInProgress) return;
    try {
      const pingMsg =
        JSON.stringify({ jsonrpc: "2.0", id: 0, method: "runtime.health" }) +
        "\n";
      this.process.stdin.write(pingMsg);
      this._outputChannel?.appendLine("[legacy-heartbeat] ping sent");
    } catch (err) {
      this._outputChannel?.appendLine(
        `[legacy-heartbeat] Failed to send ping: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }

  /** Handle heartbeat timeout from HeartbeatManager. */
  private _onHeartbeatTimeout(): void {
    this._clearFramedProtocol();
    this.heartbeat.stopAll();
    if (this.process) {
      const proc = this.process;
      this.process = null;
      proc.kill();
    }
    if (this._startupConfig && !this._shutdownInProgress) {
      void this._doReconnect();
    }
  }

  // ── Message deduplication ──

  /**
   * Check if a message has already been seen (via message_id).
   * Returns true if the message is a duplicate and should be ignored.
   * Tracks the last DEDUP_CAPACITY message_ids.
   */
  private _isDuplicateMessage(msg: FramedMessage): boolean {
    const msgId = msg.message_id;
    if (!msgId) return false; // No message_id — can't deduplicate

    if (this.recentMessageIds.has(msgId)) {
      this._outputChannel?.appendLine(
        `[dedup] Ignoring duplicate message: ${msgId}`,
      );
      return true;
    }

    // Track the new message_id
    this.recentMessageIds.add(msgId);
    this.messageIdQueue.push(msgId);

    // Evict oldest entries when at capacity
    if (this.messageIdQueue.length > this.DEDUP_CAPACITY) {
      const oldest = this.messageIdQueue.shift();
      if (oldest) {
        this.recentMessageIds.delete(oldest);
      }
    }

    return false;
  }

  /**
   * Try to set up the framed protocol (FramedReader + FramedWriter + heartbeat).
   * Returns true if successful, false if the runtime doesn't support ReadableStream.
   */
  private _trySetupFramedProtocol(): boolean {
    // Build a writer function for stdin that FramedWriter will use
    this.writerWriteFn = (frame: Uint8Array): boolean => {
      if (!this.process?.stdin) return false;
      try {
        return this.process.stdin.write(Buffer.from(frame));
      } catch (err) {
        log.warn("stdin write failed:", err);
        return false;
      }
    };
    this.framedWriter = new FramedWriter(this.writerWriteFn);

    if (!this.process) {
      this._outputChannel?.appendLine(
        "[framed] No process available, cannot set up framed protocol",
      );
      this._clearFramedProtocol();
      return false;
    }
    const nodeStdout = this.process.stdout;
    if (!nodeStdout) {
      this._outputChannel?.appendLine(
        "[framed] No stdout stream available, cannot set up framed protocol",
      );
      return false;
    }

    // Create callback handlers for the FramedReader
    const onStdoutMessage = (msg: FramedMessage): void => {
      // Deduplication: skip duplicate messages
      if (this._isDuplicateMessage(msg)) return;

      // Handle heartbeat ping from the server: respond with pong
      if (msg.type === "heartbeat.ping") {
        if (this.framedWriter) {
          this.framedWriter.writeMessage({ type: "heartbeat.pong" });
        }
        return;
      }

      // Route to pending request by JSON-RPC id
      if (typeof msg.id === "number") {
        const response = msg as unknown as JsonRpcResponse;
        const pending = this.pendingRequests.get(response.id);
        if (pending) {
          this.pendingRequests.delete(response.id);
          if (response.error) {
            pending.reject(new Error(this.formatRpcError(response.error)));
          } else {
            pending.resolve(response.result);
          }
        }
      }
    };

    const onStdoutPong = (): void => {
      this.heartbeat.resetFramedTimeout();
    };

    const onStdoutError = (err: Error): void => {
      this._outputChannel?.appendLine(`[framed] reader error: ${err.message}`);
    };

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const ReadableStreamCtor = (globalThis as any).ReadableStream;
    if (!ReadableStreamCtor) {
      this._outputChannel?.appendLine(
        "[framed] ReadableStream not available, falling back to line protocol",
      );
      this._clearFramedProtocol();
      return false;
    }

    // Create a ReadableStream from the Node.js Readable stream
    const rs: ReadableStreamLike<Uint8Array> = new ReadableStreamCtor({
      start(controller: {
        enqueue: (_chunk: Uint8Array) => void;
        close: () => void;
        error: (_err: Error) => void;
      }) {
        nodeStdout.on("data", (chunk: Buffer) => {
          controller.enqueue(new Uint8Array(chunk));
        });
        nodeStdout.on("end", () => {
          controller.close();
        });
        nodeStdout.on("error", (err: Error) => {
          controller.error(err);
        });
      },
    });

    this.framedReader = new FramedReader(
      rs,
      {
        onMessage: onStdoutMessage,
        onPong: onStdoutPong,
        onError: onStdoutError,
      },
      true, // compatibility mode: detect non-framed data
    );

    // Log stdout data (separate listener from framed reader)
    nodeStdout.on("data", (data: Buffer) => {
      this._outputChannel?.appendLine(`[stdout] ${data.toString().trimEnd()}`);
    });

    // Start framed heartbeat
    this.heartbeat.startFramed();
    return true;
  }

  private requiresAiProvider(method: string): boolean {
    return new Set([
      "chat",
      "workflow.execute",
      "task.plan",
      "task.execute",
      "learning.summary",
      "primary_secondary.summary",
    ]).has(method);
  }

  public async isAnyAiProviderReady(): Promise<boolean> {
    const now = Date.now();
    if (
      this.providerReadyCache &&
      now - this.providerReadyCache.checkedAt < 5000
    ) {
      return this.providerReadyCache.ready;
    }

    try {
      const report = asRecord(
        await this.sendRequest("health.probes", undefined, {
          skipProviderGuard: true,
        }),
      );

      const probes = asRecord(report.probes);
      const dependencies = Array.isArray(probes.dependencies)
        ? (probes.dependencies as unknown[])
        : [];

      const providerComponent = dependencies
        .map((component) => asRecord(component))
        .find((component) => component.name === "provider_dependencies");

      if (!providerComponent) {
        this.providerReadyCache = { checkedAt: now, ready: false };
        return false;
      }

      const details = asRecord(providerComponent.details);
      const ready = Number(details.ready ?? 0);
      const total = Number(details.total ?? 0);
      const isReady = total > 0 && ready > 0;
      this.providerReadyCache = { checkedAt: now, ready: isReady };
      return isReady;
    } catch (err) {
      log.warn("isAnyAiProviderReady failed:", err);
      this.providerReadyCache = { checkedAt: now, ready: false };
      return false;
    }
  }

  private async fetchAvailableProviders(): Promise<Array<{
    name: string;
    label: string;
    description: string;
    detail: string;
    envVar?: string;
    apiKeyEnv?: string;
    secretKeyEnv?: string;
  }> | null> {
    try {
      const catalog = await this.sendRequest(
        "provider.catalog",
        {},
        { skipProviderGuard: true },
      );
      const result = catalog as Record<string, unknown>;
      const providers = result?.catalog as
        | Array<Record<string, unknown>>
        | undefined;
      if (!Array.isArray(providers)) return null;

      return providers.map((p: Record<string, unknown>) => ({
        name: String(p.name ?? ""),
        label:
          String(p.name ?? "")
            .charAt(0)
            .toUpperCase() + String(p.name ?? "").slice(1),
        description: String(p.type || p.url || ""),
        detail: `Model: ${p.model || "auto"} | Env: ${p.api_key_env ? "keyring" : p.secret_key_env ? "keyring" : "N/A"}`,
        envVar: String(p.api_key_env || p.secret_key_env || ""),
        apiKeyEnv: String(p.api_key_env || ""),
        secretKeyEnv: String(p.secret_key_env || ""),
      }));
    } catch (err) {
      log.warn("fetchAvailableProviders failed:", err);
      return null;
    }
  }

  private async notifyAndOpenSetupWizard(): Promise<void> {
    const now = Date.now();
    if (now - this.lastWizardPromptAt < 5000) {
      return;
    }
    this.lastWizardPromptAt = now;

    const action = await vscode.window.showWarningMessage(
      i18n.getMessage(MessageKeys.apiKeyMissing),
      i18n.getMessage(MessageKeys.quickSetup),
      i18n.getMessage(MessageKeys.openSettings),
      i18n.getMessage(MessageKeys.later),
    );

    if (action === "Later") return;

    if (action === "Open Settings") {
      await vscode.commands.executeCommand("go-on.openSettings");
      return;
    }

    // === Quick Setup flow ===

    // Step 1: Pick provider
    const providers = await this.fetchAvailableProviders();
    if (!providers || providers.length === 0) {
      vscode.window.showErrorMessage(
        "No AI providers found in your configuration.",
      );
      return;
    }

    const providerItems = providers.map((p) => ({
      label: p.label,
      description: p.description,
      detail: p.detail,
      provider: p,
    }));

    const picked = await vscode.window.showQuickPick(providerItems, {
      placeHolder: i18n.getMessage(MessageKeys.selectProvider),
      title: i18n.getMessage(MessageKeys.quickSetupStep1Title),
    });

    if (!picked) return;
    const selectedProvider = picked.provider;

    // Step 2: Enter API key
    const apiKey = await vscode.window.showInputBox({
      prompt: `Enter your API key for ${selectedProvider.label}`,
      password: true,
      placeHolder: "sk-...",
      validateInput: (value: string) => {
        if (!value || value.trim().length < 4) {
          return "API key must be at least 4 characters";
        }
        return null;
      },
    });

    if (!apiKey || apiKey.trim().length < 4) return;

    let secretKey = "";
    if (selectedProvider.secretKeyEnv) {
      const providedSecret = await vscode.window.showInputBox({
        prompt: `Enter your secret key for ${selectedProvider.label}`,
        password: true,
        placeHolder: "secret-...",
        validateInput: (value: string) => {
          if (!value || value.trim().length < 4) {
            return "Secret key must be at least 4 characters";
          }
          return null;
        },
      });

      if (!providedSecret || providedSecret.trim().length < 4) return;
      secretKey = providedSecret.trim();
    }

    // Step 3: Save to keyring and configure
    try {
      // Save to keyring
      const envVarName = selectedProvider.envVar || selectedProvider.apiKeyEnv;
      if (envVarName) {
        await vscode.commands.executeCommand("go-on.keyringSet", {
          name: secretNameForEnvVar(envVarName),
          value: apiKey.trim(),
        });
      }

      if (selectedProvider.secretKeyEnv && secretKey) {
        await vscode.commands.executeCommand("go-on.keyringSet", {
          name: secretNameForEnvVar(selectedProvider.secretKeyEnv),
          value: secretKey,
        });
      }

      // API keys are NOT injected into the backend process environment.
      // The generated config.toml uses `keyring://go-on/{name}_api_key` URIs
      // and the backend resolves these via system keyring (libsecret, Keychain,
      // Credential Manager). Skipping env overrides prevents secrets from leaking
      // to /proc/PID/environ.
      //
      // For headless/server deployments, operators can still set env vars directly
      // (e.g., DEEPSEEK_API_KEY=sk-xxx) which the backend's load_secret_value()
      // uses as fallback when keyring:// resolution fails.

      // Save provider selection to config.toml
      if (selectedProvider.name) {
        await vscode.commands.executeCommand(
          "go-on.applyDefaultConfigTemplate",
          {
            template: "config.toml.autopilot-adaptive",
          },
        );
      }

      // B51-19: After saving API keys, reload the backend config so it picks up the changes
      // without requiring a full restart.
      if (this.isRunning()) {
        try {
          await this.sendRequest("config.reload");
        } catch (reloadError: unknown) {
          const reloadMsg =
            reloadError instanceof Error
              ? reloadError.message
              : String(reloadError);
          this._outputChannel?.appendLine(
            `[setup] config.reload failed: ${reloadMsg}`,
          );
        }
      }

      vscode.window.showInformationMessage(
        i18n.getMessage(MessageKeys.apiKeyConfigured, selectedProvider.label),
      );
    } catch (error: unknown) {
      const msg = error instanceof Error ? error.message : String(error);
      vscode.window.showErrorMessage(
        i18n.getMessage(MessageKeys.setupFailed, msg),
      );
    }
  }

  private updateStatus(): void {
    this.statusItems = [
      new vscode.TreeItem(
        `Status: ${this.isRunning() ? "Running" : "Stopped"}`,
        vscode.TreeItemCollapsibleState.None,
      ),
    ];
    vscode.commands.executeCommand("go-on-status.refresh");
    vscode.commands.executeCommand("go-on.refreshStatusMonitor");
  }

  getStatusItems(): vscode.TreeItem[] {
    return this.statusItems;
  }
}

export class GoOnStatusProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
  private _onDidChangeTreeData: vscode.EventEmitter<
    vscode.TreeItem | undefined | null | void
  > = new vscode.EventEmitter<vscode.TreeItem | undefined | null | void>();
  readonly onDidChangeTreeData: vscode.Event<
    vscode.TreeItem | undefined | null | void
  > = this._onDidChangeTreeData.event;
  private manager: GoOnManager;

  constructor(_manager: GoOnManager) {
    this.manager = _manager;
  }

  refresh(): void {
    this._onDidChangeTreeData.fire();
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: vscode.TreeItem): Thenable<vscode.TreeItem[]> {
    if (!element) {
      return Promise.resolve(this.manager.getStatusItems());
    }
    return Promise.resolve([]);
  }
}
