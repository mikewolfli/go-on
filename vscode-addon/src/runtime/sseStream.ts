/**
 * Server-Sent Events (SSE) parsing utilities.
 *
 * Supports full SSE frame format as defined in contracts/sse-protocol.md:
 * ```
 * event: <event_type>
 * data: <json_payload>
 *
 * ```
 *
 * Each parsed frame returns an object with:
 *   - `eventType`: the SSE event type (e.g. "chunk", "done", "telemetry", "error")
 *   - `data`: the parsed JSON payload with `_event_type` injected
 *
 * @see {@link ../../contracts/sse-protocol.md} for the full SSE protocol contract.
 */

/**
 * A parsed SSE frame with event type and data payload.
 */
export interface SseFrame {
  /** The SSE event type, e.g. "chunk", "done", "telemetry", "error". */
  eventType?: string;
  /** Parsed JSON data payload, with `_event_type` injected when an event type is present. */
  data: Record<string, unknown>;
}

/**
 * Parse a chunk of raw SSE text into structured frames.
 *
 * The parser:
 * 1. Splits the text on `\n\n` boundaries to identify SSE frames, falling back
 *    to `\n` (line-delimited) when no blank-line separator is present
 * 2. Normalises CRLF line endings to LF before splitting
 * 3. Within each frame, extracts `event:` and `data:` lines
 * 4. Injects the event type as `_event_type` into the parsed JSON payload
 * 5. Represents `[DONE]` sentinel frames as completion frames (mirroring the
 *    GUI's StreamProcessor) instead of silently dropping them
 * 6. Skips malformed JSON without throwing
 *
 * @param text - Raw SSE text chunk (may contain multiple frames)
 * @returns Array of parsed SseFrame (empty if none found)
 */
export function parseSseChunk(text: string): SseFrame[] {
  const results: SseFrame[] = [];

  // Normalise CRLF → LF before splitting (mirrors the GUI's StreamProcessor,
  // which strips \r from each chunk; see gui/src/backend/state.rs).
  const normalized = text.replace(/\r/g, "");

  // Delimiter selection mirrors the GUI: prefer \n\n frame boundaries, but
  // fall back to \n (line-delimited) when the chunk contains no blank-line
  // separator so single-line "data: {...}" streams still parse frame-by-frame.
  const delimiter = normalized.includes("\n\n") ? "\n\n" : "\n";
  const frames = normalized.split(delimiter);
  for (const frame of frames) {
    if (!frame.trim()) continue;

    let currentEventType: string | undefined;
    let currentData: string | undefined;

    const lines = frame.split("\n");
    for (const line of lines) {
      const trimmed = line.trim();

      if (trimmed.startsWith("event: ")) {
        currentEventType = trimmed.slice(7).trim();
      } else if (trimmed.startsWith("data: ")) {
        currentData = trimmed.slice(6).trim();
      } else if (trimmed.startsWith("data:")) {
        // Handle "data:{json}" without space after colon
        currentData = trimmed.slice(5).trim();
      }
    }

    // [DONE] sentinel: signal stream completion instead of silently dropping
    // it. Mirrors gui/src/backend/state.rs:96-107:
    //  - with an event type: wrapped as {"_event_type": ev, "data": "[DONE]"}
    //  - bare (no event type): the GUI yields the raw "[DONE]" string; in the
    //    SseFrame shape that is represented as eventType "done" so downstream
    //    routing (runtimeManager) can finalize the response.
    if (currentData === "[DONE]") {
      if (currentEventType) {
        results.push({
          eventType: currentEventType,
          data: { _event_type: currentEventType, data: "[DONE]" },
        });
      } else {
        results.push({
          eventType: "done",
          data: { data: "[DONE]" },
        });
      }
      continue;
    }

    // Skip frames with no data payload
    if (!currentData) continue;

    try {
      const parsed = JSON.parse(currentData) as Record<string, unknown>;
      // Inject event type as _event_type for downstream routing
      if (currentEventType) {
        parsed._event_type = currentEventType;
      }
      results.push({ eventType: currentEventType, data: parsed });
    } catch {
      // Skip frames with malformed JSON — log silently
    }
  }

  return results;
}

/**
 * Parse a single SSE data line.
 *
 * Simpler API for callers that process one line at a time.
 * Returns the parsed object with `_event_type` injected, or null on
 * empty/malformed/[DONE] lines.
 *
 * @param line - A single SSE line (e.g. `data: {"content":"hello"}`)
 * @returns The parsed object with _event_type, or null.
 */
export function parseSseDataLine(line: string): Record<string, unknown> | null {
  const trimmed = line.trim();
  if (!trimmed) return null;

  const dataPrefix = "data: ";
  if (!trimmed.startsWith(dataPrefix)) return null;

  const dataStr = trimmed.slice(dataPrefix.length).trim();

  // Check for stream completion markers
  if (dataStr === "[DONE]") {
    return null;
  }

  try {
    const parsed = JSON.parse(dataStr) as Record<string, unknown>;
    return parsed;
  } catch {
    return null;
  }
}

/**
 * Extract the content field from an SSE data object.
 */
export function extractSseContent(
  data: Record<string, unknown>,
): string | undefined {
  const content = data.content;
  return typeof content === "string" ? content : undefined;
}
