import { describe, it, expect } from "vitest";
import {
  GoOnClient,
  GoOnClientError,
  GoOnJsonRpcError,
  GoOnHttpError,
} from "../src";

// ── Client initialization ────────────────────────────────────────────────

describe("GoOnClient", () => {
  it("can be created with a base URL", () => {
    const client = new GoOnClient({ baseUrl: "http://localhost:8090" });
    expect(client.baseUrl).toBe("http://localhost:8090");
    expect(client.timeout).toBe(30.0);
    expect(client.maxRetries).toBe(3);
    expect(client.retryDelayMs).toBe(1000);
    client.close();
  });

  it("respects custom timeout", () => {
    const client = new GoOnClient({
      baseUrl: "http://localhost:8090",
      timeout: 10.0,
    });
    expect(client.timeout).toBe(10.0);
    client.close();
  });

  it("respects custom max retries", () => {
    const client = new GoOnClient({
      baseUrl: "http://localhost:8090",
      maxRetries: 5,
    });
    expect(client.maxRetries).toBe(5);
    client.close();
  });

  it("respects custom retry delay", () => {
    const client = new GoOnClient({
      baseUrl: "http://localhost:8090",
      retryDelayMs: 2000,
    });
    expect(client.retryDelayMs).toBe(2000);
    client.close();
  });

  it("strips trailing slash from base URL", () => {
    const client = new GoOnClient({ baseUrl: "http://localhost:8090/" });
    expect(client.baseUrl).toBe("http://localhost:8090");
    client.close();
  });

  it("computes retry delay with the unified backoff contract", () => {
    const client = new GoOnClient({ baseUrl: "http://localhost:8090" });
    // Access private method via bracket notation for testing
    const delay0 = (client as any)._retryDelayForAttempt(0);
    const delay1 = (client as any)._retryDelayForAttempt(1);
    const delay2 = (client as any)._retryDelayForAttempt(2);
    const delay5 = (client as any)._retryDelayForAttempt(5);
    const delay10 = (client as any)._retryDelayForAttempt(10);

    // Unified contract (contracts/cross-client-sync.md):
    // delay = min(base * 2^attempt, 30s) * (0.7 + random * 0.3)
    // attempt 0: [700, 1000)  1: [1400, 2000)  2: [2800, 4000)
    // attempt 5+: capped at 30s → [21000, 30000)
    expect(delay0).toBeGreaterThanOrEqual(700);
    expect(delay0).toBeLessThanOrEqual(1000);
    expect(delay1).toBeGreaterThanOrEqual(1400);
    expect(delay1).toBeLessThanOrEqual(2000);
    expect(delay2).toBeGreaterThanOrEqual(2800);
    expect(delay2).toBeLessThanOrEqual(4000);
    expect(delay5).toBeGreaterThanOrEqual(21000);
    expect(delay5).toBeLessThanOrEqual(30000);
    expect(delay10).toBeGreaterThanOrEqual(21000);
    expect(delay10).toBeLessThanOrEqual(30000);

    client.close();
  });
});

// ── Error types ──────────────────────────────────────────────────────────

describe("GoOnClientError", () => {
  it("can be created with a message", () => {
    const error = new GoOnClientError("Rate limited");
    expect(error.message).toContain("Rate limited");
    expect(error.name).toBe("GoOnClientError");
  });
});

describe("GoOnJsonRpcError", () => {
  it("stores code and message", () => {
    const error = new GoOnJsonRpcError(429, "Rate limited");
    expect(error.code).toBe(429);
    expect(error.messageText).toBe("Rate limited");
    expect(error.message).toContain("429");
    expect(error.name).toBe("GoOnJsonRpcError");
  });
});

describe("GoOnHttpError", () => {
  it("stores status code and status text", () => {
    const error = new GoOnHttpError(503, "Service Unavailable");
    expect(error.statusCode).toBe(503);
    expect(error.statusText).toBe("Service Unavailable");
    expect(error.message).toContain("503");
    expect(error.name).toBe("GoOnHttpError");
  });
});

// ── Type construction ────────────────────────────────────────────────────

describe("Chat types", () => {
  it("creates a chat message", () => {
    const msg = { role: "user" as const, content: "Hello" };
    expect(msg.role).toBe("user");
    expect(msg.content).toBe("Hello");
  });

  it("creates a chat request with multiple messages", () => {
    const messages = [
      { role: "system" as const, content: "You are helpful" },
      { role: "user" as const, content: "What is ACP?" },
    ];
    expect(messages.length).toBe(2);
  });

  it("supports optional chat request fields", () => {
    const request = {
      messages: [{ role: "user" as const, content: "Hello" }],
      model: "gpt-4",
      temperature: 0.7,
      max_tokens: 2048,
      stream: true,
    };
    expect(request.model).toBe("gpt-4");
    expect(request.temperature).toBe(0.7);
    expect(request.max_tokens).toBe(2048);
    expect(request.stream).toBe(true);
  });
});
