/**
 * Parse a Server-Sent Events (SSE) data frame.
 *
 * Returns the content of the `data:` field (minus the optional `[DONE]` terminator),
 * or null if the frame is a completion marker.
 *
 * @param line - A single SSE line (e.g. `data: {"content":"hello"}`)
 * @returns The parsed object, or null if the frame signals completion.
 *
 * @see {@link ../../contracts/sse-protocol.md} for the full SSE protocol contract.
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
 * Parse a chunk of SSE text, extracting data payloads.
 *
 * SSE format:
 * ```
 * event: chunk
 * data: {"token":"hello"}
 *
 * event: done
 * data: {"response":"..."}
 *
 * data: [DONE]
 * ```
 *
 * @param text - Raw SSE text chunk (may contain multiple frames)
 * @returns Array of parsed data objects (empty if none found)
 *
 * @see {@link ../../contracts/sse-protocol.md} for the full SSE protocol contract.
 */
export function parseSseChunk(text: string): Record<string, unknown>[] {
  const results: Record<string, unknown>[] = [];
  const lines = text.split("\n");
  for (const line of lines) {
    const parsed = parseSseDataLine(line);
    if (parsed !== null) {
      results.push(parsed);
    }
  }
  return results;
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
