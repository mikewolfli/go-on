import { ChildProcess, spawn } from "child_process";
import * as http from "http";
import * as os from "os";
import * as vscode from "vscode";
import { i18n, MessageKeys } from "./i18n";
import {
  FramedMessage,
  isAllowedProtocolMode,
  normalizeProtocolMode,
  protocolContract,
} from "./protocolContract";
import { StreamEvent, StreamRequestOptions } from "./managerTypes";

interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number;
  method: string;
  params?: unknown;
}

interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: number;
  result?: unknown;
  error?: {
    code: number;
    message: string;
    data?: unknown;
  };
}

interface PendingRequest {
  resolve: (_value: unknown) => void;
  reject: (_reason?: unknown) => void;
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : {};
}

/**
 * Extract the keyring account name from a provider's env-var / keyring URI.
 *
 * Handles two formats:
 * - `keyring://go-on/{account_name}` — extract account_name directly
 * - `OPENAI_API_KEY` — old-style env var name, convert to lowercase
 *
 * Returns the account name suitable for use with the system keyring
 * (e.g. `copilot_api_key`, `openai_api_key`).
 */
function secretNameForEnvVar(envVar: string): string {
  const normalized = String(envVar || "").trim();
  if (!normalized) {
    return "";
  }
  // Handle keyring://go-on/{name} URIs
  const keyringPrefix = "keyring://go-on/";
  if (normalized.startsWith(keyringPrefix)) {
    return normalized.slice(keyringPrefix.length);
  }
  // Legacy env-var name: GITHUB_COPILOT_TOKEN → github_copilot_token
  if (normalized === "GITHUB_COPILOT_TOKEN") {
    return "github_copilot_token";
  }
  return normalized.toLowerCase();
}

// ── Framed stdio protocol ────────────────────────────────────────────

// Minimal type declarations for ReadableStream (available at runtime via
// Node.js stream/web but not in @types/node@16.x type definitions).
interface ReadableStreamLike<R = unknown> {
  getReader(): ReadableStreamDefaultReaderLike<R>;
}
interface ReadableStreamDefaultReaderLike<R = unknown> {
  read(): Promise<{ done: boolean; value: R }>;
  cancel(): Promise<void>;
}

interface FramedReaderCallbacks {
  onMessage: (msg: FramedMessage) => void;
  onPong?: () => void;
  onError?: (err: Error) => void;
}

/**
 * FramedReader parses a length-prefixed framed protocol from a
 * ReadableStream<Uint8Array>.
 *
 * Frame format: [4-byte BigEndian uint32 payload_length][JSON payload]
 *
 * In compatibility mode, if the first bytes of the stream do not look like a
 * valid length prefix (e.g. the stream starts with a JSON object character),
 * the reader falls back to line-by-line JSON parsing.
 */
class FramedReader {
  private chunks: Uint8Array[] = [];
  private chunkLength = 0;
  private reader: ReadableStreamDefaultReaderLike<Uint8Array> | null = null;
  private callbacks: FramedReaderCallbacks;
  private compatibilityMode: boolean;
  private lineBuffer = "";
  private aborted = false;
  private fallbackActive = false;
  private readonly MAX_FRAME_SIZE = 1024 * 1024; // 1 MB

  constructor(
    stream: ReadableStreamLike<Uint8Array>,
    callbacks: FramedReaderCallbacks,
    compatibilityMode = false,
  ) {
    this.callbacks = callbacks;
    this.compatibilityMode = compatibilityMode;
    this.reader = stream.getReader();
    void this.readLoop();
  }

  /** Manually feed data into the reader (used by adapter code). */
  feed(data: Uint8Array): void {
    if (this.aborted) return;
    this.chunks.push(data);
    this.chunkLength += data.length;
    this.processBuffer();
  }

  abort(): void {
    this.aborted = true;
    if (this.reader) {
      void this.reader.cancel();
    }
    this.chunks = [];
    this.chunkLength = 0;
  }

  private async readLoop(): Promise<void> {
    try {
      while (!this.aborted && this.reader) {
        const { done, value } = await this.reader.read();
        if (done) break;
        if (value && value.length > 0) {
          this.feed(value);
        }
      }
    } catch (err) {
      if (!this.aborted) {
        this.callbacks.onError?.(
          err instanceof Error ? err : new Error(String(err)),
        );
      }
    }
  }

  private concatBuffer(): Uint8Array {
    if (this.chunks.length === 1) return this.chunks[0];
    const result = new Uint8Array(this.chunkLength);
    let offset = 0;
    for (const chunk of this.chunks) {
      result.set(chunk, offset);
      offset += chunk.length;
    }
    return result;
  }

  private processBuffer(): void {
    if (this.fallbackActive) {
      this.fallbackParse(this.concatBuffer());
      return;
    }
    if (this.chunkLength < 4) return;

    const view = this.concatBuffer();
    let offset = 0;

    // In compatibility mode, check heuristic on the first 4 bytes
    if (this.compatibilityMode && offset === 0 && this.shouldFallback(view)) {
      this.fallbackActive = true;
      this._outputLine(
        "[framed] compatibility mode: switching to line-by-line parsing",
      );
      this.fallbackParse(view);
      return;
    }

    while (offset + 4 <= view.length) {
      const payloadLen = new DataView(
        view.buffer,
        view.byteOffset + offset,
        4,
      ).getUint32(0, false); // BigEndian

      if (payloadLen > this.MAX_FRAME_SIZE) {
        this.callbacks.onError?.(
          new Error(
            `Frame payload too large: ${payloadLen} bytes (max ${this.MAX_FRAME_SIZE})`,
          ),
        );
        return;
      }

      if (offset + 4 + payloadLen > view.length) {
        break; // incomplete frame, wait for more data
      }

      offset += 4;
      const jsonBytes = view.slice(offset, offset + payloadLen);
      offset += payloadLen;

      const jsonStr = new TextDecoder().decode(jsonBytes);
      try {
        const msg = JSON.parse(jsonStr) as FramedMessage;

        // Handle heartbeat pong internally
        if (msg.type === "heartbeat.pong") {
          this.callbacks.onPong?.();
        } else if (msg.type === "heartbeat.ping") {
          // Auto-respond to heartbeat ping from the server
          // (the GoOnManager's writer will send back a pong)
          // Let the message through so the manager can respond
          this.callbacks.onMessage(msg);
        } else {
          this.callbacks.onMessage(msg);
        }
      } catch {
        this.callbacks.onError?.(
          new Error(`Invalid JSON in frame: ${jsonStr.slice(0, 200)}`),
        );
      }
    }

    // Keep remaining bytes
    if (offset < view.length) {
      this.chunks = [view.slice(offset)];
      this.chunkLength = this.chunks[0].length;
    } else {
      this.chunks = [];
      this.chunkLength = 0;
    }
  }

  /**
   * Heuristic: if the first byte is `{` (0x7B) the stream likely starts with
   * raw JSON rather than a 4-byte length prefix. Also if the uint32 decoded
   * from the first 4 bytes would exceed 1 MB, treat as invalid.
   */
  private shouldFallback(data: Uint8Array): boolean {
    // If first byte is '{', it's almost certainly raw JSON
    if (data.length > 0 && data[0] === 0x7b) return true;
    // Check if the 4-byte prefix decodes to an unreasonable size
    if (data.length >= 4) {
      const len = new DataView(data.buffer, data.byteOffset, 4).getUint32(
        0,
        false,
      );
      if (len > this.MAX_FRAME_SIZE) return true;
    }
    return false;
  }

  private fallbackParse(data: Uint8Array): void {
    const text = new TextDecoder().decode(data);
    this.lineBuffer += text;

    const lines = this.lineBuffer.split("\n");
    this.lineBuffer = lines.pop() || "";

    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const msg = JSON.parse(trimmed) as FramedMessage;
        this.callbacks.onMessage(msg);
      } catch {
        // Ignore invalid JSON in fallback mode
      }
    }
  }

  private _outputLine(msg: string): void {
    // eslint-disable-next-line no-console
    console.log(msg);
  }
}

/**
 * FramedWriter writes messages with a length-prefixed framing protocol:
 * [4-byte BigEndian uint32 payload_length][JSON payload]
 *
 * Each outgoing message is automatically annotated with a `message_id`
 * for deduplication. Supports queuing when the underlying transport's
 * buffer is full (backpressure).
 */
class FramedWriter {
  private writeFn: (data: Uint8Array) => boolean;
  private queue: Uint8Array[] = [];
  private messageCounter = 0;

  constructor(writeFn: (data: Uint8Array) => boolean) {
    this.writeFn = writeFn;
  }

  /**
   * Encode and write a message. Returns true if the message was written
   * directly, false if it was queued (backpressure).
   */
  writeMessage(msg: unknown): boolean {
    const enriched: FramedMessage = {
      message_id: `msg-${++this.messageCounter}-${Date.now()}`,
      ...(msg as Record<string, unknown>),
    };
    const json = JSON.stringify(enriched);
    const encoder = new TextEncoder();
    const jsonBytes = encoder.encode(json);

    const frame = new Uint8Array(4 + jsonBytes.length);
    new DataView(frame.buffer).setUint32(0, jsonBytes.length, false); // BE
    frame.set(jsonBytes, 4);

    return this.writeFrame(frame);
  }

  private writeFrame(frame: Uint8Array): boolean {
    if (this.queue.length > 0) {
      this.queue.push(frame);
      return false;
    }
    try {
      const canWrite = this.writeFn(frame);
      if (!canWrite) {
        this.queue.push(frame);
      }
      return canWrite;
    } catch {
      this.queue.push(frame);
      return false;
    }
  }

  /** Flush any queued messages. */
  flush(): void {
    while (this.queue.length > 0) {
      const frame = this.queue[0];
      try {
        const canWrite = this.writeFn(frame);
        if (canWrite) {
          this.queue.shift();
        } else {
          break;
        }
      } catch {
        break;
      }
    }
  }

  get queuedCount(): number {
    return this.queue.length;
  }
}

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
  private _reconnectAttempts = 0;
  private readonly maxReconnectAttempts = 3;
  private _shutdownInProgress = false;
  /**
   * Guards start()/stop() from concurrent calls.
   * When a start() or stop() is in progress, subsequent calls await the
   * in-flight operation instead of being silently dropped.
   */
  private _operationPromise: Promise<void> | null = null;
  private _closeListener: (() => void) | null = null;
  private _reconnectTimer: ReturnType<typeof setTimeout> | undefined;
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
  private writerWriteFn: ((data: Uint8Array) => boolean) | null = null;

  // ── Heartbeat ──
  private readonly HEARTBEAT_INTERVAL_MS = 30000;
  private readonly HEARTBEAT_TIMEOUT_MS = 90000;
  private heartbeatIntervalTimer: ReturnType<typeof setInterval> | null = null;
  private heartbeatTimeoutTimer: ReturnType<typeof setTimeout> | null = null;

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
    // If an operation is already in flight, await it instead of dropping.
    if (this._operationPromise) {
      return this._operationPromise;
    }
    if (this.process) {
      throw new Error("Go-On is already running");
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

      // Prune known API key env vars — keys should only flow through keyring://
      const {
        ANTHROPIC_API_KEY: _ANTHROPIC_API_KEY,
        OPENAI_API_KEY: _OPENAI_API_KEY,
        DEEPSEEK_API_KEY: _DEEPSEEK_API_KEY,
        GITHUB_COPILOT_TOKEN: _GITHUB_COPILOT_TOKEN,
        ...safeEnv
      } = process.env;

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

        // ── Traditional line-based protocol on stdout ──
        this.process.stdout?.on("data", (data: Buffer) => {
          const output = data.toString();
          this._outputChannel?.appendLine(output.trimEnd());

          // Buffered line-frame protocol: accumulate data and split by newlines
          this.stdoutBuffer += output;
          // Cap buffer at 1MB to prevent memory leak
          if (this.stdoutBuffer.length > 1024 * 1024) {
            this.stdoutBuffer = this.stdoutBuffer.slice(-1024 * 1024);
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
            } catch {
              // JSON.parse failed — buffer the fragment and prepend to the
              // next chunk. This handles fragmented JSON responses that are
              // split across two (or more) newline-delimited chunks.
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
            reject: () => {
              // Health probe failed via stdin. The process may be running
              // in HTTP mode (acp_http, mcp_http, or adaptive→http) where
              // stdin is not consumed. We fall through — the HTTP probe
              // below will handle this case.
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
        healthReq.on("error", () => {
          if (!settled) {
            // HTTP health check failed — the server may be running in
            // stdio mode instead. The health monitoring loop will pick
            // up any connectivity issues later.
          }
        });
        healthReq.setTimeout(3000, () => {
          healthReq.destroy();
        });
      }, 500);

      this.process.stderr?.on("data", (data: Buffer) => {
        const text = data.toString();
        stderrBuffer += text;
        if (stderrBuffer.length > 4000) {
          stderrBuffer = stderrBuffer.slice(-4000);
        }
        this._outputChannel?.appendLine("[stderr] " + text.trimEnd());
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
        this._clearHeartbeat();

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

    return startPromise.finally(() => {
      this._operationPromise = null;
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
      } catch {
        proc.kill(); // fallback
      }
    } else {
      proc.kill("SIGKILL");
    }
  }

  stop(): void {
    if (this._operationPromise) {
      // An operation is in flight; can't stop now. Caller should retry.
      return;
    }
    this._shutdownInProgress = true;
    this._reconnectAttempts = 0;

    // Clean up framed protocol
    this._clearFramedProtocol();
    this._clearHeartbeat();

    // Save startupConfig before clearing, so we can restore it
    // if a concurrent start() call spawned a new process during our await
    const savedConfig = this._startupConfig;
    this._startupConfig = undefined;

    // Clean up timers first
    if (this._reconnectTimer) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = undefined;
    }

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
    void this.attemptReconnect();
  }

  private _scheduleReconnect(): void {
    this._reconnectTimer = setTimeout(() => {
      void this.attemptReconnect();
    }, 5000);
  }

  private async attemptReconnect(): Promise<void> {
    if (this._shutdownInProgress || !this._startupConfig) return;

    this._reconnectAttempts++;
    if (this._reconnectAttempts > this.maxReconnectAttempts) {
      this._outputChannel?.appendLine(
        `[reconnect] Max reconnect attempts (${this.maxReconnectAttempts}) reached, giving up.`,
      );
      void vscode.window.showWarningMessage(
        i18n.getMessage(MessageKeys.reconnectMaxAttempts, [
          String(this.maxReconnectAttempts),
        ]),
      );
      this._reconnectAttempts = 0;
      return;
    }

    this._outputChannel?.appendLine(
      `[reconnect] Attempt ${this._reconnectAttempts}/${this.maxReconnectAttempts} in 2 seconds...`,
    );

    // Wait before reconnecting
    // Use a local timer so stop() can't clear it via this._reconnectTimer.
    // If stop() clears the shared timer reference, we'd never reach the
    // shutdown check below and _shutdownInProgress would stay true forever.
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 2000);
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
        `[reconnect] Reconnect attempt ${this._reconnectAttempts} succeeded.`,
      );
      this._reconnectAttempts = 0;
    } catch (error) {
      this._outputChannel?.appendLine(
        `[reconnect] Attempt ${this._reconnectAttempts} failed: ${error}`,
      );
      // Only schedule retry if not shutting down
      if (!this._shutdownInProgress) {
        this._scheduleReconnect();
      }
    }
  }

  isRunning(): boolean {
    return this.process !== null;
  }

  /** Expose reconnect state for diagnostics. */
  getReconnectState(): { attempts: number; maxAttempts: number } {
    return {
      attempts: this._reconnectAttempts,
      maxAttempts: this.maxReconnectAttempts,
    };
  }

  setRuntimeEnvOverrides(overrides: Record<string, string>): void {
    this.runtimeEnvOverrides = {
      ...this.runtimeEnvOverrides,
      ...overrides,
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
            // Fallback to HTTP if available
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            const g = globalThis as any;
            if (
              typeof globalThis !== "undefined" &&
              // eslint-disable-next-line @typescript-eslint/no-unsafe-member-access
              typeof g.fetch === "function"
            ) {
              (async () => {
                try {
                  const httpUrl = `${protocolContract.runtime.baseUrl}/rpc`;
                  // eslint-disable-next-line @typescript-eslint/no-unsafe-call,@typescript-eslint/no-unsafe-assignment
                  const httpResponse = await g.fetch(httpUrl, {
                    method: "POST",
                    headers: { "Content-Type": "application/json" },
                    body: requestStr,
                  });
                  const envelope = await httpResponse.json();
                  // Extract the inner result from the JSON-RPC envelope
                  // to match the stdin path's response shape.
                  const result =
                    envelope &&
                    typeof envelope === "object" &&
                    "result" in envelope
                      ? envelope.result
                      : envelope;
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

      const response = await g.fetch(url, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(bodyObj),
        signal,
      });

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

      // Consume SSE stream using ReadableStream
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
        const lines = buffer.split("\n");
        buffer = lines.pop() || "";

        for (const line of lines) {
          const trimmed = line.trim();
          if (!trimmed || !trimmed.startsWith("data: ")) continue;

          const dataStr = trimmed.slice(6).trim();

          // Check for OpenAI-style [DONE] marker
          if (dataStr === protocolContract.openai.streamDoneMarker) {
            callbacks?.onDone();
            return fullContent;
          }

          try {
            // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
            const event: StreamEvent = JSON.parse(dataStr);

            switch (event.type) {
              case "token":
                if (event.content) {
                  fullContent += event.content;
                  callbacks?.onToken(event.content);
                }
                break;
              case "done":
                callbacks?.onDone();
                return fullContent;
              case "error": {
                const err = new Error(event.message || "Stream error");
                callbacks?.onError(err);
                throw err;
              }
              default: {
                // Unknown event type — handle delta-style content (OpenAI compat)
                // eslint-disable-next-line @typescript-eslint/no-explicit-any
                const content = (event as any).content as string | undefined;
                if (content) {
                  fullContent += content;
                  callbacks?.onToken(content);
                }
                break;
              }
            }
          } catch (parseErr) {
            if (
              parseErr instanceof Error &&
              parseErr.message !== "Stream error"
            ) {
              this._outputChannel?.appendLine(
                `[stream] SSE parse error: ${parseErr.message} for data: ${dataStr}`,
              );
            } else if (parseErr instanceof Error) {
              throw parseErr; // re-throw stream errors
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
      method: "chat.cancel",
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

  // ── Heartbeat ──

  /** Start the heartbeat interval that sends pings every 30s. */
  private _startHeartbeat(): void {
    this._clearHeartbeat();

    // Send the first ping after a short delay
    this._scheduleHeartbeatPing();

    // Set heartbeat timeout: if no pong within 90s, reconnect
    this._resetHeartbeatTimeout();
  }

  private _scheduleHeartbeatPing(): void {
    this.heartbeatIntervalTimer = setInterval(() => {
      this._sendHeartbeatPing();
    }, this.HEARTBEAT_INTERVAL_MS);
  }

  private _sendHeartbeatPing(): void {
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

  /** Reset the heartbeat timeout timer (called on pong received). */
  private _resetHeartbeatTimeout(): void {
    if (this.heartbeatTimeoutTimer) {
      clearTimeout(this.heartbeatTimeoutTimer);
      this.heartbeatTimeoutTimer = null;
    }
    this.heartbeatTimeoutTimer = setTimeout(() => {
      this._handleHeartbeatTimeout();
    }, this.HEARTBEAT_TIMEOUT_MS);
  }

  /** Called when no pong is received within the timeout window. */
  private _handleHeartbeatTimeout(): void {
    this._outputChannel?.appendLine(
      "[heartbeat] Timeout — no pong received, triggering reconnection",
    );
    void vscode.window.showWarningMessage(
      "Go-On connection lost (heartbeat timeout). Reconnecting...",
    );

    // Clean up the current process
    this._clearFramedProtocol();
    this._clearHeartbeat();

    if (this.process) {
      const proc = this.process;
      this.process = null;
      proc.kill();
    }

    // Trigger reconnection
    if (this._startupConfig && !this._shutdownInProgress) {
      void this.attemptReconnect();
    }
  }

  /** Stop all heartbeat timers. */
  private _clearHeartbeat(): void {
    if (this.heartbeatIntervalTimer) {
      clearInterval(this.heartbeatIntervalTimer);
      this.heartbeatIntervalTimer = null;
    }
    if (this.heartbeatTimeoutTimer) {
      clearTimeout(this.heartbeatTimeoutTimer);
      this.heartbeatTimeoutTimer = null;
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
      } catch {
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
      this._resetHeartbeatTimeout();
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
        enqueue: (chunk: Uint8Array) => void;
        close: () => void;
        error: (err: Error) => void;
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

    // Start heartbeat
    this._startHeartbeat();
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
    } catch {
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
    } catch {
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
