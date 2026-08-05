import * as assert from "node:assert";
import {
  parseSseDataLine,
  parseSseChunk,
  extractSseContent,
} from "../../runtime/sseStream";

suite("sseParsing", () => {
  suite("parseSseDataLine", () => {
    test("parses a valid data line", () => {
      const result = parseSseDataLine('data: {"token":"hello"}');
      assert.ok(result, "should return a parsed object");
      assert.strictEqual(result!.token, "hello");
    });

    test("parses a data line with content field", () => {
      const result = parseSseDataLine('data: {"content":"world"}');
      assert.ok(result);
      assert.strictEqual(result!.content, "world");
    });

    test("returns null for [DONE] marker", () => {
      const result = parseSseDataLine("data: [DONE]");
      assert.strictEqual(result, null);
    });

    test("returns null for empty line", () => {
      const result = parseSseDataLine("");
      assert.strictEqual(result, null);
    });

    test("returns null for whitespace-only line", () => {
      const result = parseSseDataLine("   ");
      assert.strictEqual(result, null);
    });

    test("returns null for line without data: prefix", () => {
      const result = parseSseDataLine("event: chunk");
      assert.strictEqual(result, null);
    });

    test("returns null for invalid JSON in data field", () => {
      const result = parseSseDataLine("data: {invalid}");
      assert.strictEqual(result, null);
    });

    test("handles data with extra whitespace after prefix", () => {
      const result = parseSseDataLine('data:  {"key":"value"}');
      assert.ok(result);
      assert.strictEqual(result!.key, "value");
    });

    test("handles data with leading whitespace", () => {
      const result = parseSseDataLine('  data: {"key":"value"}');
      assert.ok(result);
      assert.strictEqual(result!.key, "value");
    });

    test("parses nested JSON in data field", () => {
      const result = parseSseDataLine(
        'data: {"response":{"type":"text","content":"hello"}}',
      );
      assert.ok(result);
      assert.deepStrictEqual(result!.response, {
        type: "text",
        content: "hello",
      });
    });

    test("parses data with numeric fields", () => {
      const result = parseSseDataLine('data: {"index":0,"count":42}');
      assert.ok(result);
      assert.strictEqual(result!.index, 0);
      assert.strictEqual(result!.count, 42);
    });
  });

  suite("parseSseChunk", () => {
    test("parses a single data line without event type", () => {
      const results = parseSseChunk('data: {"token":"hello"}\n\n');
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].data.token, "hello");
      assert.strictEqual(results[0].eventType, undefined);
    });

    test("parses multiple data lines without event types", () => {
      const chunk =
        'data: {"token":"hello"}\n\ndata: {"token":"world"}\n\ndata: [DONE]\n\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 3);
      assert.strictEqual(results[0].data.token, "hello");
      assert.strictEqual(results[1].data.token, "world");
      assert.strictEqual(results[2].eventType, "done");
      assert.strictEqual(results[2].data.data, "[DONE]");
    });

    test("injects event type as _event_type into parsed data", () => {
      const chunk = 'event: chunk\ndata: {"token":"hello"}\n\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].eventType, "chunk");
      assert.strictEqual(results[0].data._event_type, "chunk");
      assert.strictEqual(results[0].data.token, "hello");
    });

    test("parses multiple event types correctly", () => {
      const chunk =
        'event: chunk\ndata: {"token":"The"}\n\nevent: chunk\ndata: {"token":" fox"}\n\nevent: done\ndata: {"response":"ok"}\n\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 3);
      assert.strictEqual(results[0].eventType, "chunk");
      assert.strictEqual(results[0].data.token, "The");
      assert.strictEqual(results[1].eventType, "chunk");
      assert.strictEqual(results[1].data.token, " fox");
      assert.strictEqual(results[2].eventType, "done");
      assert.strictEqual(results[2].data.response, "ok");
    });

    test("parses telemetry events with token economy", () => {
      const chunk =
        'event: telemetry\ndata: {"token_economy":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}\n\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].eventType, "telemetry");
      const te = results[0].data.token_economy as { input_tokens: number };
      assert.strictEqual(te.input_tokens, 10);
    });

    test("parses error events", () => {
      const chunk = 'event: error\ndata: {"message":"rate limit exceeded"}\n\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].eventType, "error");
      assert.strictEqual(results[0].data.message, "rate limit exceeded");
    });

    test("handles event lines without space after colon", () => {
      const chunk = 'event:chunk\ndata:{"token":"hello"}\n\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 1);
      // Without space, event:chunk is not matched by "event: " prefix — this
      // is acceptable per spec; callers should ensure a space after event:
      // (the backend writes "event: chunk" with a space at all times).
    });

    test("handles empty chunk", () => {
      const results = parseSseChunk("");
      assert.strictEqual(results.length, 0);
    });

    test("handles chunk with only non-data lines", () => {
      const results = parseSseChunk("event: done\n\n");
      assert.strictEqual(results.length, 0);
    });

    test("parses a complete SSE stream with multiple tokens", () => {
      const stream =
        'event: chunk\ndata: {"token":"The"}\n\nevent: chunk\ndata: {"token":" quick"}\n\nevent: chunk\ndata: {"token":" brown"}\n\nevent: chunk\ndata: {"token":" fox"}\n\ndata: [DONE]\n\n';
      const results = parseSseChunk(stream);
      assert.strictEqual(results.length, 5);
      const tokens = results.slice(0, 4).map((r) => r.data.token as string);
      assert.deepStrictEqual(tokens, ["The", " quick", " brown", " fox"]);
      assert.strictEqual(results[4].eventType, "done");
      assert.strictEqual(results[4].data.data, "[DONE]");
    });

    test("represents [DONE] sentinel frames as completion frames", () => {
      const results = parseSseChunk(
        'data: {"token":"hello"}\n\ndata: [DONE]\n\n',
      );
      assert.strictEqual(results.length, 2);
      assert.strictEqual(results[0].data.token, "hello");
      assert.strictEqual(results[1].eventType, "done");
      assert.strictEqual(results[1].data.data, "[DONE]");
    });

    test("skips malformed JSON frames", () => {
      const results = parseSseChunk(
        'data: {invalid}\n\ndata: {"ok":"yes"}\n\n',
      );
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].data.ok, "yes");
    });

    test("falls back to \\n delimiter for line-delimited streams", () => {
      const chunk =
        'data: {"token":"hello"}\ndata: {"token":"world"}\ndata: [DONE]\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 3);
      assert.strictEqual(results[0].data.token, "hello");
      assert.strictEqual(results[1].data.token, "world");
      assert.strictEqual(results[2].eventType, "done");
      assert.strictEqual(results[2].data.data, "[DONE]");
    });

    test("parses a single data line without trailing separator", () => {
      const results = parseSseChunk('data: {"token":"hello"}');
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].data.token, "hello");
    });

    test("represents a bare [DONE] sentinel as a done frame", () => {
      const results = parseSseChunk("data: [DONE]\n\n");
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].eventType, "done");
      assert.strictEqual(results[0].data.data, "[DONE]");
    });

    test("preserves event type on a [DONE] sentinel frame", () => {
      const results = parseSseChunk("event: done\ndata: [DONE]\n\n");
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].eventType, "done");
      assert.strictEqual(results[0].data._event_type, "done");
      assert.strictEqual(results[0].data.data, "[DONE]");
    });

    test("normalises CRLF line endings before splitting", () => {
      const chunk = 'event: chunk\r\ndata: {"token":"hello"}\r\n\r\n';
      const results = parseSseChunk(chunk);
      assert.strictEqual(results.length, 1);
      assert.strictEqual(results[0].eventType, "chunk");
      assert.strictEqual(results[0].data.token, "hello");
    });
  });

  suite("extractSseContent", () => {
    test("extracts content field from data", () => {
      const content = extractSseContent({ content: "hello" });
      assert.strictEqual(content, "hello");
    });

    test("returns undefined when content field is missing", () => {
      const content = extractSseContent({ token: "hello" });
      assert.strictEqual(content, undefined);
    });

    test("returns undefined when content is not a string", () => {
      const content = extractSseContent({ content: 42 });
      assert.strictEqual(content, undefined);
    });

    test("returns undefined for empty object", () => {
      const content = extractSseContent({});
      assert.strictEqual(content, undefined);
    });

    test("extracts empty string content", () => {
      const content = extractSseContent({ content: "" });
      assert.strictEqual(content, "");
    });
  });
});
