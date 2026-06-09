import * as crypto from "crypto";
import { Logger } from "../logger";
import { FramedMessage } from "../protocolContract";

const log = Logger.forModule("framedProtocol");

// ── Framed stdio protocol ────────────────────────────────────────────

// Minimal type declarations for ReadableStream (available at runtime via
// Node.js stream/web but not in @types/node@16.x type definitions).
export interface ReadableStreamLike<R = unknown> {
  getReader(): ReadableStreamDefaultReaderLike<R>;
}
export interface ReadableStreamDefaultReaderLike<R = unknown> {
  read(): Promise<{ done: boolean; value: R }>;
  cancel(): Promise<void>;
}

export interface FramedReaderCallbacks {
  onMessage: (_msg: FramedMessage) => void;
  onPong?: () => void;
  onError?: (_err: Error) => void;
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
export class FramedReader {
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
      } catch (err) {
        log.warn("processBuffer parse failed:", err);
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
      } catch (err) {
        log.warn("fallbackParse failed:", err);
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
export class FramedWriter {
  private writeFn: (_data: Uint8Array) => boolean;
  private queue: Uint8Array[] = [];
  private messageCounter = 0;
  private sessionId: string;

  constructor(writeFn: (_data: Uint8Array) => boolean) {
    this.writeFn = writeFn;
    this.sessionId = `session-${Date.now()}-${crypto.randomUUID().slice(0, 8)}`;
  }

  /**
   * Encode and write a message. Returns true if the message was written
   * directly, false if it was queued (backpressure).
   */
  writeMessage(msg: unknown): boolean {
    const enriched: FramedMessage = {
      message_id: `msg-${this.sessionId}-${++this.messageCounter}`,
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
    } catch (err) {
      log.warn("writeFrame failed:", err);
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
      } catch (err) {
        log.warn("flush failed:", err);
        break;
      }
    }
  }

  get queuedCount(): number {
    return this.queue.length;
  }
}
