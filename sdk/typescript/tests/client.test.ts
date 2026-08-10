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

  it("metricsPrometheus fetches GET /metrics as plain text", async () => {
    const prometheusText = "# HELP acp_test_total 1\nacp_test_total 1\n";
    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () => Promise.resolve(prometheusText),
      json: () => Promise.reject(new Error("not json")),
    } as unknown as Response);

    const result = await client.metricsPrometheus();
    expect(result).toBe(prometheusText);
    const [url] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock
      .calls[0];
    expect(url).toBe(`${MOCK_BASE_URL}/metrics`);
  });

  it("initialize omits setup_level when not provided", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: {},
    });

    await client.initialize();
    const body = JSON.parse(
      (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0][1]!
        .body as string,
    );
    expect(body.method).toBe("initialize");
    expect(body.params).toEqual({});
  });

  it("initialize sends setup_level when provided", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: {},
    });

    await client.initialize("full");
    const body = JSON.parse(
      (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0][1]!
        .body as string,
    );
    expect(body.method).toBe("initialize");
    expect(body.params).toEqual({ setup_level: "full" });
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

  // ── Session methods (contract coverage) ───────────────────────────────

  it("should call session/load with sessionId", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 4,
      result: {
        modes: { currentModeId: "ask", availableModes: [] },
        configOptions: [
          {
            id: "model",
            name: "Model",
            kind: {
              type: "select",
              currentValue: "gpt-4o",
              options: { type: "grouped", groups: [] },
            },
          },
        ],
      },
    });

    const result = await client.sessionLoad("sess-1");
    expect(result.configOptions?.[0]?.id).toBe("model");
    expect(result.modes?.currentModeId).toBe("ask");
    const body = JSON.parse((globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0][1]!.body as string);
    expect(body.method).toBe("session/load");
    expect(body.params).toEqual({ sessionId: "sess-1" });
  });

  it("should call session/config/get with sessionId", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 5,
      result: { configOptions: { model: "gpt-4o" }, sessionId: "sess-1" },
    });

    const result = await client.sessionConfigGet("sess-1");
    expect(result.configOptions["model"]).toBe("gpt-4o");
    const body = JSON.parse((globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0][1]!.body as string);
    expect(body.method).toBe("session/config/get");
    expect(body.params).toEqual({ sessionId: "sess-1" });
  });

  it("should call session/request_permission with sessionId and optionId", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 6,
      result: {},
    });

    await client.sessionRequestPermission("sess-1", "approve");
    const body = JSON.parse((globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0][1]!.body as string);
    expect(body.method).toBe("session/request_permission");
    expect(body.params).toEqual({ sessionId: "sess-1", optionId: "approve" });
  });

  it("should call session/delete with sessionId", async () => {
    globalThis.fetch = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 7,
      result: { deleted: true, sessionId: "sess-1" },
    });

    const result = await client.sessionDelete("sess-1");
    expect(result.deleted).toBe(true);
    const body = JSON.parse((globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls[0][1]!.body as string);
    expect(body.method).toBe("session/delete");
    expect(body.params).toEqual({ sessionId: "sess-1" });
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

  it("chatStream nests model/temperature/max_tokens inside options", async () => {
    const encoder = new TextEncoder();
    const stream = new ReadableStream({
      start(controller) {
        controller.enqueue(encoder.encode('data: {"token":"Hi"}\n\n'));
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
      model: "gpt-4",
      temperature: 0.7,
      max_tokens: 128,
    })) {
      chunks.push(chunk);
    }
    expect(chunks).toEqual([{ token: "Hi" }]);

    const [, init] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock
      .calls[0];
    const body = JSON.parse(init!.body as string);
    expect(body.options).toEqual({
      model: "gpt-4",
      temperature: 0.7,
      max_tokens: 128,
    });
    expect(body.model).toBeUndefined();
    expect(body.temperature).toBeUndefined();
    expect(body.max_tokens).toBeUndefined();
  });

  it("chatStream forwards stream only when explicitly provided", async () => {
    const encoder = new TextEncoder();
    const stream = new ReadableStream({
      start(controller) {
        controller.enqueue(encoder.encode('data: {"token":"Hi"}\n\n'));
        controller.enqueue(encoder.encode("data: [DONE]\n\n"));
        controller.close();
      },
    });

    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      body: stream,
      headers: new Headers(),
    } as Response);

    // stream: false must be forwarded verbatim (not overwritten to true)
    const generator = client.chatStream({
      messages: [{ role: "user", content: "Hi" }],
      stream: false,
    });
    // consume one chunk so the request is sent
    await generator.next();
    const [, init] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock
      .calls[0];
    const body = JSON.parse(init!.body as string);
    expect(body.stream).toBe(false);

    // stream: undefined → omitted from the payload (matches Rust/Python)
    const generator2 = client.chatStream({
      messages: [{ role: "user", content: "Hi" }],
    });
    await generator2.next();
    const [, init2] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock
      .calls[1];
    const body2 = JSON.parse(init2!.body as string);
    expect(body2.stream).toBeUndefined();
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

  it("tool.approve sends tool_name", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { ok: true },
    });
    globalThis.fetch = mock;

    await client.toolApprove("apply_patch");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("tool.approve");
    expect(body.params).toEqual({ tool_name: "apply_patch" });
  });

  it("skill.create sends name, description, prompt_template and input_schema", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { ok: true, action: "create", name: "my-skill" },
    });
    globalThis.fetch = mock;

    await client.skillCreate({
      name: "my-skill",
      description: "A test skill",
      prompt_template: "Do {{task}}",
      input_schema: { task: "string" },
    });
    const body = lastRpcBody(mock);
    expect(body.method).toBe("skill.create");
    expect(body.params).toEqual({
      name: "my-skill",
      description: "A test skill",
      prompt_template: "Do {{task}}",
      input_schema: { task: "string" },
    });
  });

  it("skill.version.rollback sends name and version", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { ok: true, action: "version.rollback", name: "s1", version: "1.0.1" },
    });
    globalThis.fetch = mock;

    await client.skillVersionRollback("s1", "1.0.1");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("skill.version.rollback");
    expect(body.params).toEqual({ name: "s1", version: "1.0.1" });
  });

  it("prompts.create sends lang plus template fields", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { ok: true },
    });
    globalThis.fetch = mock;

    await client.promptsCreate({ id: "t1", content: "Hello" }, "zh-CN");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("prompts.create");
    expect(body.params).toEqual({ lang: "zh-CN", id: "t1", content: "Hello" });
  });

  it("prompts.search sends query and lang", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { results: [] },
    });
    globalThis.fetch = mock;

    await client.promptsSearch("test", "en");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("prompts.search");
    expect(body.params).toEqual({ query: "test", lang: "en" });
  });

  it("session/resume sends sessionId and optional cwd", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { sessionId: "sess-1", modes: {} },
    });
    globalThis.fetch = mock;

    await client.sessionResume({ sessionId: "sess-1", cwd: "/tmp" });
    const body = lastRpcBody(mock);
    expect(body.method).toBe("session/resume");
    expect(body.params).toEqual({ sessionId: "sess-1", cwd: "/tmp" });
  });

  it("session/config/set sends configId and value", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { configOptions: [{ id: "model", value: "gpt-4o" }] },
    });
    globalThis.fetch = mock;

    await client.sessionConfigSet("sess-1", "model", "gpt-4o");
    const body = lastRpcBody(mock);
    expect(body.method).toBe("session/config/set");
    expect(body.params).toEqual({
      sessionId: "sess-1",
      configId: "model",
      value: "gpt-4o",
    });
  });

  it("session/close sends sessionId", async () => {
    const mock = createMockFetch(true, {
      jsonrpc: "2.0",
      id: 1,
      result: { ok: true },
    });
    globalThis.fetch = mock;

    await client.sessionClose({ sessionId: "sess-1" });
    const body = lastRpcBody(mock);
    expect(body.method).toBe("session/close");
    expect(body.params).toEqual({ sessionId: "sess-1" });
  });
});

// ---------------------------------------------------------------------------
// OpenAI-compatible endpoint contract tests
//
// These endpoints return plain OpenAI wire-format JSON (no JSON-RPC envelope),
// so the tests assert the URL path and the raw request body instead.
// ---------------------------------------------------------------------------

describe("OpenAI-compat contracts", () => {
  let client: GoOnClient;

  beforeEach(() => {
    client = new GoOnClient({ baseUrl: MOCK_BASE_URL });
    vi.clearAllMocks();
  });

  function lastRequest(): [string, RequestInit] {
    const calls = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls;
    return calls[0] as [string, RequestInit];
  }

  it("chatCompletions POSTs the OpenAI body to /v1/chat/completions", async () => {
    const mock = createMockFetch(true, {
      id: "chatcmpl-1",
      object: "chat.completion",
      choices: [{ index: 0, message: { role: "assistant", content: "hi" } }],
    });
    globalThis.fetch = mock;

    const result = await client.chatCompletions({
      model: "go-on",
      messages: [{ role: "user", content: "hi" }],
    });
    expect(result.object).toBe("chat.completion");
    const [url, init] = lastRequest();
    expect(url).toBe(`${MOCK_BASE_URL}/v1/chat/completions`);
    expect(init.method).toBe("POST");
    expect(JSON.parse(init.body as string)).toEqual({
      model: "go-on",
      messages: [{ role: "user", content: "hi" }],
    });
  });

  it("responsesCreate POSTs the Responses API body to /v1/responses", async () => {
    const mock = createMockFetch(true, {
      id: "resp_1",
      object: "response",
      output: [],
      status: "completed",
    });
    globalThis.fetch = mock;

    const result = await client.responsesCreate({ model: "go-on", input: "hi" });
    expect(result.status).toBe("completed");
    const [url, init] = lastRequest();
    expect(url).toBe(`${MOCK_BASE_URL}/v1/responses`);
    expect(init.method).toBe("POST");
    expect(JSON.parse(init.body as string)).toEqual({ model: "go-on", input: "hi" });
  });

  it("responsesGet GETs /v1/responses/{id}", async () => {
    const mock = createMockFetch(true, {
      id: "resp_1",
      object: "response",
      status: "completed",
    });
    globalThis.fetch = mock;

    const result = await client.responsesGet("resp_1");
    expect(result.id).toBe("resp_1");
    const [url, init] = lastRequest();
    expect(url).toBe(`${MOCK_BASE_URL}/v1/responses/resp_1`);
    expect(init.method).toBe("GET");
  });

  it("modelsList GETs /v1/models and returns the list", async () => {
    const mock = createMockFetch(true, {
      object: "list",
      data: [{ id: "go-on", object: "model" }],
    });
    globalThis.fetch = mock;

    const result = await client.modelsList();
    expect(result.data).toHaveLength(1);
    const [url, init] = lastRequest();
    expect(url).toBe(`${MOCK_BASE_URL}/v1/models`);
    expect(init.method).toBe("GET");
  });
});

// ---------------------------------------------------------------------------
// Type definition tests
// ---------------------------------------------------------------------------

describe("SDK Types", () => {
  // Each test constructs a typed value (compile-time contract check) and then
  // asserts its JSON wire shape — the field names the backend actually sees.

  it("should construct a ToolCall with a stable wire shape", () => {
    const tc: ToolCall = {
      tool_name: "read_file",
      arguments: { path: "/test.txt" },
      agent_name: "coder",
      duration_ms: 150,
    };
    const wire = JSON.parse(JSON.stringify(tc)) as Record<string, unknown>;
    expect(wire.tool_name).toBe("read_file");
    expect(wire.agent_name).toBe("coder");
    expect(wire.duration_ms).toBe(150);
    expect((wire.arguments as Record<string, unknown>).path).toBe("/test.txt");
  });

  it("should construct a StreamChunk with a stable wire shape", () => {
    const chunk: StreamChunk = {
      token: "Hello",
      done: false,
      index: 0,
      total_chars: 5,
    };
    const wire = JSON.parse(JSON.stringify(chunk)) as Record<string, unknown>;
    expect(wire.token).toBe("Hello");
    expect(wire.done).toBe(false);
    expect(wire.index).toBe(0);
    expect(wire.total_chars).toBe(5);
  });

  it("should construct MultimodalInput variants with a stable wire shape", () => {
    const text: MultimodalInput = { type: "text", text: "Hello" };
    expect(JSON.parse(JSON.stringify(text))).toEqual({
      type: "text",
      text: "Hello",
    });

    const image: MultimodalInput = {
      type: "image",
      image_url: "data:image/png;base64,...",
      detail: "auto",
    };
    expect(JSON.parse(JSON.stringify(image))).toEqual({
      type: "image",
      image_url: "data:image/png;base64,...",
      detail: "auto",
    });

    const doc: MultimodalInput = {
      type: "document",
      data: "base64data",
      mime_type: "application/pdf",
      filename: "doc.pdf",
    };
    expect(JSON.parse(JSON.stringify(doc))).toEqual({
      type: "document",
      data: "base64data",
      mime_type: "application/pdf",
      filename: "doc.pdf",
    });

    const audio: MultimodalInput = {
      type: "audio",
      data: "base64data",
      format: "wav",
    };
    expect(JSON.parse(JSON.stringify(audio))).toEqual({
      type: "audio",
      data: "base64data",
      format: "wav",
    });
  });

  it("should construct an AgentInfo with a stable wire shape", () => {
    const info: AgentInfo = {
      name: "coder-agent",
      agent_type: "copilot",
      description: "Coding assistant",
      models: ["gpt-4o"],
      capabilities: ["coding", "general"],
      healthy: true,
    };
    const wire = JSON.parse(JSON.stringify(info)) as Record<string, unknown>;
    expect(wire.name).toBe("coder-agent");
    expect(wire.agent_type).toBe("copilot");
    expect(wire.healthy).toBe(true);
    expect(wire.models).toEqual(["gpt-4o"]);
    expect(wire.capabilities).toEqual(["coding", "general"]);
  });
});
