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
// Contract parameter alignment tests
//
// These tests assert the exact JSON-RPC params each method sends, so the SDK
// stays aligned with the backend ACP contract (see src/acp/impl/request/).
// ---------------------------------------------------------------------------

describe("ACP contract params", () => {
  let client: GoOnClient;

  beforeEach(() => {
    client = new GoOnClient({ baseUrl: MOCK_BASE_URL });
    vi.clearAllMocks();
  });

  function lastRpcBody(mock: typeof fetch): {
    method: string;
    params: Record<string, unknown>;
  } {
    const calls = (mock as ReturnType<typeof vi.fn>).mock.calls;
    const [, init] = calls[0] as [string, RequestInit];
    return JSON.parse(init.body as string);
  }

  it("checkpoint.list sends required conversation_id", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { ok: true, conversation_id: "conv-1", count: 0, checkpoints: [] },
    });
    globalThis.fetch = mock;

    await client.checkpointList("conv-1");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("checkpoint.list");
    expect(body.params).toEqual({ conversation_id: "conv-1" });
  });

  it("conversation.rollback sends conversation_id and checkpoint_id", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { ok: true },
    });
    globalThis.fetch = mock;

    await client.conversationRollback("conv-1", "cp-42");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("conversation.rollback");
    expect(body.params).toEqual({
      conversation_id: "conv-1",
      checkpoint_id: "cp-42",
    });
  });

  it("session/new sends work_dirs (snake_case) and additionalDirectories", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { sessionId: "sess-1" },
    });
    globalThis.fetch = mock;

    await client.sessionNew({
      cwd: "/tmp",
      work_dirs: ["/tmp/a"],
      additionalDirectories: ["/tmp/b"],
      mode: "safeguard",
    });
    const body = lastRpcBody(mock);
    expect(body.method).toBe("session/new");
    expect(body.params).toEqual({
      cwd: "/tmp",
      work_dirs: ["/tmp/a"],
      additionalDirectories: ["/tmp/b"],
      mode: "safeguard",
    });
  });

  it("session/set_config_option uses configId (not optionId)", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { configOptions: [] },
    });
    globalThis.fetch = mock;

    await client.sessionSetConfigOption("sess-1", "model", "gpt-4o");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("session/set_config_option");
    expect(body.params).toEqual({
      sessionId: "sess-1",
      configId: "model",
      value: "gpt-4o",
    });
  });

  it("session/set_mode sends sessionId and modeId", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: {},
    });
    globalThis.fetch = mock;

    await client.sessionSetMode("sess-1", "edit");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("session/set_mode");
    expect(body.params).toEqual({ sessionId: "sess-1", modeId: "edit" });
  });

  it("session/list parses the minimal { id } session shape", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { sessions: [{ id: "sess-1" }] },
    });

    const result = await client.sessionList();
    expect(result.sessions).toEqual([{ id: "sess-1" }]);
  });

  it("tools/list sends no params and parses the tools array", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: {
        tools: [
          {
            name: "read_file",
            description: "Read a file",
            input_schema: { type: "object" },
          },
        ],
      },
    });
    globalThis.fetch = mock;

    const tools = await client.toolsList();
    expect(tools).toHaveLength(1);
    expect(tools[0].name).toBe("read_file");
    expect(tools[0].input_schema).toEqual({ type: "object" });
    const body = lastRpcBody(mock);
    expect(body.method).toBe("tools/list");
  });

  it("tools/call sends name, arguments and optional sessionId", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { content: [{ type: "text", text: "ok" }] },
    });
    globalThis.fetch = mock;

    await client.toolsCall({
      name: "read_file",
      arguments: { path: "/tmp/a.txt" },
      sessionId: "sess-1",
    });
    const body = lastRpcBody(mock);
    expect(body.method).toBe("tools/call");
    expect(body.params).toEqual({
      name: "read_file",
      arguments: { path: "/tmp/a.txt" },
      sessionId: "sess-1",
    });
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
