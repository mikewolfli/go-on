/**
 * Tests for go-on TypeScript SDK client.
 *
 * BLUE56-E06: TypeScript SDK test suite using vitest.
 * All API calls are mocked — no live server required.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { GoOnClient } from "../src/client";
import type {
  ToolCall,
  StreamChunk,
  MultimodalInput,
  AgentInfo,
} from "../src/types";

// ---------------------------------------------------------------------------
// Mock setup
// ---------------------------------------------------------------------------

const MOCK_BASE_URL = "http://127.0.0.1:8090";

function createMockFetch(ok: boolean, responseBody: unknown): typeof fetch {
  return vi.fn().mockResolvedValue({
    ok,
    status: ok ? 200 : 500,
    json: () => Promise.resolve(responseBody),
    text: () => Promise.resolve(JSON.stringify(responseBody)),
  } as Response);
}

// ---------------------------------------------------------------------------
// GoOnClient tests
// ---------------------------------------------------------------------------

describe("GoOnClient", () => {
  let client: GoOnClient;

  beforeEach(() => {
    client = new GoOnClient({ baseUrl: MOCK_BASE_URL });
    vi.clearAllMocks();
  });

  // ── Constructor ───────────────────────────────────────────────────────

  it("should construct with a base URL", () => {
    expect(client).toBeInstanceOf(GoOnClient);
  });

  // ── JSON-RPC methods ─────────────────────────────────────────────────

  it("should call runtime.health successfully", async () => {
    const mockResponse = {
      ok: true,
      status: "healthy",
      version: "1.1.0",
      uptime_seconds: 3600,
    };
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: mockResponse,
    });

    const result = await client.runtimeHealth();
    expect(result).toEqual(mockResponse);
  });

  it("should handle governance.status", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 2,
      result: { ok: true, governance: { enabled: true } },
    });

    const result = await client.governanceStatus();
    expect(result.ok).toBe(true);
  });

  it("should handle governance.audit.verify", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 3,
      result: {
        ok: true,
        entry_count: 3,
        is_chain_intact: true,
        violations: [],
      },
    });

    const result = await client.governanceAuditVerify({});
    expect(result.ok).toBe(true);
    expect(result.is_chain_intact).toBe(true);
  });

  it("should handle JSON-RPC error responses", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 3,
      error: { code: -32000, message: "Method not found" },
    });

    await expect(client.runtimeHealth()).rejects.toThrow();
  });

  it("should handle HTTP errors", async () => {
    // Disable retries so the error path is exercised directly (the default
    // unified backoff would otherwise sleep ~5s across 3 retries).
    const noRetryClient = new GoOnClient({
      baseUrl: MOCK_BASE_URL,
      maxRetries: 0,
    });
    globalThis.fetch = createMockFetch(false, { error: "Internal error" });

    await expect(noRetryClient.governanceStatus()).rejects.toThrow();
  });

  // ── Chat streaming ────────────────────────────────────────────────────

  it("should handle chat stream with SSE chunks", async () => {
    const encoder = new TextEncoder();
    const stream = new ReadableStream({
      start(controller) {
        controller.enqueue(encoder.encode('data: {"token":"Hello"}\n\n'));
        controller.enqueue(encoder.encode('data: {"token":" World"}\n\n'));
        controller.enqueue(encoder.encode("data: [DONE]\n\n"));
        controller.close();
      },
    });

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      body: stream,
      headers: new Headers(),
    } as Response);

    const chunks: Record<string, unknown>[] = [];
    for await (const chunk of client.chatStream({
      messages: [{ role: "user", content: "Hi" }],
    })) {
      chunks.push(chunk);
    }
    expect(chunks.length).toBeGreaterThanOrEqual(2);
  });

  it("should abort chat stream on signal without hanging", async () => {
    const controller = new AbortController();
    const encoder = new TextEncoder();
    const stream = new ReadableStream({
      start(c) {
        c.enqueue(encoder.encode('data: {"token":"Hello"}\n\n'));
        // Simulate the server stream being cut off when the signal fires.
        controller.signal.addEventListener("abort", () => c.close());
      },
    });

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      body: stream,
      headers: new Headers(),
    } as Response);

    const generator = client.chatStream(
      { messages: [{ role: "user", content: "Hi" }] },
      controller.signal,
    );
    const chunks: Record<string, unknown>[] = [];
    const consume = (async () => {
      for await (const chunk of generator) {
        chunks.push(chunk);
      }
    })();

    // Let the first frame land, then abort — the stream must terminate
    // instead of hanging on a never-closing connection.
    await new Promise((resolve) => setTimeout(resolve, 10));
    controller.abort();
    await consume;
    expect(chunks.length).toBeGreaterThanOrEqual(1);
  });
});

// ---------------------------------------------------------------------------
// Type definition tests
// ---------------------------------------------------------------------------

describe("SDK Types", () => {
  it("should construct a ToolCall", () => {
    const tc: ToolCall = {
      tool_name: "read_file",
      arguments: { path: "/test.txt" },
      agent_name: "coder",
      duration_ms: 150,
    };
    expect(tc.tool_name).toBe("read_file");
    expect(tc.agent_name).toBe("coder");
  });

  it("should construct a StreamChunk", () => {
    const chunk: StreamChunk = {
      token: "Hello",
      done: false,
      index: 0,
      total_chars: 5,
    };
    expect(chunk.token).toBe("Hello");
    expect(chunk.done).toBe(false);
  });

  it("should construct MultimodalInput variants", () => {
    const text: MultimodalInput = { type: "text", text: "Hello" };
    expect(text.type).toBe("text");

    const image: MultimodalInput = {
      type: "image",
      image_url: "data:image/png;base64,...",
      detail: "auto",
    };
    expect(image.type).toBe("image");

    const doc: MultimodalInput = {
      type: "document",
      data: "base64data",
      mime_type: "application/pdf",
      filename: "doc.pdf",
    };
    expect(doc.type).toBe("document");

    const audio: MultimodalInput = {
      type: "audio",
      data: "base64data",
      format: "wav",
    };
    expect(audio.type).toBe("audio");
  });

  it("should construct an AgentInfo", () => {
    const info: AgentInfo = {
      name: "coder-agent",
      agent_type: "copilot",
      description: "Coding assistant",
      models: ["gpt-4o"],
      capabilities: ["coding", "general"],
      healthy: true,
    };
    expect(info.name).toBe("coder-agent");
    expect(info.healthy).toBe(true);
  });
});
