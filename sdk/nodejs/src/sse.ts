//! Minimal streaming Server-Sent Events (SSE) parser for the go-on SDK.
//!
//! Implements the wire format in `contracts/sse-protocol.md`:
//! - frames are delimited by a blank line (`\n\n`, CRLF normalized to LF)
//! - each frame has `event:` and one-or-more `data:` lines
//! - multi-line `data:` values are joined with `\n`
//! - comments (`: ...`) and unknown field prefixes are ignored
//! - `data: [DONE]` marks stream completion
//! - the event type is injected into the parsed payload as `_event_type`
//! - line length (1 MB) and total buffer (16 MB) limits prevent memory abuse
//!
//! The parser is incremental: feed raw text chunks with `push()` and receive
//! the frames completed by that chunk. Partial frames are buffered and
//! reassembled across `push()` calls.

import { GoOnClientError } from "./errors";

/** A single parsed SSE frame. */
export interface SseFrame {
  /** The SSE event type (e.g. "chunk", "done", "telemetry", "error"). */
  eventType?: string;
  /** Parsed JSON payload, with `_event_type` injected when an event type was present. */
  data: Record<string, unknown>;
}

/** Max length of a single SSE line (contract recommendation: 1 MB). */
export const SSE_MAX_LINE_LENGTH = 1024 * 1024;
/** Max buffered bytes while waiting for a frame boundary (contract recommendation: 16 MB). */
export const SSE_MAX_BUFFER_SIZE = 16 * 1024 * 1024;

/** Options for {@link createSseParser}. */
export interface SseParserOptions {
  /** Maximum length of a single line before the parser throws (default: 1 MB). */
  maxLineLength?: number;
  /** Maximum buffered bytes before the parser throws (default: 16 MB). */
  maxBufferSize?: number;
}

/**
 * An incremental SSE parser.
 *
 * ```ts
 * const parser = createSseParser();
 * for await (const chunk of stream) {
 *   for (const frame of parser.push(chunk.toString("utf-8"))) {
 *     // handle frame
 *   }
 *   if (parser.done) break;
 * }
 * ```
 */
export interface SseParser {
  /**
   * Feed raw text. Returns the frames completed by this chunk.
   * Throws `GoOnClientError` if a line or the buffer exceeds the configured limits.
   */
  push(chunk: string): SseFrame[];
  /** True once a `data: [DONE]` sentinel frame has been consumed. */
  readonly done: boolean;
  /**
   * Parse any trailing bytes left in the buffer (a final frame without a
   * trailing blank line). Call once when the underlying stream ends.
   */
  flush(): SseFrame[];
}

/**
 * Create an incremental SSE parser.
 *
 * Malformed JSON frames are skipped rather than thrown (per the contract:
 * a parse problem must not crash the consumer). `[DONE]` frames are not
 * returned; they set the `done` flag instead.
 */
export function createSseParser(options: SseParserOptions = {}): SseParser {
  const maxLineLength = options.maxLineLength ?? SSE_MAX_LINE_LENGTH;
  const maxBufferSize = options.maxBufferSize ?? SSE_MAX_BUFFER_SIZE;

  let buffer = "";
  let done = false;

  const parseFrame = (raw: string): SseFrame | null => {
    if (raw.trim() === "") return null;

    let eventType: string | undefined;
    const dataLines: string[] = [];

    for (const line of raw.split("\n")) {
      if (line === "" || line.startsWith(":")) continue; // blank / comment lines
      if (line.startsWith("event:")) {
        eventType = line.slice("event:".length).trim();
      } else if (line.startsWith("data:")) {
        dataLines.push(line.slice("data:".length).trim());
      }
      // Unknown field prefixes are ignored per the SSE spec.
    }

    if (dataLines.length === 0) return null;

    const rawData = dataLines.join("\n");
    if (rawData === "[DONE]") {
      done = true;
      return null;
    }

    try {
      const parsed = JSON.parse(rawData) as Record<string, unknown>;
      if (eventType !== undefined) {
        parsed._event_type = eventType;
      }
      return { eventType, data: parsed };
    } catch {
      // Malformed JSON: skip the frame rather than crashing the consumer.
      return null;
    }
  };

  const checkLineLength = (text: string): void => {
    // Only the trailing partial line needs checking here; complete lines
    // were already validated when their frame was extracted.
    const lastLine = text.slice(text.lastIndexOf("\n") + 1);
    if (lastLine.length > maxLineLength) {
      throw new GoOnClientError(`SSE line exceeds ${maxLineLength} bytes`);
    }
  };

  return {
    push(chunk: string): SseFrame[] {
      const frames: SseFrame[] = [];
      if (done || chunk === "") return frames;

      buffer += chunk.replace(/\r\n/g, "\n");
      if (buffer.length > maxBufferSize) {
        throw new GoOnClientError(`SSE buffer exceeds ${maxBufferSize} bytes`);
      }

      while (true) {
        const idx = buffer.indexOf("\n\n");
        if (idx === -1) break;
        const frame = buffer.slice(0, idx);
        buffer = buffer.slice(idx + 2);

        checkLineLength(frame);
        const parsed = parseFrame(frame);
        if (parsed) frames.push(parsed);
        if (done) break;
      }

      // Guard the partial line still awaiting a frame boundary.
      checkLineLength(buffer);
      return frames;
    },

    get done(): boolean {
      return done;
    },

    flush(): SseFrame[] {
      if (done || buffer.trim() === "") return [];
      const raw = buffer;
      buffer = "";
      checkLineLength(raw);
      const parsed = parseFrame(raw);
      return parsed ? [parsed] : [];
    },
  };
}
