# SSE Protocol Contract

## Overview

The Go-On backend streams chat completions and other long-running operations using **Server-Sent Events (SSE)** over HTTP. This document defines the wire format, event types, and parsing contract shared by all consumers (GUI, VSCode addon, CLI).

## Wire Format

Each SSE frame follows the standard SSE specification ([W3C SSE](https://html.spec.whatwg.org/multipage/server-sent-events.html)):

```
event: <event_type>
data: <json_payload>

```

- Frames are separated by a blank line (`\n\n`).
- Lines may use CRLF (`\r\n`) or LF (`\n`) line endings. Consumers MUST normalize both to LF.
- Each frame contains exactly one `event:` line and one or more `data:` lines.
- Multi-line `data:` values are joined with `\n` by the consumer.
- The stream terminates with a `data: [DONE]` sentinel (for non-streaming endpoints) or with a `done` event (for streaming endpoints).

## Event Types

### `chunk`

Sent incrementally during streaming generation. Multiple `chunk` events are emitted as tokens are produced.

```json
{
  "token": "Hello",
  "reasoning": "thinking trace..."
}
```

| Field      | Type   | Required | Description                                 |
|------------|--------|----------|---------------------------------------------|
| `token`    | string | yes      | The next text token(s) in the generation.   |
| `reasoning`| string | no       | Reasoning/thinking trace for the token(s).   |
| `agent`    | string | no       | Name of the agent producing this chunk.     |
| `selected_model` | string | no  | Model selected by auto-routing (e.g. Copilot). |

### `telemetry`

Sent periodically during streaming to report token consumption.

```json
{
  "token_economy": {
    "input_tokens": 150,
    "output_tokens": 42,
    "total_tokens": 192
  }
}
```

| Field                      | Type   | Required | Description                              |
|----------------------------|--------|----------|------------------------------------------|
| `token_economy.input_tokens`  | number | yes   | Tokens consumed by the input prompt.     |
| `token_economy.output_tokens` | number | yes   | Tokens generated so far.                 |
| `token_economy.total_tokens`  | number | yes   | Sum of input + output tokens.            |

### `result` / `done`

Sent at the end of a successful generation. Contains the final assembled response.

```json
{
  "response": "Final complete response text.",
  "thinking": "Full reasoning trace.",
  "agent": "agent-name",
  "selected_model": "gpt-4o",
  "conversation_id": "uuid-1234",
  "branch_id": "uuid-5678"
}
```

| Field             | Type   | Required | Description                              |
|-------------------|--------|----------|------------------------------------------|
| `response`        | string | no       | Final complete response text.            |
| `content`         | string | no       | Alias for `response` (legacy).           |
| `thinking`        | string | no       | Full reasoning/thinking trace.           |
| `agent`           | string | no       | Name of the selected agent.              |
| `selected_agent`  | string | no       | Alias for `agent` (legacy).              |
| `selected_model`  | string | no       | Model actually used (after auto-routing).|
| `conversation_id` | string | no       | Conversation tracking ID.                |
| `branch_id`       | string | no       | Branch tracking ID for session forks.    |

### `error`

Sent when a streaming generation encounters an error.

```json
{
  "message": "Provider rate limit exceeded.",
  "error": "rate_limit_exceeded"
}
```

| Field     | Type   | Required | Description                         |
|-----------|--------|----------|-------------------------------------|
| `message` | string | yes      | Human-readable error description.   |
| `error`   | string | no       | Machine-readable error code.        |

## Parsing Contract

All SSE consumers MUST implement the following parsing behavior:

1. **Frame detection**: Split the byte stream on `\n\n` (or `\r\n\r\n`) boundaries. Partial frames at the end of a chunk MUST be buffered and reassembled with the next chunk.

2. **Field extraction**: Within each frame, extract lines starting with `event:` and `data:`. Lines starting with `:` (comments) MUST be ignored. Unknown field prefixes MUST be ignored.

3. **JSON parsing**: The value of the first `data:` line (or concatenated data lines joined by `\n`) MUST be parsed as JSON. Malformed JSON MUST emit a parse error event rather than crashing the consumer.

4. `[DONE]` **sentinel**: A `data: [DONE]` frame signals stream completion. Consumers MUST stop reading and finalize the response. If an event type is present alongside `[DONE]`, the event type is preserved in the parsed output.

5. **Event type injection**: Parsers MUST attach the event type to each parsed JSON object (e.g., as an `_event_type` field) so that downstream routing code can dispatch without re-parsing SSE lines.

6. **Buffer limits**: Consumers MUST enforce a maximum line length (1 MB recommended) and a maximum total buffer size (16 MB recommended) to prevent memory exhaustion.

## Implementations

| Consumer        | File                                         | Parser                             |
|-----------------|----------------------------------------------|------------------------------------|
| GUI (streaming) | `gui/src/views/chat/chat_impl/runtime.rs`    | `StreamProcessor` (backend.rs)     |
| GUI (fallback)  | `gui/src/backend.rs`                         | `StreamProcessor::push_chunk()`    |
| VSCode addon    | `vscode-addon/src/runtime/sseStream.ts`      | `parseSseDataLine()` / `parseSseChunk()` |

All consumers derive from the `StreamProcessor` in `gui/src/backend.rs` (Rust) or the `parseSseChunk` functions in `runtime/sseStream.ts` (TypeScript).
