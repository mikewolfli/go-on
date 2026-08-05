import { describe, it, expect } from "vitest";
import {
  createSseParser,
  SSE_MAX_LINE_LENGTH,
  SSE_MAX_BUFFER_SIZE,
} from "../src/sse";

// Captured sample stream matching the wire format in contracts/sse-protocol.md
// and the shared test fixture used by the other SDK consumers:
// chunk tokens, telemetry, done, terminated by `data: [DONE]`.
const capturedSampleStream = [
  'event: chunk\ndata: {"token":"The"}\n\n',
  'event: chunk\ndata: {"token":" quick"}\n\n',
  'event: chunk\ndata: {"token":" brown"}\n\n',
  'event: chunk\ndata: {"token":" fox"}\n\n',
  'event: telemetry\ndata: {"token_economy":{"input_tokens":1,"output_tokens":4,"total_tokens":5}}\n\n',
  'event: done\ndata: {"response":"The quick brown fox","agent":"default","selected_model":"gpt-4o"}\n\n',
  "data: [DONE]\n\n",
].join("");

describe("createSseParser", () => {
  it("parses a complete captured stream into typed frames", () => {
    const parser = createSseParser();
    const frames = parser.push(capturedSampleStream);

    expect(frames.map((f) => f.eventType)).toEqual([
      "chunk",
      "chunk",
      "chunk",
      "chunk",
      "telemetry",
      "done",
    ]);
    expect(frames[0].data.token).toBe("The");
    expect(frames[3].data.token).toBe(" fox");
    expect(frames[4].data.token_economy.total_tokens).toBe(5);
    expect(frames[5].data.response).toBe("The quick brown fox");
    expect(frames[5].data.selected_model).toBe("gpt-4o");
    // [DONE] sentinel is consumed and flips the done flag without a frame.
    expect(parser.done).toBe(true);
  });

  it("injects the event type as _event_type", () => {
    const parser = createSseParser();
    const [frame] = parser.push('event: chunk\ndata: {"token":"hello"}\n\n');
    expect(frame.eventType).toBe("chunk");
    expect(frame.data._event_type).toBe("chunk");
  });

  it("reassembles frames split across arbitrary chunk boundaries", () => {
    const parser = createSseParser();
    const stream = [
      'event: chunk\ndata: {"token":"The"}\n\n',
      'event: chunk\ndata: {"token":" fox"}\n\n',
      "data: [DONE]\n\n",
    ].join("");
    const frames: ReturnType<typeof parser.push> = [];
    for (let i = 0; i < stream.length; i += 7) {
      frames.push(...parser.push(stream.slice(i, i + 7)));
    }
    expect(frames.map((f) => f.data.token)).toEqual(["The", " fox"]);
    expect(parser.done).toBe(true);
  });

  it("buffers a partial frame until its blank-line terminator arrives", () => {
    const parser = createSseParser();
    // First push ends in the middle of a line, second completes the frame.
    expect(parser.push('event: chunk\ndata: {"tok')).toEqual([]);
    const frames = parser.push('en":"x"}\n\n');
    expect(frames).toHaveLength(1);
    expect(frames[0].data.token).toBe("x");
  });

  it("normalizes CRLF line endings", () => {
    const parser = createSseParser();
    const frames = parser.push(
      'event: chunk\r\ndata: {"token":"hi"}\r\n\r\nevent: chunk\r\ndata: {"token":" there"}\r\n\r\n',
    );
    expect(frames.map((f) => f.data.token)).toEqual(["hi", " there"]);
  });

  it("ignores comment lines and unknown field prefixes", () => {
    const parser = createSseParser();
    const frames = parser.push(
      ': keep-alive comment\nretry: 100\nevent: chunk\ndata: {"token":"ok"}\n\n',
    );
    expect(frames).toHaveLength(1);
    expect(frames[0].data.token).toBe("ok");
  });

  it("joins multi-line data values with a newline", () => {
    const parser = createSseParser();
    // Multiple `data:` lines are joined with "\n" before JSON parsing. The
    // split points land between JSON tokens so the joined payload stays valid.
    const frames = parser.push(
      'event: chunk\ndata: {"token":\ndata: "Hello"}\n\n',
    );
    expect(frames).toHaveLength(1);
    expect(frames[0].data.token).toBe("Hello");
  });

  it("handles event and data lines without a space after the colon", () => {
    const parser = createSseParser();
    const frames = parser.push('event:chunk\ndata:{"token":"hi"}\n\n');
    expect(frames).toHaveLength(1);
    expect(frames[0].eventType).toBe("chunk");
    expect(frames[0].data.token).toBe("hi");
  });

  it("skips malformed JSON frames instead of throwing", () => {
    const parser = createSseParser();
    const frames = parser.push(
      'event: chunk\ndata: {not json}\n\nevent: chunk\ndata: {"token":"after"}\n\n',
    );
    expect(frames).toHaveLength(1);
    expect(frames[0].data.token).toBe("after");
    expect(parser.done).toBe(false);
  });

  it("skips frames with no data lines", () => {
    const parser = createSseParser();
    expect(parser.push("event: done\n\n")).toEqual([]);
  });

  it("stops parsing after the [DONE] sentinel", () => {
    const parser = createSseParser();
    const frames = parser.push(
      'event: chunk\ndata: {"token":"A"}\n\ndata: [DONE]\n\nevent: chunk\ndata: {"token":"ignored"}\n\n',
    );
    expect(frames.map((f) => f.data.token)).toEqual(["A"]);
    expect(parser.done).toBe(true);
    // push after [DONE] yields nothing
    expect(parser.push('event: chunk\ndata: {"token":"more"}\n\n')).toEqual([]);
  });

  it("flush() parses a trailing frame without a blank-line terminator", () => {
    const parser = createSseParser();
    parser.push('event: chunk\ndata: {"token":"tail"}');
    const tail = parser.flush();
    expect(tail).toHaveLength(1);
    expect(tail[0].data.token).toBe("tail");
  });

  it("flush() after [DONE] returns nothing", () => {
    const parser = createSseParser();
    parser.push("data: [DONE]\n\n");
    expect(parser.done).toBe(true);
    expect(parser.flush()).toEqual([]);
  });

  it("enforces the maximum line length", () => {
    const parser = createSseParser({ maxLineLength: 16 });
    const longLine = "x".repeat(17);
    expect(() => parser.push(`data: ${longLine}\n\n`)).toThrow(
      /SSE line exceeds 16 bytes/,
    );
  });

  it("enforces the maximum buffer size", () => {
    const parser = createSseParser({ maxBufferSize: 32 });
    expect(() => parser.push("y".repeat(64))).toThrow(
      /SSE buffer exceeds 32 bytes/,
    );
  });

  it("exposes the documented default limits", () => {
    expect(SSE_MAX_LINE_LENGTH).toBe(1024 * 1024);
    expect(SSE_MAX_BUFFER_SIZE).toBe(16 * 1024 * 1024);
  });
});
