import { describe, it, expect } from "vitest";
import * as http from "http";
import { AddressInfo } from "net";
import {
  GoOnClient,
  GoOnClientError,
  GoOnHttpError,
} from "../src";

/** Collect all yielded values from an async generator (rejects if it throws). */
async function collect(gen: AsyncGenerator<string>): Promise<string[]> {
  const out: string[] = [];
  for await (const chunk of gen) out.push(chunk);
  return out;
}

interface TestServer {
  srv: http.Server;
  baseUrl: string;
}

/** Start a local HTTP server with the given request handler. */
async function withServer(
  handler: (req: http.IncomingMessage, res: http.ServerResponse) => void,
): Promise<TestServer> {
  const srv = http.createServer(handler);
  await new Promise<void>((resolve) => srv.listen(0, "127.0.0.1", resolve));
  const port = (srv.address() as AddressInfo).port;
  return { srv, baseUrl: `http://127.0.0.1:${port}` };
}

/** Close the server, clearing any pending timers attached to `res`. */
async function closeServer(srv: http.Server): Promise<void> {
  await new Promise<void>((resolve) => srv.close(() => resolve()));
}

/** Run `fn` after `ms`, unless the response has already been closed. */
function schedule(
  res: http.ServerResponse,
  ms: number,
  fn: () => void,
): void {
  const timer = setTimeout(() => {
    if (!res.destroyed) fn();
  }, ms);
  res.on("close", () => clearTimeout(timer));
}

describe("GoOnClient.chatStream (true streaming)", () => {
  it("yields tokens incrementally as they arrive, not after the stream ends", async () => {
    const { srv, baseUrl } = await withServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write('event: chunk\ndata: {"token":"Hel"}\n\n');
      schedule(res, 600, () => {
        res.write('event: chunk\ndata: {"token":"lo"}\n\n');
        schedule(res, 600, () => {
          res.write('event: chunk\ndata: {"token":" world"}\n\n');
          res.end();
        });
      });
    });

    const client = new GoOnClient({ baseUrl });
    const received: Array<{ text: string; at: number }> = [];
    const start = Date.now();
    let totalMs = 0;
    try {
      for await (const chunk of client.chatStream([
        { role: "user", content: "hi" },
      ])) {
        received.push({ text: chunk, at: Date.now() - start });
      }
      totalMs = Date.now() - start;
    } finally {
      await closeServer(srv);
      client.close();
    }

    expect(received.map((r) => r.text)).toEqual(["Hel", "lo", " world"]);
    // The first token must arrive well before the server finished (~1200ms),
    // proving the response body is consumed as a stream, not buffered.
    expect(received[0].at).toBeLessThan(400);
    // And the overall stream took multiple round-trips.
    expect(totalMs).toBeGreaterThanOrEqual(1000);
  });

  it("reassembles frames split across arbitrary network chunk boundaries", async () => {
    const stream =
      'event: chunk\ndata: {"token":"Hel"}\n\n' +
      'event: chunk\ndata: {"token":"lo"}\n\n' +
      "data: [DONE]\n\n";
    const { srv, baseUrl } = await withServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      for (let i = 0; i < stream.length; i += 7) {
        res.write(stream.slice(i, i + 7));
      }
      res.end();
    });

    const client = new GoOnClient({ baseUrl });
    try {
      expect(
        await collect(client.chatStream([{ role: "user", content: "hi" }])),
      ).toEqual(["Hel", "lo"]);
    } finally {
      await closeServer(srv);
      client.close();
    }
  });

  it("terminates on the [DONE] sentinel even if the connection stays open", async () => {
    const { srv, baseUrl } = await withServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write('event: chunk\ndata: {"token":"A"}\n\n');
      res.write("data: [DONE]\n\n");
      // Keep the connection open — the client must not wait for it to close.
      schedule(res, 1500, () => res.end());
    });

    const client = new GoOnClient({ baseUrl });
    try {
      const result = await Promise.race([
        collect(client.chatStream([{ role: "user", content: "x" }])),
        new Promise<never>((_, reject) =>
          setTimeout(
            () => reject(new Error("stream did not terminate on [DONE]")),
            1000,
          ),
        ),
      ]);
      expect(result).toEqual(["A"]);
    } finally {
      await closeServer(srv);
      client.close();
    }
  });

  it("yields token text and falls back to raw JSON for non-token frames", async () => {
    const { srv, baseUrl } = await withServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write('event: chunk\ndata: {"token":"Hello"}\n\n');
      res.write(
        'event: telemetry\ndata: {"token_economy":{"output_tokens":1}}\n\n',
      );
      res.write('event: done\ndata: {"response":"Hello"}\n\n');
      res.end();
    });

    const client = new GoOnClient({ baseUrl });
    try {
      expect(
        await collect(client.chatStream([{ role: "user", content: "hi" }])),
      ).toEqual([
        "Hello",
        '{"token_economy":{"output_tokens":1},"_event_type":"telemetry"}',
        '{"response":"Hello","_event_type":"done"}',
      ]);
    } finally {
      await closeServer(srv);
      client.close();
    }
  });

  it("skips reasoning-only chunks (empty token field) without throwing", async () => {
    const { srv, baseUrl } = await withServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write('event: chunk\ndata: {"token":"","reasoning":"think..."}\n\n');
      res.write('event: chunk\ndata: {"token":"Answer"}\n\n');
      res.end();
    });

    const client = new GoOnClient({ baseUrl });
    try {
      expect(
        await collect(client.chatStream([{ role: "user", content: "hi" }])),
      ).toEqual(["Answer"]);
    } finally {
      await closeServer(srv);
      client.close();
    }
  });

  it("throws GoOnClientError on an SSE error event mid-stream", async () => {
    const { srv, baseUrl } = await withServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write('event: chunk\ndata: {"token":"Hi"}\n\n');
      res.write(
        'event: error\ndata: {"message":"rate limit exceeded","error":"rate_limit"}\n\n',
      );
      res.end();
    });

    const client = new GoOnClient({ baseUrl });
    try {
      await expect(
        collect(client.chatStream([{ role: "user", content: "x" }])),
      ).rejects.toThrow(GoOnClientError);
      await expect(
        collect(client.chatStream([{ role: "user", content: "x" }])),
      ).rejects.toThrow(/rate limit exceeded/);
    } finally {
      await closeServer(srv);
      client.close();
    }
  });

  it("throws GoOnHttpError on a non-200 status", async () => {
    const { srv, baseUrl } = await withServer((req, res) => {
      res.writeHead(500, { "Content-Type": "text/plain" });
      res.end("boom");
    });

    const client = new GoOnClient({ baseUrl });
    try {
      await expect(
        collect(client.chatStream([{ role: "user", content: "x" }])),
      ).rejects.toThrow(GoOnHttpError);
    } finally {
      await closeServer(srv);
      client.close();
    }
  });

  it("throws GoOnClientError on connection failure", async () => {
    const { srv, baseUrl } = await withServer((req, res) => {
      res.end();
    });
    const port = new URL(baseUrl).port;
    await closeServer(srv);

    const client = new GoOnClient({ baseUrl: `http://127.0.0.1:${port}` });
    try {
      await expect(
        collect(client.chatStream([{ role: "user", content: "x" }])),
      ).rejects.toThrow(GoOnClientError);
    } finally {
      client.close();
    }
  });

  it("terminates (does not hang) when the server drops the connection mid-stream", async () => {
    const { srv, baseUrl } = await withServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write('event: chunk\ndata: {"token":"A"}\n\n');
      // Abruptly destroy the socket mid-stream.
      schedule(res, 100, () => res.socket?.destroy());
    });

    const client = new GoOnClient({ baseUrl });
    try {
      const outcome = await Promise.race([
        collect(client.chatStream([{ role: "user", content: "x" }])).then(
          (chunks) => ({ settled: true as const, chunks }),
          (err) => ({ settled: true as const, error: err as Error }),
        ),
        new Promise<{ settled: false }>((resolve) =>
          setTimeout(() => resolve({ settled: false }), 1500),
        ),
      ]);
      // The key regression this guards against: the old buffered
      // implementation never settled when the connection was cut.
      expect(outcome.settled).toBe(true);
    } finally {
      await closeServer(srv);
      client.close();
    }
  });

  it("sends stream:true and chat options in the request body", async () => {
    let receivedBody = "";
    const { srv, baseUrl } = await withServer((req, res) => {
      req.on("data", (chunk: Buffer) => {
        receivedBody += chunk.toString("utf-8");
      });
      req.on("end", () => {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write('event: chunk\ndata: {"token":"ok"}\n\n');
        res.end();
      });
    });

    const client = new GoOnClient({ baseUrl });
    try {
      expect(
        await collect(
          client.chatStream([{ role: "user", content: "hi" }], {
            model: "gpt-4",
            temperature: 0.7,
            maxTokens: 128,
          }),
        ),
      ).toEqual(["ok"]);
      const parsed = JSON.parse(receivedBody);
      expect(parsed.stream).toBe(true);
      expect(parsed.model).toBe("gpt-4");
      expect(parsed.temperature).toBe(0.7);
      expect(parsed.max_tokens).toBe(128);
      expect(parsed.messages).toEqual([{ role: "user", content: "hi" }]);
    } finally {
      await closeServer(srv);
      client.close();
    }
  });
});
