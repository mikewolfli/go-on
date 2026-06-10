import * as assert from "node:assert";
import { FramedReader, FramedWriter } from "../../runtime/framedProtocol";
import { FramedMessage } from "../../protocolContract";

/**
 * Create a simple ReadableStream-like adapter from a buffer.
 * Used to test FramedReader without a real stream.
 */
function makeStream(data: Uint8Array): {
  stream: {
    getReader: () => {
      read: () => Promise<{ done: boolean; value: Uint8Array }>;
      cancel: () => Promise<void>;
    };
  };
  feed: (_chunk: Uint8Array) => void;
  close: () => void;
} {
  let chunks: Uint8Array[] = [data];
  let closed = false;

  return {
    stream: {
      getReader: () => ({
        read: async () => {
          if (chunks.length > 0) {
            return { done: false, value: chunks.shift()! };
          }
          if (closed) {
            return { done: true, value: new Uint8Array(0) };
          }
          // Wait and check again
          await new Promise((r) => setTimeout(r, 10));
          if (chunks.length > 0) {
            return { done: false, value: chunks.shift()! };
          }
          return { done: true, value: new Uint8Array(0) };
        },
        cancel: async () => {
          chunks = [];
          closed = true;
        },
      }),
    },
    feed: (chunk: Uint8Array) => {
      chunks.push(chunk);
    },
    close: () => {
      closed = true;
    },
  };
}

function encodeFrame(payload: Record<string, unknown>): Uint8Array {
  const json = JSON.stringify(payload);
  const jsonBytes = new TextEncoder().encode(json);
  const frame = new Uint8Array(4 + jsonBytes.length);
  new DataView(frame.buffer).setUint32(0, jsonBytes.length, false);
  frame.set(jsonBytes, 4);
  return frame;
}

suite("framedProtocol", () => {
  suite("FramedWriter", () => {
    test("writes a message and returns true", () => {
      const written: Uint8Array[] = [];
      const writer = new FramedWriter((data: Uint8Array) => {
        written.push(data);
        return true;
      });

      const result = writer.writeMessage({ type: "test", value: 42 });
      assert.strictEqual(result, true);
      assert.strictEqual(written.length, 1);

      // Verify frame structure: 4-byte length prefix + JSON payload
      const frame = written[0];
      assert.ok(frame.length > 4, "frame should have length prefix + payload");

      const payloadLen = new DataView(
        frame.buffer,
        frame.byteOffset,
        4,
      ).getUint32(0, false);
      assert.strictEqual(payloadLen, frame.length - 4);
    });

    test("message gets auto-assigned message_id", () => {
      let captured: string | undefined;
      const writer = new FramedWriter((data: Uint8Array) => {
        const json = new TextDecoder().decode(data.slice(4));
        const msg = JSON.parse(json) as FramedMessage;
        captured = msg.message_id;
        return true;
      });

      writer.writeMessage({ type: "ping" });
      assert.ok(captured, "message_id should be auto-assigned");
      assert.ok(captured!.startsWith("msg-"), 'message_id should start with "msg-"');
    });

    test("queues message when writeFn returns false", () => {
      const writer = new FramedWriter(() => false);

      const result = writer.writeMessage({ type: "test" });
      assert.strictEqual(result, false);
      assert.strictEqual(writer.queuedCount, 1);
    });

    test("flushes queued messages", () => {
      let canWrite = false;
      const written: Uint8Array[] = [];
      const writer = new FramedWriter((data: Uint8Array) => {
        if (canWrite) {
          written.push(data);
          return true;
        }
        return false;
      });

      // Write a message that gets queued
      writer.writeMessage({ type: "first" });
      assert.strictEqual(writer.queuedCount, 1);

      // Enable writing and flush
      canWrite = true;
      writer.flush();
      assert.strictEqual(writer.queuedCount, 0);
      assert.strictEqual(written.length, 1);
    });

    test("queuedCount returns correct count", () => {
      const writer = new FramedWriter(() => false);
      assert.strictEqual(writer.queuedCount, 0);

      writer.writeMessage({ type: "a" });
      assert.strictEqual(writer.queuedCount, 1);

      writer.writeMessage({ type: "b" });
      assert.strictEqual(writer.queuedCount, 2);
    });

    test("each message gets a unique message_id", () => {
      const ids: string[] = [];
      const writer = new FramedWriter((data: Uint8Array) => {
        const json = new TextDecoder().decode(data.slice(4));
        const msg = JSON.parse(json) as FramedMessage;
        ids.push(msg.message_id!);
        return true;
      });

      writer.writeMessage({ type: "a" });
      writer.writeMessage({ type: "b" });
      writer.writeMessage({ type: "c" });

      assert.strictEqual(ids.length, 3);
      assert.notStrictEqual(ids[0], ids[1]);
      assert.notStrictEqual(ids[1], ids[2]);
    });

    test("writeMessage preserves additional message fields", () => {
      let parsed: Record<string, unknown> | undefined;
      const writer = new FramedWriter((data: Uint8Array) => {
        const json = new TextDecoder().decode(data.slice(4));
        parsed = JSON.parse(json);
        return true;
      });

      writer.writeMessage({ type: "rpc.request", method: "test", id: 1 });
      assert.strictEqual(parsed!.type, "rpc.request");
      assert.strictEqual(parsed!.method, "test");
      assert.strictEqual(parsed!.id, 1);
    });
  });

  suite("FramedReader", () => {
    test("parses a single framed message", (done) => {
      const frame = encodeFrame({ type: "test", value: 42 });

      const { stream } = makeStream(frame);
      const reader = new FramedReader(
        stream,
        {
          onMessage: (msg) => {
            assert.strictEqual(msg.type, "test");
            assert.strictEqual(msg.value, 42);
            reader.abort();
            done();
          },
        },
        false,
      );
    });

    test("parses multiple framed messages", (done) => {
      const frame1 = encodeFrame({ type: "a", seq: 1 });
      const frame2 = encodeFrame({ type: "b", seq: 2 });
      const combined = new Uint8Array(frame1.length + frame2.length);
      combined.set(frame1);
      combined.set(frame2, frame1.length);

      let calls = 0;
      const { stream } = makeStream(combined);
      const reader = new FramedReader(
        stream,
        {
          onMessage: (msg) => {
            calls++;
            if (calls === 1) {
              assert.strictEqual(msg.type, "a");
              assert.strictEqual(msg.seq, 1);
            } else if (calls === 2) {
              assert.strictEqual(msg.type, "b");
              assert.strictEqual(msg.seq, 2);
              reader.abort();
              done();
            }
          },
        },
        false,
      );
    });

    test("handles heartbeat.pong internally", (done) => {
      const frame = encodeFrame({ type: "heartbeat.pong" });
      let pongCalled = false;

      const { stream } = makeStream(frame);
      const reader = new FramedReader(
        stream,
        {
          onMessage: () => {
            // Should not be called for pong
          },
          onPong: () => {
            pongCalled = true;
          },
        },
        false,
      );

      // Give it time to process
      setTimeout(() => {
        assert.strictEqual(pongCalled, true);
        reader.abort();
        done();
      }, 50);
    });

    test("handles oversized frame via error callback", (done) => {
      // Create a frame with payload length > MAX_FRAME_SIZE (1 MB)
      const oversized = new Uint8Array(4 + 10);
      new DataView(oversized.buffer).setUint32(0, 1024 * 1024 + 1, false);
      oversized.set(new TextEncoder().encode("too big"), 4);

      const { stream } = makeStream(oversized);
      const reader = new FramedReader(
        stream,
        {
          onMessage: () => {
            // Should not be called
          },
          onError: (err) => {
            assert.ok(
              err.message.includes("Frame payload too large"),
              `Unexpected error: ${err.message}`,
            );
            reader.abort();
            done();
          },
        },
        false,
      );
    });

    test("abort stops processing", (done) => {
      const frame = encodeFrame({ type: "test" });
      let callCount = 0;

      const { stream } = makeStream(frame);
      const reader = new FramedReader(
        stream,
        {
          onMessage: () => {
            callCount++;
          },
        },
        false,
      );

      reader.abort();

      setTimeout(() => {
        assert.strictEqual(callCount, 0);
        done();
      }, 50);
    });

    test("compatibility mode falls back on JSON starting stream", (done) => {
      const jsonData = new TextEncoder().encode(
        '{"type":"fallback","value":1}\n',
      );

      let completed = false;
      let msgReceived = false;
      const { stream } = makeStream(jsonData);
      const reader = new FramedReader(
        stream,
        {
          onMessage: (msg) => {
            if (msg.type === "fallback" && msg.value === 1) {
              if (completed) {
                return;
              }
              completed = true;
              msgReceived = true;
              reader.abort();
              done();
            }
          },
        },
        true, // compatibility mode
      );

      setTimeout(() => {
        if (completed) {
          return;
        }
        completed = true;
        assert.strictEqual(
          msgReceived,
          true,
          "should have received message via fallback",
        );
        reader.abort();
        done();
      }, 100);
    });

    test("compatibility mode detects non-framed 4-byte prefix", (done) => {
      // Prefix that looks like a valid but huge length should fall back
      const data = new Uint8Array(10);
      new DataView(data.buffer).setUint32(0, 999999999, false); // > MAX_FRAME_SIZE
      data.set(new TextEncoder().encode("test"), 4);

      let errored = false;
      const { stream } = makeStream(data);
      const reader = new FramedReader(
        stream,
        {
          onMessage: () => {},
          onError: () => {
            errored = true;
          },
        },
        true, // compatibility mode
      );

      setTimeout(() => {
        // With compatibility mode, huge prefix means fallback
        // Verify the onError callback was triggered
        assert.ok(errored, "onError should have been called");
        reader.abort();
        done();
      }, 50);
    });

    test("feed method processes data manually", () => {
      let received: FramedMessage | undefined;
      const reader = new FramedReader(
        {
          getReader: () => ({
            read: async () => ({
              done: true,
              value: new Uint8Array(0),
            }),
            cancel: async () => {},
          }),
        },
        {
          onMessage: (msg) => {
            received = msg;
          },
        },
        false,
      );

      const frame = encodeFrame({ type: "manual", ok: true });
      reader.feed(frame);

      assert.ok(received, "should have received message via feed()");
      assert.strictEqual(received!.type, "manual");
      assert.strictEqual(received!.ok, true);

      reader.abort();
    });
  });

  suite("FramedReader + FramedWriter round-trip", () => {
    test("write then read produces identical content", (done) => {
      const original = { type: "rpc.request", method: "echo", params: { x: 1 } };

      const written: Uint8Array[] = [];
      const writer = new FramedWriter((data: Uint8Array) => {
        written.push(data);
        return true;
      });

      writer.writeMessage(original);

      // Now feed the written frame back into a reader
      const combined = written[0];
      let parsed: Record<string, unknown> | undefined;

      const { stream } = makeStream(combined);
      const reader = new FramedReader(
        stream,
        {
          onMessage: (msg) => {
            parsed = msg as Record<string, unknown>;
            reader.abort();

            // The parsed message should have the original fields plus message_id
            assert.strictEqual(parsed!.type, original.type);
            assert.strictEqual(parsed!.method, original.method);
            assert.deepStrictEqual(parsed!.params, original.params);
            assert.ok(parsed!.message_id, "should have message_id");

            done();
          },
        },
        false,
      );
    });
  });
});
